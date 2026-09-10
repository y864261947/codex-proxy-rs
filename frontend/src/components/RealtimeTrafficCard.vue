<script setup lang="ts">
import { RefreshCw } from '@lucide/vue'
import { computed } from 'vue'
import BaseCard from '@/components/base/BaseCard.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import { useRealtimeTraffic } from '@/composables/useRealtimeTraffic'
import { formatDateTime } from '@/utils/date'
import { formatInteger } from '@/utils/number'

const { snapshot, loading, error, refresh } = useRealtimeTraffic()
const metrics = computed(() => [
  { label: '实时并发', value: snapshot.value?.inFlightRequests, detail: '准备中 + 执行中 · 按逻辑请求计数' },
  { label: '入口 RPM', value: snapshot.value?.ingressRequestsLastMinute, detail: '最近 60 秒业务请求 · 包含接入失败' },
  { label: '准备中', value: snapshot.value?.preparingRequests, detail: '路由、准入及首个执行会话准备' },
  { label: '执行中', value: snapshot.value?.executingRequests, detail: '包含流式交付与重试 · 结束后释放' },
])
</script>

<template>
  <BaseCard title="实时负载" description="当前服务 · 每 5 秒更新">
    <template #actions>
      <BaseIconButton label="刷新实时负载" :loading="loading" :disabled="loading" @click="refresh">
        <RefreshCw :size="18" />
      </BaseIconButton>
    </template>
    <div class="grid grid-cols-1 gap-5 sm:grid-cols-2 xl:grid-cols-4">
      <div v-for="metric in metrics" :key="metric.label" class="min-w-0">
        <div class="text-sm font-emphasis text-cp-text-secondary">
          {{ metric.label }}
        </div>
        <div class="mt-2 text-3xl font-heavy text-cp-text tabular-nums" :class="{ 'opacity-50': error }">
          {{ metric.value === undefined ? '—' : formatInteger(metric.value) }}
        </div>
        <p class="mt-2 text-xs leading-relaxed text-cp-text-muted">
          {{ metric.detail }}
        </p>
      </div>
    </div>
    <p v-if="error" role="status" class="mt-4 text-sm text-cp-danger">
      {{ error }}{{ snapshot ? '，显示上次采样，请勿视为当前负载。' : '，尚无可用采样。' }}
    </p>
    <p class="mt-4 text-xs leading-relaxed text-cp-text-muted">
      <template v-if="snapshot">
        采样于 {{ formatDateTime(snapshot.observedAt) }}。
        <template v-if="snapshot.uptimeSeconds < snapshot.windowSeconds">
          服务启动不足 60 秒，RPM 正在积累。
        </template>
      </template>
      统计精度 1 秒，重启清零；不含探针、后台查询或空闲 WebSocket 连接。
    </p>
  </BaseCard>
</template>
