<script setup lang="ts">
import type { AccessGroup, AccessGroupSourcePreference, AccessGroupWrite } from '@/api'
import { computed, ref } from 'vue'
import { createAccessGroup, deleteAccessGroup, updateAccessGroup } from '@/api'
import AccountGroupCheckboxGrid from '@/components/AccountGroupCheckboxGrid.vue'
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
import ChannelCheckboxPicker from '@/components/ChannelCheckboxPicker.vue'
import { useAccessGroupsQuery } from '@/composables/useAccessGroupsQuery'
import { useAccountGroupCatalog } from '@/composables/useAccountGroupCatalog'
import { errorMessage } from '@/utils/async'

const { accessGroups, search, loading, loadError, load, resize, pagination } = useAccessGroupsQuery()
const { groups: pools, loading: poolsLoading, loadError: poolsError, loaded: poolsLoaded, loadGroups } = useAccountGroupCatalog({ immediate: false })
const modelText = ref('')
const channelsReady = ref(false)
const models = computed(() => [...new Set(modelText.value.split(/\r?\n/).map(value => value.trim()).filter(Boolean))])
const columns = defineTableColumns<AccessGroup>([
  { key: 'identity', label: '接入分组', kind: 'identity', size: 'xl' },
  { key: 'enabled', label: '状态', kind: 'status' },
  { key: 'permissions', label: '模型 / 来源', kind: 'numeric' },
  { key: 'keyCount', label: '密钥数', kind: 'numeric' },
  { key: 'maxConcurrency', label: '共享并发上限', kind: 'numeric' },
  { key: 'requestsPerMinute', label: '共享 RPM 上限', kind: 'numeric' },
  { key: 'actions', label: '操作', kind: 'actions', size: 'xl' },
])
const open = ref(false)
const deleteOpen = ref(false)
const editing = ref<AccessGroup | null>(null)
const deleting = ref<AccessGroup | null>(null)
const saving = ref(false)
const form = ref<AccessGroupWrite>({ allowCapacityFallback: true, sourcePreferences: [], name: '', note: '', enabled: true, maxConcurrency: 0, requestsPerMinute: 0, allowedModels: [], poolGroupIds: [], channelIds: [] })
const valid = computed(() => form.value.name.trim().length > 0 && [form.value.maxConcurrency, form.value.requestsPerMinute].every(value => Number.isSafeInteger(value) && value >= 0))
const routingSources = computed(() => [
  ...form.value.poolGroupIds.map(sourceId => ({ kind: 'account_pool' as const, sourceId, name: pools.value.find(pool => pool.id === sourceId)?.name || sourceId })),
  ...form.value.channelIds.map(sourceId => ({ kind: 'channel' as const, sourceId, name: sourceId })),
])
const routingValid = computed(() => activePreferences().every(preference => [preference.priority, preference.weight].every(value => value === null || (Number.isInteger(value) && value >= 1 && value <= 65535))))
const permissionsValid = computed(() => routingValid.value && channelsReady.value && form.value.channelIds.length <= 256 && poolsLoaded.value && !poolsLoading.value && !poolsError.value && models.value.length <= 2048 && models.value.every(model => model !== '*' && new TextEncoder().encode(model).length <= 256))
function activePreferences() {
  return form.value.sourcePreferences.filter(preference => routingSources.value.some(source => source.kind === preference.kind && source.sourceId === preference.sourceId))
}
function preferenceValue(source: { kind: AccessGroupSourcePreference['kind'], sourceId: string }, field: 'priority' | 'weight') {
  const value = form.value.sourcePreferences.find(preference => preference.kind === source.kind && preference.sourceId === source.sourceId)?.[field]
  return value === undefined || value === null ? '' : String(value)
}
function setPreference(source: { kind: AccessGroupSourcePreference['kind'], sourceId: string }, field: 'priority' | 'weight', value: string) {
  let preference = form.value.sourcePreferences.find(preference => preference.kind === source.kind && preference.sourceId === source.sourceId)
  if (!preference) {
    preference = { kind: source.kind, sourceId: source.sourceId, priority: null, weight: null }
    form.value.sourcePreferences.push(preference)
  }
  preference[field] = value.trim() === '' ? null : Number(value)
  if (preference.priority === null && preference.weight === null)
    form.value.sourcePreferences = form.value.sourcePreferences.filter(item => item.kind !== source.kind || item.sourceId !== source.sourceId)
}

