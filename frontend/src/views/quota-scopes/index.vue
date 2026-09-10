<script setup lang="ts">
import type { QuotaScope, QuotaScopeWrite } from '@/api'
import { computed, ref } from 'vue'
import { createQuotaScope, deleteQuotaScope, updateQuotaScope } from '@/api'
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
import UpstreamNavigation from '@/components/UpstreamNavigation.vue'
import { useQuotaScopesQuery } from '@/composables/useQuotaScopesQuery'
import { errorMessage } from '@/utils/async'

const { quotaScopes, search, loading, loadError, load, resize, pagination } = useQuotaScopesQuery()
const columns = defineTableColumns<QuotaScope>([
  { key: 'identity', label: '共享配额', kind: 'identity', size: 'xl' },
  { key: 'enabled', label: '状态', kind: 'status' },
  { key: 'sourceCount', label: '关联来源', kind: 'numeric' },
  { key: 'maxConcurrency', label: '共享并发上限', kind: 'numeric' },
  { key: 'requestsPerMinute', label: '共享 RPM 上限', kind: 'numeric' },
  { key: 'actions', label: '操作', kind: 'actions', size: 'xl' },
])
const open = ref(false)
const deleteOpen = ref(false)
const editing = ref<QuotaScope | null>(null)
const deleting = ref<QuotaScope | null>(null)
const saving = ref(false)
const form = ref<QuotaScopeWrite>({ name: '', note: '', enabled: true, maxConcurrency: 0, requestsPerMinute: 0 })
const valid = computed(() => form.value.name.trim().length > 0 && [form.value.maxConcurrency, form.value.requestsPerMinute].every(value => Number.isSafeInteger(value) && value >= 0))
function edit(quotaScope: QuotaScope | null) {
  editing.value = quotaScope
  form.value = quotaScope ? { name: quotaScope.name, note: quotaScope.note || '', enabled: quotaScope.enabled, maxConcurrency: quotaScope.maxConcurrency, requestsPerMinute: quotaScope.requestsPerMinute } : { name: '', note: '', enabled: true, maxConcurrency: 0, requestsPerMinute: 0 }
  open.value = true
}
async function save() {
  if (saving.value || !valid.value)
    return
  saving.value = true
  try {
    const data = { ...form.value, name: form.value.name.trim(), note: form.value.note?.trim() || null }
    if (editing.value)
      await updateQuotaScope({ ...data, id: editing.value.id })
    else await createQuotaScope(data)
    open.value = false
    toast.success('共享配额已保存')
    await load()
  }
  catch (error: unknown) { toast.error(errorMessage(error, '保存共享配额失败')) }
  finally { saving.value = false }
}
function requestDelete(quotaScope: QuotaScope) {
  deleting.value = quotaScope
  deleteOpen.value = true
}
async function remove() {
  if (!deleting.value || saving.value)
    return
  saving.value = true
  try {
    await deleteQuotaScope(deleting.value.id)
    deleteOpen.value = false
    toast.success('共享配额已删除')
    await load()
  }
  catch (error: unknown) { toast.error(errorMessage(error, '删除失败，请先解除关联渠道和号池')) }
  finally { saving.value = false }
}
</script>

<template>
  <div class="flex h-full min-h-0 w-full flex-col">
    <BasePageHeader title="共享配额" description="多个渠道或号池共用同一上游项目时，集中限制总并发和 RPM；停用后关联来源停止接受新请求" />
    <UpstreamNavigation />
    <BaseCard class="mt-5 flex min-h-125 flex-col">
      <template #header>
        <div class="flex flex-wrap items-center justify-between gap-3">
          <BaseInput v-model="search" class="max-w-xs" aria-label="搜索共享配额" placeholder="搜索共享配额名称" />
          <BaseButton variant="primary" @click="edit(null)">
            创建共享配额
          </BaseButton>
        </div>
      </template>
      <template #body>
        <div v-if="loadError" role="alert" class="mb-4 flex items-center gap-3 text-cp-error">
          {{ loadError }}{{ quotaScopes.length ? '；当前保留上次结果。' : '' }}
          <BaseButton variant="ghost" @click="load()">
            重试
          </BaseButton>
        </div>
        <BaseTable :columns="columns" :rows="quotaScopes" :loading="loading" empty-text="暂无共享配额；创建后可在渠道或号池编辑页关联">
          <template #identity="{ row }">
            <div class="grid gap-1">
              <strong>{{ row.name }}</strong><span class="max-w-sm truncate text-cp-xs text-cp-text-secondary">{{ row.note || '未填写备注' }}</span>
            </div>
          </template>
          <template #enabled="{ row }">
            <span :class="row.enabled ? 'text-cp-success' : 'text-cp-text-secondary'">{{ row.enabled ? '启用' : '停用' }}</span>
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
              </BaseButton><BaseButton variant="ghost" :disabled="row.sourceCount > 0" :title="row.sourceCount ? '请先解除关联渠道和号池' : '删除共享配额'" @click="requestDelete(row)">
                删除
              </BaseButton>
            </div>
          </template>
        </BaseTable>
        <BaseTablePagination :pagination="pagination" :loading="loading" @page-change="load" @page-size-change="resize" />
      </template>
    </BaseCard>
    <BaseModal v-model="open" :title="editing ? '编辑共享配额' : '创建共享配额'" description="0 表示本层不限制；渠道、号池及下游自身限额继续生效" :dismissible="!saving" size="md">
      <BaseForm class="grid gap-5">
        <BaseFormItem label="共享配额名称" required>
          <BaseInput v-model="form.name" aria-label="共享配额名称" :maxlength="128" :disabled="saving" />
        </BaseFormItem>
        <BaseFormItem label="备注">
          <BaseTextarea :model-value="form.note || ''" aria-label="共享配额备注" :maxlength="1024" :disabled="saving" @update:model-value="form.note = $event" />
        </BaseFormItem>
        <BaseFormItem label="共享配额状态" description="停用将阻止全部关联渠道和号池的新请求">
          <BaseSwitch v-model="form.enabled" label="启用共享配额" :show-label="true" :disabled="saving" />
        </BaseFormItem>
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
        </BaseButton><BaseButton variant="primary" :loading="saving" :disabled="!valid" @click="save">
          保存共享配额
        </BaseButton>
      </template>
    </BaseModal>
    <BaseConfirmModal v-model="deleteOpen" title="删除共享配额" :description="`删除 ${deleting?.name || ''}？`" destructive :loading="saving" @confirm="remove" />
  </div>
</template>
