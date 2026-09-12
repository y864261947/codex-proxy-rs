<script setup lang="ts">
import type { Channel, ChannelFields, ResponsesChannelConfig } from '@/api'
import { computed, onMounted, onScopeDispose, ref, watch } from 'vue'
import { createChannel, deleteChannel, getChannelConnection, getChannelProviders, updateChannel } from '@/api'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCard from '@/components/base/BaseCard.vue'
import BaseConfirmModal from '@/components/base/BaseConfirmModal.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseForm from '@/components/base/BaseForm/index.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import BaseNumberInput from '@/components/base/BaseNumberInput.vue'
import BasePageHeader from '@/components/base/BasePageHeader.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'
import BaseTablePagination from '@/components/base/BaseTable/BaseTablePagination.vue'
import { defineTableColumns } from '@/components/base/BaseTable/columns'
import BaseTable from '@/components/base/BaseTable/index.vue'
import BaseTextarea from '@/components/base/BaseTextarea.vue'
import { toast } from '@/components/base/BaseToast'
import QuotaScopePicker from '@/components/QuotaScopePicker.vue'
import UpstreamNavigation from '@/components/UpstreamNavigation.vue'
import { useChannelsQuery } from '@/composables/useChannelsQuery'
import { errorMessage } from '@/utils/async'
import ChannelModelDiscovery from './components/ChannelModelDiscovery.vue'

const { channels, search, loading, loadError, load, resize, pagination } = useChannelsQuery()
const columns = defineTableColumns<Channel>([
  { key: 'identity', label: '渠道', kind: 'identity', size: 'xl' },
  { key: 'enabled', label: '状态', kind: 'status' },
  { key: 'preference', label: '优先级 / 权重', kind: 'numeric' },
  { key: 'capacity', label: '并发 / RPM 上限', kind: 'numeric' },
  { key: 'actions', label: '操作', kind: 'actions', size: 'xl' },
])
const open = ref(false)
const deleteOpen = ref(false)
const scheduleConfirmOpen = ref(false)
const editing = ref<Channel | null>(null)
const deleting = ref<Channel | null>(null)
const saving = ref(false)
const quotaReady = ref(false)
watch(open, () => {
  quotaReady.value = false
})
const connectionLoading = ref(false)
const connectionError = ref('')
const providers = ref<string[]>([])
const providersError = ref('')
const providersLoading = ref(false)
const form = ref<ChannelFields>(defaults())
const baseUrl = ref('https://api.openai.com/v1')
const apiKey = ref('')
const modelText = ref('')
const organization = ref('')
const project = ref('')
const hasApiKey = ref(false)
const savedConnection = ref('')
const connectionIdentity = computed(() => JSON.stringify([baseUrl.value, organization.value, project.value]))
const discoveryDisabled = computed(() => saving.value || connectionLoading.value || !!connectionError.value || !!apiKey.value || connectionIdentity.value !== savedConnection.value)
let editController: AbortController | undefined
let editSequence = 0
const lifetime = new AbortController()

function defaults(): ChannelFields {
  return { name: '', note: '', enabled: true, discoveryIntervalMinutes: null, priority: 1, weight: 1, maxConcurrency: 0, requestsPerMinute: 0, quotaScopeId: null }
}
const models = computed(() => modelText.value.split(/\r?\n/).map(value => value.trim()).filter(Boolean))
const urlValid = computed(() => {
  try {
    const value = new URL(baseUrl.value.trim())
    return ['http:', 'https:'].includes(value.protocol) && !!value.hostname && !value.username && !value.password && !value.search && !value.hash
  }
  catch { return false }
})
const valid = computed(() => quotaReady.value && !connectionLoading.value && !connectionError.value && !providersError.value
  && providers.value.includes('openai_api') && (!editing.value || editing.value.provider === 'openai_api')
  && !!form.value.name.trim() && urlValid.value
  && (form.value.discoveryIntervalMinutes === null || (Number.isInteger(form.value.discoveryIntervalMinutes) && form.value.discoveryIntervalMinutes >= 5 && form.value.discoveryIntervalMinutes <= 1440))
  && models.value.length > 0 && models.value.length <= 1000 && new Set(models.value).size === models.value.length
  && (apiKey.value ? /^[\x21-\x7E]+$/.test(apiKey.value) && apiKey.value.length <= 16384 : !!editing.value && hasApiKey.value)
  && [organization.value, project.value].every(value => !value || /^[\w-]{1,256}$/.test(value))
  && [form.value.priority, form.value.weight].every(value => Number.isInteger(value) && value > 0 && value <= 65535)
  && [form.value.maxConcurrency, form.value.requestsPerMinute].every(value => Number.isSafeInteger(value) && value >= 0))

