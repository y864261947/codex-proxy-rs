<script setup lang="ts">
import { computed } from 'vue'

const props = defineProps<{
  apiKey: {
    enabled: boolean
    customer?: { enabled: boolean } | null
  }
}>()
const customerDisabled = computed(() => props.apiKey.customer?.enabled === false)
</script>

<template>
  <span
    class="inline-flex h-6 min-w-14 items-center justify-center rounded-full px-2 text-cp-sm leading-none font-bold whitespace-nowrap"
    :class="{
      'bg-cp-success-container text-cp-success-on-container': apiKey.enabled && !customerDisabled,
      'bg-cp-fill-quaternary text-cp-text-secondary': !apiKey.enabled || customerDisabled,
    }"
  >
    {{ !apiKey.enabled ? '已禁用' : customerDisabled ? '客户停用' : '已启用' }}
  </span>
</template>
