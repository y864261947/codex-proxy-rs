<script setup lang="ts">
import type { AccessGroup, AccessGroupWrite } from '@/api'
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
import { useAccessGroupsQuery } from '@/composables/useAccessGroupsQuery'
import { useAccountGroupCatalog } from '@/composables/useAccountGroupCatalog'
import { errorMessage } from '@/utils/async'

const { accessGroups, search, loading, loadError, load, resize, pagination } = useAccessGroupsQuery()
const { groups: pools, loading: poolsLoading, loadError: poolsError, loaded: poolsLoaded, loadGroups } = useAccountGroupCatalog({ immediate: false })
const modelText = ref('')
const models = computed(() => [...new Set(modelText.value.split(/\r?\n/).map(value => value.trim()).filter(Boolean))])
const permissionsValid = computed(() => poolsLoaded.value && !poolsLoading.value && !poolsError.value && models.value.length <= 2048 && models.value.every(model => model !== '*' && new TextEncoder().encode(model).length <= 256))
const columns = defineTableColumns<AccessGroup>([
  { key: 'identity', label: '接入分组', kind: 'identity', size: 'xl' },
  { key: 'enabled', label: '状态', kind: 'status' },
  { key: 'permissions', label: '模型 / 号池', kind: 'numeric' },
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
const form = ref<AccessGroupWrite>({ name: '', note: '', enabled: true, maxConcurrency: 0, requestsPerMinute: 0, allowedModels: [], poolGroupIds: [] })
const valid = computed(() => form.value.name.trim().length > 0 && [form.value.maxConcurrency, form.value.requestsPerMinute].every(value => Number.isSafeInteger(value) && value >= 0))
function edit(accessGroup: AccessGroup | null) {
  modelText.value = accessGroup?.allowedModels.join('\n') || ''
  void loadGroups()
  editing.value = accessGroup
  form.value = accessGroup ? { name: accessGroup.name, note: accessGroup.note || '', enabled: accessGroup.enabled, maxConcurrency: accessGroup.maxConcurrency, requestsPerMinute: accessGroup.requestsPerMinute, allowedModels: [...accessGroup.allowedModels], poolGroupIds: [...accessGroup.poolGroupIds] } : { name: '', note: '', enabled: true, maxConcurrency: 0, requestsPerMinute: 0, allowedModels: [], poolGroupIds: [] }
  open.value = true
}
async function save() {
  if (saving.value || !valid.value || !permissionsValid.value)
    return
  saving.value = true
  try {
    const data = { ...form.value, allowedModels: models.value, name: form.value.name.trim(), note: form.value.note?.trim() || null }
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
    <BasePageHeader title="接入分组" description="定义对外模型白名单、允许使用的号池与组共享限额；分配给 Key 后生效" />
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
            <span :class="!row.allowedModels.length || !row.poolGroupIds.length ? 'text-cp-warning' : ''">{{ row.allowedModels.length }} / {{ row.poolGroupIds.length }}</span>
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
    <BaseModal v-model="open" :title="editing ? '编辑接入分组' : '创建接入分组'" description="空模型或空号池表示未授权；组、客户和 Key 的限额同时生效" :dismissible="!saving" size="lg">
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
        <p v-if="!models.length || !form.poolGroupIds.length" class="m-0 text-cp-sm text-cp-warning">
          尚未完整授权，保存后该组 Key 暂时无法调用模型。
        </p>
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
