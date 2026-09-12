<script setup lang="ts">
import type { ApiKey, CustomerRef } from '@/api'
import { computed, ref, watch } from 'vue'
import { assignKeyCustomer } from '@/api'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import BaseTablePagination from '@/components/base/BaseTable/BaseTablePagination.vue'
import { toast } from '@/components/base/BaseToast'
import { useCustomersQuery } from '@/composables/useCustomersQuery'
import { errorMessage } from '@/utils/async'

const props = defineProps<{ apiKey: ApiKey | null }>()
const emit = defineEmits<{ saved: [] }>()
const open = defineModel<boolean>({ required: true })
const { customers, search, loading, loadError, load, resize, pagination } = useCustomersQuery(false)
const selected = ref('')
const pickedCustomer = ref<CustomerRef | null>(null)
const saving = ref(false)
const options = computed(() => {
  const options = customers.value.map(customer => ({ value: customer.id, label: `${customer.name}${customer.enabled ? '' : '（已停用）'}` }))
  const current = props.apiKey?.customer
  if (current && !options.some(option => option.value === current.id))
    options.unshift({ value: current.id, label: `${current.name}（当前归属）` })
  const picked = pickedCustomer.value
  if (picked && !options.some(option => option.value === picked.id))
    options.unshift({ value: picked.id, label: `${picked.name}${picked.enabled ? '' : '（已停用）'}` })
  return [{ value: '', label: '不归属客户' }, ...options]
})
watch(open, (value) => {
  if (value) {
    pickedCustomer.value = props.apiKey?.customer || null
    selected.value = props.apiKey?.customer?.id || ''
    search.value = ''
    void load(1)
  }
})
watch(selected, (value) => {
  pickedCustomer.value = customers.value.find(customer => customer.id === value) || (props.apiKey?.customer?.id === value ? props.apiKey.customer : null)
})
async function save() {
  if (!props.apiKey || saving.value || loading.value || loadError.value)
    return
  saving.value = true
  try {
    await assignKeyCustomer(props.apiKey.id, selected.value || null)
    toast.success('密钥归属已更新')
    open.value = false
    emit('saved')
  }
  catch (error: unknown) { toast.error(errorMessage(error, '修改归属失败')) }
  finally { saving.value = false }
}
</script>

<template>
  <BaseModal v-model="open" title="设置客户归属" :description="`${apiKey?.name || '此密钥'} 将同时受 Key 和客户限额约束；接入分组权限及限额保持生效`" size="md" :dismissible="!saving">
    <div class="grid gap-4">
      <BaseInput v-model="search" aria-label="搜索可分配客户" placeholder="搜索客户名称" :disabled="saving" />
      <div v-if="loadError" role="alert" class="text-cp-error">
        {{ loadError }} <BaseButton variant="ghost" @click="load()">
          重试
        </BaseButton>
      </div>
      <BaseFormItem label="客户">
        <BaseSelect v-model="selected" :options="options" :disabled="loading || saving || !!loadError" />
      </BaseFormItem>
      <BaseTablePagination :pagination="pagination" :loading="loading || saving" @page-change="load" @page-size-change="resize" />
      <p class="m-0 text-cp-sm text-cp-text-secondary">
        归属变更用于后续请求；正在执行的请求继续占用原客户额度。
      </p>
    </div>
    <template #footer>
      <BaseButton variant="ghost" :disabled="saving" @click="open = false">
        取消
      </BaseButton><BaseButton variant="primary" :loading="saving" :disabled="loading || !!loadError" @click="save">
        保存归属
      </BaseButton>
    </template>
  </BaseModal>
</template>
