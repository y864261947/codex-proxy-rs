<script setup lang="ts">
import type { AccessGroupRef, ApiKey } from '@/api'
import { computed, ref, watch } from 'vue'
import { assignKeyAccessGroup } from '@/api'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import BaseTablePagination from '@/components/base/BaseTable/BaseTablePagination.vue'
import { toast } from '@/components/base/BaseToast'
import { useAccessGroupsQuery } from '@/composables/useAccessGroupsQuery'
import { errorMessage } from '@/utils/async'

const props = defineProps<{ apiKey: ApiKey | null }>()
const emit = defineEmits<{ saved: [] }>()
const open = defineModel<boolean>({ required: true })
const { accessGroups, search, loading, loadError, load, resize, pagination } = useAccessGroupsQuery(false)
const selected = ref('')
const pickedAccessGroup = ref<AccessGroupRef | null>(null)
const saving = ref(false)
const options = computed(() => {
  const options = accessGroups.value.map(accessGroup => ({ value: accessGroup.id, label: `${accessGroup.name}${accessGroup.enabled ? '' : '（已停用）'}` }))
  const current = props.apiKey?.accessGroup
  if (current && !options.some(option => option.value === current.id))
    options.unshift({ value: current.id, label: `${current.name}（当前归属）` })
  const picked = pickedAccessGroup.value
  if (picked && !options.some(option => option.value === picked.id))
    options.unshift({ value: picked.id, label: `${picked.name}${picked.enabled ? '' : '（已停用）'}` })
  return [{ value: '', label: '恢复旧账号分组权限' }, ...options]
})
watch(open, (value) => {
  if (value) {
    pickedAccessGroup.value = props.apiKey?.accessGroup || null
    selected.value = props.apiKey?.accessGroup?.id || ''
    search.value = ''
    void load(1)
  }
})
watch(selected, (value) => {
  pickedAccessGroup.value = accessGroups.value.find(accessGroup => accessGroup.id === value) || (props.apiKey?.accessGroup?.id === value ? props.apiKey.accessGroup : null)
})
async function save() {
  if (!props.apiKey || saving.value || loading.value || loadError.value)
    return
  saving.value = true
  try {
    await assignKeyAccessGroup(props.apiKey.id, selected.value || null)
    toast.success('密钥归属已更新')
    open.value = false
    emit('saved')
  }
  catch (error: unknown) { toast.error(errorMessage(error, '修改归属失败')) }
  finally { saving.value = false }
}
</script>

<template>
  <BaseModal v-model="open" title="设置接入分组" :description="`${apiKey?.name || '此密钥'} 将使用所选组的模型和来源权限；客户和 Key 限额继续生效`" size="md" :dismissible="!saving">
    <div class="grid gap-4">
      <BaseInput v-model="search" aria-label="搜索可分配接入分组" placeholder="搜索接入分组名称" :disabled="saving" />
      <div v-if="loadError" role="alert" class="text-cp-error">
        {{ loadError }} <BaseButton variant="ghost" @click="load()">
          重试
        </BaseButton>
      </div>
      <BaseFormItem label="接入分组">
        <BaseSelect v-model="selected" :options="options" :disabled="loading || saving || !!loadError" />
      </BaseFormItem>
      <BaseTablePagination :pagination="pagination" :loading="loading || saving" @page-change="load" @page-size-change="resize" />
      <p class="m-0 text-cp-sm text-cp-text-secondary">
        归属变更用于后续请求；正在执行的请求继续占用原接入分组额度。
      </p>
      <p v-if="!selected" class="m-0 text-cp-sm text-cp-warning">
        解除接入分组将恢复原账号分组权限：{{ apiKey?.groups.length ? apiKey.groups.map(group => group.name).join('、') : '全部账号' }}。
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
