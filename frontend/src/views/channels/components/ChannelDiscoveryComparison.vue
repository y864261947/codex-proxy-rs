<script setup lang="ts">
import type { Channel, ChannelDiscoveryComparison, ChannelDiscoveryReference } from '@/api'
import { computed, onScopeDispose, ref, watch } from 'vue'
import { compareChannelModelDiscoveries } from '@/api'
import BaseButton from '@/components/base/BaseButton.vue'
import { errorMessage } from '@/utils/async'

const props = defineProps<{ channel: Channel, base: ChannelDiscoveryReference | null, target: ChannelDiscoveryReference | null }>()
const emit = defineEmits<{ clear: [] }>()
const result = ref<ChannelDiscoveryComparison | null>(null)
const loading = ref(false)
const failure = ref('')
const ordered = computed(() => !!props.base && !!props.target && BigInt(props.base.generation) < BigInt(props.target.generation))
let sequence = 0
let controller: AbortController | undefined

function reset() {
  sequence++
  controller?.abort()
  loading.value = false
  failure.value = ''
  result.value = null
}
watch(() => [props.channel.id, props.base?.generation, props.target?.generation], reset, { flush: 'sync' })
onScopeDispose(reset)

async function compare() {
  if (!ordered.value || !props.base || !props.target || loading.value)
    return
  controller?.abort()
  controller = new AbortController()
  const request = ++sequence
  const baseGeneration = props.base.generation
  const targetGeneration = props.target.generation
  loading.value = true
  failure.value = ''
  try {
    const comparison = await compareChannelModelDiscoveries(props.channel.id, baseGeneration, targetGeneration, controller.signal)
    if (request !== sequence)
      return
    if (comparison.id !== props.channel.id || comparison.base.generation !== baseGeneration || comparison.target.generation !== targetGeneration)
      throw new Error('比较结果与所选记录不匹配，请重新读取历史')
    result.value = comparison
  }
  catch (error: unknown) {
    if (request === sequence)
      failure.value = errorMessage(error, '发现记录比较失败')
  }
  finally {
    if (request === sequence)
      loading.value = false
  }
}
</script>

<template>
  <section class="grid min-w-0 gap-3 rounded border border-cp-border p-3 text-cp-xs" aria-label="发现结果比较">
    <p class="m-0 text-cp-text-secondary">
      从历史中分别选择基准（较早）和目标（较晚），可跨页选择；只比较两次上游返回的模型集合，不改草稿或配置。
    </p>
    <p class="m-0 break-all">
      基准：{{ base ? `序号 ${base.generation} · 连接版本 ${base.connectionRevision} · ${new Date(base.fetchedAt).toLocaleString()}` : '未选择' }}
    </p>
    <p class="m-0 break-all">
      目标：{{ target ? `序号 ${target.generation} · 连接版本 ${target.connectionRevision} · ${new Date(target.fetchedAt).toLocaleString()}` : '未选择' }}
    </p>
    <p v-if="base && target && !ordered" role="alert" class="m-0 text-cp-warning">
      基准序号必须小于目标序号，请选择两条不同记录；不按查询时间判断顺序。
    </p>
    <div class="flex flex-wrap gap-2">
      <BaseButton variant="ghost" :disabled="!ordered" :loading="loading" @click="compare">
        比较所选记录
      </BaseButton>
      <BaseButton variant="ghost" :disabled="!base && !target" @click="emit('clear')">
        清空比较选择
      </BaseButton>
    </div>
    <p v-if="failure" role="alert" class="m-0 text-cp-error">
      {{ failure }}{{ result ? '；仍显示此前对此组记录的比较，本次读取未确认。' : '' }}
    </p>
    <template v-if="result">
      <p v-if="!result.sameConnectionRevision" role="alert" class="m-0 text-cp-warning">
        两条记录的连接版本不同，差异可能来自地址、凭据或配置变化，不应直接判定为上游新增或下架。
      </p>
      <p v-if="result.target.connectionRevision !== channel.connectionRevision" class="m-0 text-cp-warning">
        目标记录也不是当前连接版本，仅供历史查看。
      </p>
      <p class="m-0">
        新出现 {{ result.appeared.length }} · 本次未再发现 {{ result.disappeared.length }} · 两次均发现 {{ result.unchanged.length }}
      </p>
      <p v-if="!result.appeared.length && !result.disappeared.length" class="m-0">
        两次发现的模型集合相同
      </p>
      <div class="grid max-h-52 gap-2 overflow-auto break-all text-cp-text-secondary">
        <p class="m-0">
          新出现：<span class="font-mono">{{ result.appeared.join('、') || '无' }}</span>
        </p>
        <p class="m-0">
          本次未再发现：<span class="font-mono">{{ result.disappeared.join('、') || '无' }}</span>
        </p>
        <p class="m-0">
          两次均发现：<span class="font-mono">{{ result.unchanged.join('、') || '无' }}</span>
        </p>
      </div>
      <p class="m-0 text-cp-text-secondary">
        未再发现不代表应删除，也不证明模型能力、价格或可用性。采用模型仍需重新查询当前渠道并走原有草稿保存流程。
      </p>
    </template>
  </section>
</template>