async function loadProviders() {
  if (providersLoading.value)
    return
  providersLoading.value = true
  providersError.value = ''
  try {
    providers.value = await getChannelProviders(lifetime.signal)
  }
  catch (error: unknown) {
    if (!lifetime.signal.aborted)
      providersError.value = errorMessage(error, '协议列表加载失败')
  }
  finally { providersLoading.value = false }
}

async function loadConnection(channel: Channel) {
  editController?.abort()
  editController = new AbortController()
  const sequence = ++editSequence
  connectionLoading.value = true
  connectionError.value = ''
  try {
    if (channel.provider !== 'openai_api')
      throw new Error('当前页面尚不支持编辑此协议的连接配置')
    const result = await getChannelConnection(channel.id, editController.signal)
    if (sequence !== editSequence || !open.value)
      return
    if (result.connectionRevision !== channel.connectionRevision || result.provider !== channel.provider)
      throw new Error('渠道已被更新，请关闭窗口、刷新列表后重新编辑')
    baseUrl.value = result.config.baseUrl
    modelText.value = result.config.models.join('\n')
    organization.value = result.config.organization || ''
    project.value = result.config.project || ''
    hasApiKey.value = result.config.hasApiKey
    savedConnection.value = connectionIdentity.value
  }
  catch (error: unknown) {
    if (sequence === editSequence && open.value)
      connectionError.value = errorMessage(error, '连接配置加载失败')
  }
  finally {
    if (sequence === editSequence)
      connectionLoading.value = false
  }
}

function edit(channel: Channel | null) {
  if (saving.value)
    return
  editing.value = channel
  form.value = channel ? { name: channel.name, note: channel.note || '', enabled: channel.enabled, discoveryIntervalMinutes: channel.discoveryIntervalMinutes, priority: channel.priority, weight: channel.weight, maxConcurrency: channel.maxConcurrency, requestsPerMinute: channel.requestsPerMinute, quotaScopeId: channel.quotaScopeId } : defaults()
  baseUrl.value = channel ? '' : 'https://api.openai.com/v1'
  apiKey.value = ''
  modelText.value = ''
  organization.value = ''
  project.value = ''
  hasApiKey.value = false
  savedConnection.value = ''
  connectionError.value = ''
  open.value = true
  if (channel)
    void loadConnection(channel)
}

function requestSave() {
  if (!valid.value || saving.value)
    return
  if (form.value.discoveryIntervalMinutes !== null && (form.value.discoveryIntervalMinutes !== editing.value?.discoveryIntervalMinutes || (form.value.enabled && !editing.value?.enabled)))
    scheduleConfirmOpen.value = true
  else void save()
}

