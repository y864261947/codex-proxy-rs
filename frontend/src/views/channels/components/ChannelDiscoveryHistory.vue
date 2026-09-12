<script setup lang="ts">
import type { Channel, ChannelDiscoveryPage, ChannelDiscoveryReference } from '@/api'
import { onScopeDispose, ref, watch } from 'vue'
import { getChannelModelDiscoveries } from '@/api'
import BaseButton from '@/components/base/BaseButton.vue'
import { errorMessage } from '@/utils/async'
import ChannelDiscoveryComparison from './ChannelDiscoveryComparison.vue'

const props = defineProps<{ channel: Channel, latestGeneration?: string }>()
const visible = ref(false)
const loading = ref(false)
const failure = ref('')
const page = ref<ChannelDiscoveryPage | null>(null)
const before = ref<string>()
const previous = ref<(string | undefined)[]>([])
const base = ref<ChannelDiscoveryReference | null>(null)
const target = ref<ChannelDiscoveryReference | null>(null)
let sequence = 0
let controller: AbortController | undefined

function cancel() {
  sequence++
  controller?.abort()
  loading.value = false
}
async function load(cursor?: string, stack: (string | undefined)[] = []) {
  cancel()
  controller = new AbortController()
  const request = ++sequence
  loading.value = true
  failure.value = ''
  try {
    const result = await getChannelModelDiscoveries(props.channel.id, cursor, controller.signal)
    if (request !== sequence)
      return
    page.value = result
    before.value = cursor
    previous.value = stack
  }
  catch (error: unknown) {
    if (request === sequence)
      failure.value = errorMessage(error, '发现历史读取失败')
  }
  finally {
    if (request === sequence)
      loading.value = false
  }
}
function toggle() {
  visible.value = !visible.value
  if (visible.value) {
    void load()
  }
  else {
    cancel()
    base.value = null
    target.value = null
  }
}
function older() {
  if (page.value?.nextBeforeGeneration)
    void load(page.value.nextBeforeGeneration, [...previous.value, before.value])
}
function newer() {
  if (previous.value.length)
    void load(previous.value.at(-1), previous.value.slice(0, -1))
}
watch(() => props.channel.id, () => {
  cancel()
  visible.value = false
  page.value = null
  failure.value = ''
  previous.value = []
  before.value = undefined
  base.value = null
  target.value = null
})
watch(() => props.latestGeneration, () => {
  if (visible.value)
    void load()
})
onScopeDispose(cancel)
</script>

<template>
  <section class="grid min-w-0 gap-3 border-t border-cp-border pt-3" aria-label="成功发现历史">
    <BaseButton variant="ghost" class="justify-self-start" :aria-expanded="visible" @click="toggle">
      {{ visible ? '收起发现历史' : '查看发现历史' }}
    </BaseButton>
    <template v-if="visible">
      <p class="m-0 break-all text-cp-xs text-cp-text-secondary">
        当前渠道 {{ channel.name }} · {{ channel.provider }} · {{ channel.id }}。以下为成功保存记录，按发现序号从新到旧排列；仅供查看，不会改动草稿或请求上游。
      </p>
      <div class="flex flex-wrap items-center gap-2">
        <BaseButton variant="ghost" :loading="loading" @click="load()">
          刷新历史
        </BaseButton>
        <BaseButton variant="ghost" :disabled="loading || !previous.length" @click="newer">
          较新记录
        </BaseButton>
        <BaseButton variant="ghost" :disabled="loading || !page?.nextBeforeGeneration" @click="older">
          更早记录
        </BaseButton>
      </div>
      <p v-if="failure" role="alert" class="m-0 text-cp-xs text-cp-error">
        {{ failure }}{{ page ? '；仍显示上次读取的历史，可能不是最新记录。' : '' }}
      </p>
      <ChannelDiscoveryComparison :channel="channel" :base="base" :target="target" @clear="base = null; target = null" />
      <p v-if="page && !page.items.length && !failure" class="m-0 text-cp-xs text-cp-text-secondary">
        {{ before ? '没有更早的成功记录' : '此渠道暂无成功发现历史' }}
      </p>
      <div v-if="page?.items.length" class="grid max-h-80 gap-2 overflow-auto" :aria-busy="loading">
        <details v-for="item in page.items" :key="item.generation" class="min-w-0 rounded border border-cp-border p-3 text-cp-xs">
          <summary class="cursor-pointer break-all leading-relaxed">
            {{ new Date(item.fetchedAt).toLocaleString() }} · 序号 {{ item.generation }} · 连接版本 {{ item.connectionRevision }}
            <span :class="item.connectionRevision === channel.connectionRevision ? 'text-cp-text-secondary' : 'text-cp-warning'">
              {{ item.connectionRevision === channel.connectionRevision ? '当前连接版本 · 只读' : '旧连接版本 · 只读' }}
            </span>
          </summary>
          <div class="mt-3 grid gap-2 break-all text-cp-text-secondary">
            <div class="flex flex-wrap gap-2">
              <BaseButton variant="ghost" :disabled="loading || !!failure" :aria-label="`设为基准 ${item.generation}`" @click="base = item">
                {{ base?.generation === item.generation ? '已选为基准' : '设为基准' }}
              </BaseButton>
              <BaseButton variant="ghost" :disabled="loading || !!failure" :aria-label="`设为目标 ${item.generation}`" @click="target = item">
                {{ target?.generation === item.generation ? '已选为目标' : '设为目标' }}
              </BaseButton>
            </div>
            <p class="m-0">
              相对于查询时配置：新增 {{ item.added.length }} · 未发现 {{ item.missing.length }} · 重合 {{ item.unchanged.length }}。不是与上一次发现的比较，未发现不代表删除。
            </p>
            <p v-if="!item.added.length && !item.unchanged.length" class="m-0">
              本次上游成功返回空模型列表
            </p>
            <p class="m-0">
              新增：<span class="font-mono">{{ item.added.join('、') || '无' }}</span>
            </p>
            <p class="m-0">
              未发现：<span class="font-mono">{{ item.missing.join('、') || '无' }}</span>
            </p>
            <p class="m-0">
              重合：<span class="font-mono">{{ item.unchanged.join('、') || '无' }}</span>
            </p>
          </div>
        </details>
      </div>
      <p class="m-0 text-cp-xs text-cp-text-secondary">
        升级前仅保留的最后一次成功记录会延续；更早已覆盖的记录无法恢复。失败或过期查询不会写入成功历史，当前没有自动清理；删除渠道会同时删除历史。
      </p>
    </template>
  </section>
</template>