function edit(accessGroup: AccessGroup | null) {
  channelsReady.value = false
  modelText.value = accessGroup?.allowedModels.join('\n') || ''
  void loadGroups()
  editing.value = accessGroup
  form.value = accessGroup ? { allowCapacityFallback: accessGroup.allowCapacityFallback, sourcePreferences: accessGroup.sourcePreferences.map(preference => ({ ...preference })), name: accessGroup.name, note: accessGroup.note || '', enabled: accessGroup.enabled, maxConcurrency: accessGroup.maxConcurrency, requestsPerMinute: accessGroup.requestsPerMinute, allowedModels: [...accessGroup.allowedModels], poolGroupIds: [...accessGroup.poolGroupIds], channelIds: [...accessGroup.channelIds] } : { allowCapacityFallback: true, sourcePreferences: [], name: '', note: '', enabled: true, maxConcurrency: 0, requestsPerMinute: 0, allowedModels: [], poolGroupIds: [], channelIds: [] }
  open.value = true
}
async function save() {
  if (saving.value || !valid.value || !permissionsValid.value)
    return
  saving.value = true
  try {
    const data = { ...form.value, sourcePreferences: activePreferences(), allowedModels: models.value, name: form.value.name.trim(), note: form.value.note?.trim() || null }
    if (editing.value)
      await updateAccessGroup({ ...data, id: editing.value.id })
    else await createAccessGroup(data)
    open.value = false
    toast.success('接入分组已保存')
    await load()
  }
  catch (error: unknown) { toast.error(errorMessage(error, '保存接入分组失败')) }
  finally { saving.value = false }
}
function requestDelete(accessGroup: AccessGroup) {
  deleting.value = accessGroup
  deleteOpen.value = true
}
async function remove() {
  if (!deleting.value || saving.value)
    return
  saving.value = true
  try {
    await deleteAccessGroup(deleting.value.id)
    deleteOpen.value = false
    toast.success('接入分组已删除')
    await load()
  }
  catch (error: unknown) { toast.error(errorMessage(error, '删除失败，请先解除关联密钥')) }
  finally { saving.value = false }
}
</script>

