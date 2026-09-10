<script setup lang="ts">
import type { CustomerRef } from '@/api'
import { computed, ref, useId, watch, watchEffect } from 'vue'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import BaseTablePagination from '@/components/base/BaseTable/BaseTablePagination.vue'
import { useAccessGroupsQuery } from '@/composables/useAccessGroupsQuery'
import { useCustomersQuery } from '@/composables/useCustomersQuery'

const props = defineProps<{ kind: 'customer' | 'accessGroup', active: boolean, disabled?: boolean }>()
const emit = defineEmits<{ ready: [value: boolean] }>()
const selected = defineModel<string>({ required: true })
const query = props.kind === 'customer' ? useCustomersQuery(false) : useAccessGroupsQuery(false)
const { search, loading, loadError, load, resize, pagination } = query
const items = computed(() => 'customers' in query ? query.customers.value : query.accessGroups.value)
const picked = ref<CustomerRef | null>(null)
const loaded = ref(false)
const searchId = `owner-search-${useId()}`
const label = computed(() => props.kind === 'customer' ? '客户' : '接入分组')
const options = computed(() => {
  const values = items.value.map(item => ({ value: item.id, label: `${item.name}${item.enabled ? '' : '（停用）'}` }))
  if (picked.value && !values.some(item => item.value === picked.value?.id))
    values.unshift({ value: picked.value.id, label: picked.value.name })
  return [{ value: '', label: props.kind === 'customer' ? '不关联客户' : '使用账号分组权限' }, ...values]
})
watch(selected, (value) => {
  picked.value = items.value.find(item => item.id === value) || (picked.value?.id === value ? picked.value : null)
})
watch(() => props.active, async (active) => {
  if (!active)
    return
  picked.value = null
  loaded.value = false
  search.value = ''
  await load(1)
  loaded.value = true
}, { immediate: true })
watchEffect(() => emit('ready', props.active && loaded.value && !loading.value && !loadError.value))
</script>

<template>
  <div class="grid gap-2">
    <BaseInput :id="searchId" v-model="search" :aria-label="`搜索${label}`" :placeholder="`搜索${label}`" :disabled="disabled" />
    <p v-if="loadError" role="alert" class="m-0 text-cp-sm text-cp-error">
      {{ loadError }} <BaseButton variant="ghost" @click="load()">
        重试
      </BaseButton>
    </p>
    <BaseSelect v-model="selected" :aria-label="`选择${label}`" :options="options" :disabled="disabled || loading || !!loadError" />
    <BaseTablePagination :pagination="pagination" :loading="loading || disabled" @page-change="load" @page-size-change="resize" />
  </div>
</template>