async function save() {
  if (saving.value || !valid.value)
    return
  saving.value = true
  try {
    const data = { ...form.value, name: form.value.name.trim(), note: form.value.note?.trim() || null }
    const config: ResponsesChannelConfig = { baseUrl: baseUrl.value.trim(), models: models.value, organization: organization.value || null, project: project.value || null }
    if (apiKey.value)
      config.apiKey = apiKey.value
    if (editing.value)
      await updateChannel({ ...data, id: editing.value.id, expectedRevision: editing.value.connectionRevision, config })
    else await createChannel({ ...data, provider: 'openai_api', config })
    open.value = false
    scheduleConfirmOpen.value = false
    apiKey.value = ''
    toast.success('渠道已保存')
    await load()
  }
  catch (error: unknown) { toast.error(errorMessage(error, '保存渠道失败')) }
  finally { saving.value = false }
}

function requestDelete(channel: Channel) {
  deleting.value = channel
  deleteOpen.value = true
}
async function remove() {
  if (!deleting.value || saving.value)
    return
  saving.value = true
  try {
    await deleteChannel(deleting.value.id, deleting.value.connectionRevision)
    deleteOpen.value = false
    toast.success('渠道已删除')
    await load()
  }
  catch (error: unknown) { toast.error(errorMessage(error, '删除渠道失败，请刷新后重试')) }
  finally { saving.value = false }
}
watch(open, (value) => {
  if (!value) {
    apiKey.value = ''
    editSequence++
    editController?.abort()
    connectionLoading.value = false
  }
})
onMounted(() => {
  void loadProviders()
})
onScopeDispose(() => {
  editSequence++
  editController?.abort()
  lifetime.abort()
  apiKey.value = ''
})
</script>