<template>
  <div class="flex h-full min-h-0 w-full flex-col">
    <BasePageHeader title="接入分组" description="定义对外模型白名单、允许使用的号池和渠道，以及组共享限额；分配给 Key 后生效" />
    <BaseCard class="mt-5 flex min-h-125 flex-col">
      <template #header>
        <div class="flex flex-wrap items-center justify-between gap-3">
          <BaseInput v-model="search" class="max-w-xs" aria-label="搜索接入分组" placeholder="搜索接入分组名称" />
          <BaseButton variant="primary" @click="edit(null)">
            创建接入分组
          </BaseButton>
        </div>
      </template>
      <template #body>
        <div v-if="loadError" role="alert" class="mb-4 flex items-center gap-3 text-cp-error">
          {{ loadError }}{{ accessGroups.length ? '；当前保留上次结果。' : '' }}
          <BaseButton variant="ghost" @click="load()">
            重试
          </BaseButton>
        </div>
        <BaseTable :columns="columns" :rows="accessGroups" :loading="loading" empty-text="暂无接入分组；创建后可在 API 密钥中分配归属">
          <template #identity="{ row }">
            <div class="grid gap-1">
              <strong>{{ row.name }}</strong><span class="max-w-sm truncate text-cp-xs text-cp-text-secondary">{{ row.note || '未填写备注' }}</span>
            </div>
          </template>
          <template #enabled="{ row }">
            <span :class="row.enabled ? 'text-cp-success' : 'text-cp-text-secondary'">{{ row.enabled ? '启用' : '停用' }}</span>
          </template>
          <template #permissions="{ row }">
            <span :class="!row.allowedModels.length || !(row.poolGroupIds.length + row.channelIds.length) ? 'text-cp-warning' : ''">{{ row.allowedModels.length }} / {{ row.poolGroupIds.length + row.channelIds.length }}</span>
          </template>
          <template #maxConcurrency="{ row }">
            {{ row.maxConcurrency || '不限制' }}
          </template>
          <template #requestsPerMinute="{ row }">
            {{ row.requestsPerMinute || '不限制' }}
          </template>
          <template #actions="{ row }">
            <div class="flex gap-2">
              <BaseButton variant="ghost" @click="edit(row)">
                编辑
              </BaseButton><BaseButton variant="ghost" :disabled="row.keyCount > 0" :title="row.keyCount ? '请先在 API 密钥中解除或更改归属' : '删除接入分组'" @click="requestDelete(row)">
                删除
              </BaseButton>
            </div>
          </template>
        </BaseTable>
        <BaseTablePagination :pagination="pagination" :loading="loading" @page-change="load" @page-size-change="resize" />
      </template>
    </BaseCard>
    <BaseModal v-model="open" :title="editing ? '编辑接入分组' : '创建接入分组'" description="至少授权一个模型和一个来源才可调用；组、客户和 Key 的限额同时生效" :dismissible="!saving" size="lg">
      <BaseForm class="grid gap-5">
        <BaseFormItem label="接入分组名称" required>
          <BaseInput v-model="form.name" aria-label="接入分组名称" :maxlength="128" :disabled="saving" />
        </BaseFormItem>
        <BaseFormItem label="备注">
          <BaseTextarea :model-value="form.note || ''" aria-label="接入分组备注" :maxlength="1024" :disabled="saving" @update:model-value="form.note = $event" />
        </BaseFormItem>
        <BaseFormItem label="接入分组状态" description="停用将影响该接入分组的全部 Key">
          <BaseSwitch v-model="form.enabled" label="启用接入分组" :show-label="true" :disabled="saving" />
        </BaseFormItem>
        <BaseFormItem label="开放模型" description="每行一个对外模型名，精确匹配；别名填写对外名称，暂不支持通配符">
          <BaseTextarea v-model="modelText" aria-label="开放模型" :disabled="saving" placeholder="例如：my-coding-model" />
        </BaseFormItem>
        <BaseFormItem label="允许的号池" description="仅勾选的号池参与调度；账号分组为空或停用时无法提供账号">
          <p v-if="poolsError" role="alert" class="text-cp-error">
            {{ poolsError }} <BaseButton variant="ghost" @click="loadGroups()">
              重试
            </BaseButton>
          </p>
          <AccountGroupCheckboxGrid v-model="form.poolGroupIds" :groups="pools" :loading="poolsLoading" :disabled="saving || !!poolsError" />
        </BaseFormItem>
        <fieldset class="m-0 min-w-0 border-0 p-0">
          <legend class="mb-2 text-cp-sm font-emphasis">
            允许的渠道
          </legend>
          <ChannelCheckboxPicker v-if="open" v-model="form.channelIds" :disabled="saving" @ready="channelsReady = $event" />
        </fieldset>
        <p v-if="!models.length || !(form.poolGroupIds.length + form.channelIds.length)" class="m-0 text-cp-sm text-cp-warning">
          尚未完整授权，保存后该组 Key 暂时无法调用模型。
        </p>
        <BaseFormItem label="满载回退" description="关闭后，来源或账号容量不足时只尝试同优先级来源，不降级到低优先级；故障重试和会话来源锁定规则不变">
          <BaseSwitch v-model="form.allowCapacityFallback" label="允许满载后使用低优先级来源" :show-label="true" :disabled="saving" />
        </BaseFormItem>
        <fieldset v-if="routingSources.length" class="m-0 grid min-w-0 gap-3 border-0 p-0">
          <legend class="mb-2 text-cp-sm font-emphasis">
            分组来源偏好
          </legend>
          <p class="m-0 text-cp-xs text-cp-text-secondary">
            留空继承来源默认值；优先级数值越小越优先，同级按权重分配。取值 1–65535，不改变来源容量或共享配额。
          </p>
          <div v-for="source in routingSources" :key="source.kind + source.sourceId" class="grid gap-2 rounded-cp border border-cp-border p-3">
            <span class="break-all text-cp-sm">{{ source.kind === 'account_pool' ? '号池' : '渠道' }} · {{ source.name }}</span>
            <div class="grid grid-cols-2 gap-3">
              <BaseFormItem label="优先级覆盖">
                <BaseInput :model-value="preferenceValue(source, 'priority')" type="number" min="1" max="65535" step="1" placeholder="继承来源" :aria-label="`${source.name}优先级覆盖`" :disabled="saving" @update:model-value="setPreference(source, 'priority', $event)" />
              </BaseFormItem>
              <BaseFormItem label="权重覆盖">
                <BaseInput :model-value="preferenceValue(source, 'weight')" type="number" min="1" max="65535" step="1" placeholder="继承来源" :aria-label="`${source.name}权重覆盖`" :disabled="saving" @update:model-value="setPreference(source, 'weight', $event)" />
              </BaseFormItem>
            </div>
          </div>
          <p v-if="!routingValid" role="alert" class="m-0 text-cp-sm text-cp-error">
            优先级和权重须为 1–65535 的整数，或留空继承。
          </p>
        </fieldset>
        <BaseFormItem label="共享并发上限">
          <BaseNumberInput v-model="form.maxConcurrency" label="共享并发上限" :min="0" :max="Number.MAX_SAFE_INTEGER" :disabled="saving" />
        </BaseFormItem>
        <BaseFormItem label="共享 RPM 上限">
          <BaseNumberInput v-model="form.requestsPerMinute" label="共享 RPM 上限" :min="0" :max="Number.MAX_SAFE_INTEGER" :disabled="saving" />
        </BaseFormItem>
      </BaseForm>
      <template #footer>
        <BaseButton variant="ghost" :disabled="saving" @click="open = false">
          取消
        </BaseButton><BaseButton variant="primary" :loading="saving" :disabled="!valid || !permissionsValid" @click="save">
          保存接入分组
        </BaseButton>
      </template>
    </BaseModal>
    <BaseConfirmModal v-model="deleteOpen" title="删除接入分组" :description="`删除 ${deleting?.name || ''}？`" destructive :loading="saving" @confirm="remove" />
  </div>
</template>
