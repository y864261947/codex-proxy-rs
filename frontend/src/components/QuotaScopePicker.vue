<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseTablePagination from '@/components/base/BaseTable/BaseTablePagination.vue'
import { useQuotaScopesQuery } from '@/composables/useQuotaScopesQuery'

defineProps<{ disabled?: boolean }>()
const emit = defineEmits<{ ready: [value: boolean] }>()
const selection = defineModel<string | null>({ required: true })
const { quotaScopes, search, loaded, loading, loadError, load, resize, pagination } = useQuotaScopesQuery()
const known = ref(new Map<string, { name: string, enabled: boolean }>())
watch(quotaScopes, (items) => {
  for (const item of items) known.value.set(item.id, item)
})
const selected = computed(() => selection.value ? known.value.get(selection.value) : undefined)
watch(() => loaded.value && !loading.value && !loadError.value, value => emit('ready', value), { immediate: true })
</script>

<template>
  <fieldset class="min-w-0 rounded-lg border border-cp-border p-4" :disabled="disabled">
    <legend class="px-1 text-cp-sm font-medium">
      共享配额（可选）
    </legend>
    <p class="mb-3 text-cp-xs text-cp-text-secondary">
      关联后，与使用同一配额的其他渠道、号池共用并发和 RPM 上限。
    </p>
    <div class="mb-3 flex items-center justify-between gap-3">
      <span class="min-w-0 truncate text-cp-sm">{{ selection ? (selected?.name || '已选择共享配额') : '未关联共享配额' }}</span>
      <BaseButton v-if="selection" variant="ghost" :disabled="disabled || loading || !!loadError" @click="selection = null">
        解除关联
      </BaseButton>
    </div>
    <p v-if="selection && selected && !selected.enabled" role="status" class="mb-3 text-cp-warning">
      此配额已停用，关联来源暂不能接受新请求。
    </p>
    <BaseInput v-model="search" aria-label="搜索共享配额" placeholder="按配额名称搜索" :disabled="disabled" />
    <div v-if="loadError" role="alert" class="my-3 flex items-center gap-3 text-cp-error">
      {{ loadError }}；已保留选择，请重试后保存。
      <BaseButton variant="ghost" :disabled="disabled || loading" @click="load()">
        重试
      </BaseButton>
    </div>
    <div class="my-3 grid max-h-40 gap-1 overflow-y-auto" role="group" aria-label="可选共享配额" :aria-busy="loading">
      <button v-for="quota in quotaScopes" :key="quota.id" type="button" class="flex items-center justify-between gap-3 rounded-lg px-3 py-2 text-left text-cp-sm hover:bg-(--cp-input-bg)" :class="selection === quota.id ? 'bg-cp-primary-container text-cp-primary-text' : 'text-cp-text'" :disabled="disabled || loading || !!loadError" :aria-pressed="selection === quota.id" @click="selection = quota.id">
        <span class="min-w-0 truncate">{{ quota.name }}</span>
        <span class="shrink-0 text-cp-xs">{{ quota.enabled ? `${quota.maxConcurrency || '不限'} 并发 / ${quota.requestsPerMinute || '不限'} RPM` : '停用' }}</span>
      </button>
      <p v-if="!loading && !loadError && !quotaScopes.length" class="py-2 text-cp-xs text-cp-text-secondary">
        {{ search ? '没有匹配的配额，已选项仍然保留。' : '暂无共享配额，可在上游渠道 → 共享配额中创建。' }}
      </p>
    </div>
    <BaseTablePagination :pagination="pagination" :loading="loading" @page-change="load" @page-size-change="resize" />
  </fieldset>
</template>