<template>
  <div class="flex h-full min-h-0 w-full flex-col">
    <BasePageHeader title="上游渠道" description="集中管理官方 API 和第三方上游，每个渠道独立配置连接、模型和调度策略" />
    <UpstreamNavigation />
    <BaseCard class="mt-5 flex min-h-125 flex-col">
      <template #header>
        <div class="flex flex-wrap items-center justify-between gap-3">
          <BaseInput v-model="search" class="max-w-xs" aria-label="搜索渠道" placeholder="搜索渠道名称" />
          <div class="flex gap-2">
            <BaseButton variant="ghost" :disabled="loading" @click="load()">
              刷新
            </BaseButton>
            <BaseButton variant="primary" :disabled="!providers.includes('openai_api') || !!providersError" @click="edit(null)">
              新增渠道
            </BaseButton>
          </div>
        </div>
      </template>
      <template #body>
        <div v-if="providersError" role="alert" class="mb-4 flex items-center gap-3 text-cp-error">
          {{ providersError }}<BaseButton variant="ghost" @click="loadProviders">
            重试协议列表
          </BaseButton>
        </div>
        <div v-if="loadError" role="alert" class="mb-4 flex items-center gap-3 text-cp-error">
          {{ loadError }}{{ channels.length ? '；当前保留上次结果。' : '' }}<BaseButton variant="ghost" @click="load()">
            重试
          </BaseButton>
        </div>
        <BaseTable :columns="columns" :rows="channels" :loading="loading" empty-text="暂无渠道，添加上游连接后再分配给接入分组">
          <template #identity="{ row }">
            <div class="grid gap-1">
              <strong>{{ row.name }}</strong><span class="text-cp-xs text-cp-text-secondary">{{ row.provider === 'openai_api' ? 'OpenAI Responses' : row.provider }}</span>
              <span v-if="row.note" class="max-w-sm truncate text-cp-xs text-cp-text-secondary">{{ row.note }}</span>
            </div>
          </template>
          <template #enabled="{ row }">
            <span :class="row.enabled ? 'text-cp-success' : 'text-cp-text-secondary'">{{ row.enabled ? '启用' : '停用' }}</span>
            <div class="mt-1 text-cp-xs text-cp-text-secondary">
              {{ row.discoveryIntervalMinutes === null ? '定时发现关闭' : row.enabled ? `每 ${row.discoveryIntervalMinutes} 分钟发现` : '定时发现暂停' }}
            </div>
          </template>
          <template #preference="{ row }">
            {{ row.priority }} / {{ row.weight }}
          </template>
          <template #capacity="{ row }">
            {{ row.maxConcurrency || '不限' }} / {{ row.requestsPerMinute || '不限' }}
          </template>
          <template #actions="{ row }">
            <div class="flex gap-2">
              <BaseButton variant="ghost" :disabled="saving" @click="edit(row)">
                编辑
              </BaseButton><BaseButton variant="ghost" :disabled="saving" @click="requestDelete(row)">
                删除
              </BaseButton>
            </div>
          </template>
        </BaseTable>
        <BaseTablePagination :pagination="pagination" :loading="loading" @page-change="load" @page-size-change="resize" />
      </template>
    </BaseCard>
    <BaseModal v-model="open" :title="editing ? '编辑渠道' : '新增渠道'" description="OpenAI Responses 协议 · 支持官方 API 或兼容该协议的第三方服务" :dismissible="!saving" size="lg">
      <div v-if="connectionLoading" role="status" class="mb-4 text-cp-text-secondary">
        正在读取连接配置…
      </div>
      <div v-if="connectionError" role="alert" class="mb-4 grid gap-2 text-cp-error">
        {{ connectionError }}<BaseButton v-if="editing" variant="ghost" @click="loadConnection(editing)">
          重新读取
        </BaseButton>
      </div>
      <BaseForm class="grid gap-5 sm:grid-cols-2">
        <BaseFormItem label="渠道名称" required>
          <BaseInput v-model="form.name" aria-label="渠道名称" :maxlength="128" :disabled="saving" placeholder="例如 A 渠道" />
        </BaseFormItem>
        <BaseFormItem label="状态">
          <BaseSwitch v-model="form.enabled" label="启用渠道" :show-label="true" :disabled="saving" />
        </BaseFormItem>
        <BaseFormItem label="API 地址" required class="sm:col-span-2" description="填写 API 根路径，例如 https://api.openai.com/v1；不要包含密钥或 /responses">
          <BaseInput v-model="baseUrl" aria-label="API 地址" :maxlength="2048" :disabled="saving || connectionLoading || !!connectionError" />
        </BaseFormItem>
        <BaseFormItem label="API Key" :required="!editing" class="sm:col-span-2" :description="editing ? '留空保留当前 Key；输入新 Key 将替换。已有 Key 不会回显。' : '填写该上游的 API Key'">
          <BaseInput v-model="apiKey" aria-label="API Key" type="password" autocomplete="new-password" :maxlength="16384" :disabled="saving || connectionLoading || !!connectionError" :placeholder="editing && hasApiKey ? '已配置，留空保留' : '输入 API Key'" />
        </BaseFormItem>
        <BaseFormItem label="模型 ID" required class="sm:col-span-2" description="每行一个上游模型 ID，填写此渠道实际提供的模型">
          <BaseTextarea v-model="modelText" aria-label="模型 ID" :rows="4" :disabled="saving || connectionLoading || !!connectionError" />
          <ChannelModelDiscovery v-if="open && editing?.provider === 'openai_api'" :key="`${editing.id}:${editing.connectionRevision}`" :channel="editing" :disabled="discoveryDisabled" :models="models" @append="modelText = [...new Set([...models, ...$event])].join('\n')" />
        </BaseFormItem>
        <BaseFormItem label="定时模型发现" class="sm:col-span-2" description="默认关闭。保存后后台只查询模型列表并保留发现历史，不自动增删配置，不发送对话。停用渠道时暂停；每次保存渠道后重新计时，不补跑错过的周期。">
          <BaseSwitch :model-value="form.discoveryIntervalMinutes !== null" label="允许后台定时查询此上游" :show-label="true" :disabled="saving" @update:model-value="form.discoveryIntervalMinutes = $event ? 60 : null" />
          <BaseNumberInput v-if="form.discoveryIntervalMinutes !== null" v-model="form.discoveryIntervalMinutes" class="mt-3" label="发现间隔（分钟）" :min="5" :max="1440" :disabled="saving" />
          <div v-if="editing" class="mt-3 grid gap-1 text-cp-xs text-cp-text-secondary">
            <span>已保存计划的状态（以列表最近读取为准，关闭后刷新列表可更新）：</span>
            <span>下次到期：{{ editing.discoveryIntervalMinutes !== null && editing.enabled ? editing.discoverySchedule.nextDueAt || '尚未排期' : '未启用或已暂停' }}（实际执行可能排队延迟）</span>
            <span>最近尝试：{{ editing.discoverySchedule.attemptedAt || '暂无' }}</span>
            <span>结果：{{ editing.discoverySchedule.succeeded === true ? '已确认成功，结果已写入发现历史' : editing.discoverySchedule.attemptedAt ? '未确认成功；可能失败、中断或仍在执行，请查看发现历史' : '尚未执行' }}</span>
            <span>关闭计划无法撤回已领取或已发送的查询；旧版本结果拒绝保存。历史记录不会自动清理。</span>
          </div>
        </BaseFormItem>
        <BaseFormItem label="Organization（可选）">
          <BaseInput v-model="organization" aria-label="Organization" :maxlength="256" :disabled="saving || connectionLoading || !!connectionError" />
        </BaseFormItem>
        <BaseFormItem label="Project（可选）">
          <BaseInput v-model="project" aria-label="Project" :maxlength="256" :disabled="saving || connectionLoading || !!connectionError" />
        </BaseFormItem>
        <BaseFormItem label="调度优先级" description="数值越小越优先">
          <BaseNumberInput v-model="form.priority" label="调度优先级" :min="1" :max="65535" :disabled="saving" />
        </BaseFormItem>
        <BaseFormItem label="同级权重" description="同一优先级内，权重越大被选中的机会越高">
          <BaseNumberInput v-model="form.weight" label="同级权重" :min="1" :max="65535" :disabled="saving" />
        </BaseFormItem>
        <BaseFormItem label="并发上限" description="0 表示本渠道不额外限制">
          <BaseNumberInput v-model="form.maxConcurrency" label="渠道并发上限" :min="0" :max="Number.MAX_SAFE_INTEGER" :disabled="saving" />
        </BaseFormItem>
        <BaseFormItem label="RPM 上限" description="0 表示本渠道不额外限制">
          <BaseNumberInput v-model="form.requestsPerMinute" label="渠道 RPM 上限" :min="0" :max="Number.MAX_SAFE_INTEGER" :disabled="saving" />
        </BaseFormItem>
        <div v-if="open" class="sm:col-span-2">
          <QuotaScopePicker v-model="form.quotaScopeId" :disabled="saving || connectionLoading || !!connectionError" @ready="quotaReady = $event" />
        </div>
        <BaseFormItem label="备注" class="sm:col-span-2">
          <BaseTextarea :model-value="form.note || ''" aria-label="渠道备注" :maxlength="1024" :disabled="saving" @update:model-value="form.note = $event" />
        </BaseFormItem>
      </BaseForm>
      <template #footer>
        <BaseButton variant="ghost" :disabled="saving" @click="open = false">
          取消
        </BaseButton><BaseButton variant="primary" :loading="saving" :disabled="!valid" @click="requestSave">
          保存渠道
        </BaseButton>
      </template>
    </BaseModal>
    <BaseConfirmModal v-model="scheduleConfirmOpen" title="保存并启用定时模型发现？" :description="`允许后台每 ${form.discoveryIntervalMinutes} 分钟使用已保存的凭据查询此上游模型列表；停用渠道期间暂停。不自动修改模型配置，不发起对话。保存后开始计时。`" :loading="saving" @confirm="save" />
    <BaseConfirmModal v-model="deleteOpen" title="删除渠道" :description="`删除 ${deleting?.name || ''}？历史请求记录会保留。`" destructive :loading="saving" @confirm="remove" />
  </div>
</template>
