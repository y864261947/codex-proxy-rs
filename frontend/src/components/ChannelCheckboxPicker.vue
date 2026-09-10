<script setup lang="ts">
import { watch } from 'vue'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseTablePagination from '@/components/base/BaseTable/BaseTablePagination.vue'
import { useChannelsQuery } from '@/composables/useChannelsQuery'

const props = defineProps<{ disabled?: boolean }>()
const emit = defineEmits<{ ready: [ready: boolean] }>()
const selected = defineModel<string[]>({ required: true })
const { channels, search, loading, loaded, loadError, load, resize, pagination } = useChannelsQuery()
watch([loading, loaded, loadError], () => {
  emit('ready', loaded.value && !loading.value && !loadError.value)
}, { immediate: true })
function select(id: string, checked: boolean) {
  const ids = new Set(selected.value)
  if (checked)
    ids.add(id)
  else ids.delete(id)
  selected.value = [...ids]
}
</script>

<template>
  <div class="grid gap-3">
    <div class="flex flex-wrap items-center justify-between gap-2">
      <BaseInput v-model="search" aria-label="搜索可用渠道" placeholder="搜索渠道名称" :disabled="props.disabled" class="max-w-xs" />
      <span class="text-cp-xs text-cp-text-secondary">已选 {{ selected.length }} 个；切换搜索或分页保留选择</span>
    </div>
    <div v-if="loadError" role="alert" class="flex items-center gap-2 text-cp-error">
      {{ loadError }}<BaseButton variant="ghost" @click="load()">
        重试
      </BaseButton>
    </div>
    <div class="grid gap-2 sm:grid-cols-2">
      <div v-for="channel in channels" :key="channel.id" class="flex min-h-11 items-center justify-between gap-3 rounded-cp bg-cp-fill-quaternary px-3.5 py-2.5">
        <BaseCheckbox :model-value="selected.includes(channel.id)" :label="channel.name" show-label :disabled="props.disabled || loading || !!loadError || (!selected.includes(channel.id) && selected.length >= 256)" @update:model-value="select(channel.id, $event)" />
        <span class="shrink-0 text-cp-xs text-cp-text-secondary">{{ channel.enabled ? (channel.provider === 'openai_api' ? 'Responses' : channel.provider) : '已停用' }}</span>
      </div>
    </div>
    <p v-if="!channels.length && !loadError" class="m-0 text-cp-sm text-cp-text-secondary">
      {{ loading ? '正在读取渠道…' : '没有匹配渠道，可先在上游渠道中添加。' }}
    </p>
    <BaseTablePagination :pagination="pagination" :loading="loading" @page-change="load" @page-size-change="resize" />
  </div>
</template>
