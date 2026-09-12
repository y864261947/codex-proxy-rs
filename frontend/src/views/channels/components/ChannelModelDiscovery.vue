<script setup lang="ts">
import type { Channel, ChannelModelPreview } from '@/api'
import { Plus, Search } from '@lucide/vue'
import { computed, onScopeDispose, ref, watch } from 'vue'
import { discoverChannelModels, getChannelModelDiscovery } from '@/api'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import { errorMessage } from '@/utils/async'
import ChannelDiscoveryHistory from './ChannelDiscoveryHistory.vue'

const props = defineProps<{ channel: Channel, disabled: boolean, models: string[] }>()
const emit = defineEmits<{ append: [models: string[]] }>()
const preview = ref<ChannelModelPreview | null>(null)
const selected = ref<string[]>([])
const loading = ref(false)
const failure = ref('')
const restored = ref(false)
const loaded = ref(false)
const stale = computed(() => !!preview.value && preview.value.connectionRevision !== props.channel.connectionRevision)
const additions = computed(() => preview.value?.added.filter(model => !props.models.includes(model)) ?? [])
const accepted = computed(() => selected.value.filter(model => additions.value.includes(model)))
let controller: AbortController | undefined
let sequence = 0

function invalidate() {
  sequence++
  controller?.abort()
  loading.value = false
  preview.value = null
  selected.value = []
  failure.value = ''
  loaded.value = false
}
watch(() => [props.channel.id, props.channel.connectionRevision, props.disabled], () => {
  invalidate()
  if (!props.disabled)
    void load(false)
}, { flush: 'sync', immediate: true })
onScopeDispose(invalidate)

async function load(upstream: boolean) {
  if (props.disabled || loading.value)
    return
  controller?.abort()
  controller = new AbortController()
  const request = ++sequence
  loading.value = true
  failure.value = ''
  selected.value = []
  try {
    const result = upstream
      ? await discoverChannelModels(props.channel.id, props.channel.connectionRevision, controller.signal)
      : await getChannelModelDiscovery(props.channel.id, controller.signal)
    if (request !== sequence)
      return
    if (result && (result.id !== props.channel.id || (upstream && result.connectionRevision !== props.channel.connectionRevision)))
      throw new Error('渠道版本已变化，请重新打开编辑窗口')
    preview.value = result
    loaded.value = true
    restored.value = !upstream
  }
  catch (error: unknown) {
    if (request === sequence)
      failure.value = errorMessage(error, upstream ? '模型发现或快照保存失败' : '保存的发现记录读取失败')
  }
  finally {
    if (request === sequence)
      loading.value = false
  }
}
function select(model: string, checked: boolean) {
  selected.value = checked ? [...new Set([...selected.value, model])] : selected.value.filter(value => value !== model)
}
function append() {
  if (props.disabled || loading.value || stale.value || failure.value || !accepted.value.length || props.models.length + accepted.value.length > 1000)
    return
  emit('append', accepted.value)
  selected.value = []
}
</script>

<template>
  <section class="grid min-w-0 gap-3 border-t border-cp-border pt-4">
    <div class="flex flex-wrap items-center justify-between gap-3">
      <h3 class="m-0 text-cp-sm font-emphasis">
        上游模型发现
      </h3>
      <BaseButton variant="ghost" :loading="loading" :disabled="disabled" @click="load(true)">
        <template #icon>
          <Search :size="16" />
        </template>
        查询已保存渠道
      </BaseButton>
    </div>
    <p class="m-0 text-cp-xs leading-relaxed text-cp-text-secondary">
      打开窗口仅读取本地保存的成功记录。点击查询才使用已保存凭据请求上游模型列表，并保存新的成功记录，不发起生成测试。添加到草稿后仍需“保存渠道”才生效；关闭窗口丢弃草稿，不删除成功记录。
    </p>
    <p v-if="disabled" class="m-0 text-cp-xs text-cp-text-secondary">
      连接配置尚未就绪或存在未保存更改。
    </p>
    <div v-if="failure" role="alert" class="text-cp-sm text-cp-error">
      {{ failure }}{{ preview ? '；保留上次预览，暂不可添加。' : '' }}
    </div>
    <BaseButton variant="ghost" class="justify-self-start" :disabled="disabled || loading" @click="load(false)">
      重新读取成功记录
    </BaseButton>
    <p v-if="loaded && !preview && !failure" class="m-0 text-cp-xs text-cp-text-secondary">
      此渠道还没有成功发现记录，请手动查询上游。
    </p>
    <template v-if="preview">
      <p class="m-0 break-all text-cp-xs text-cp-text-secondary">
        {{ restored ? '已恢复保存记录' : '成功记录已保存' }} · 查询于 {{ new Date(preview.fetchedAt).toLocaleString() }} · 连接版本 {{ preview.connectionRevision }} · 发现序号 {{ preview.generation }}
      </p>
      <p v-if="stale" role="alert" class="m-0 text-cp-xs text-cp-warning">
        此记录属于旧连接版本，仅供查看；请重新查询当前渠道后再添加。
      </p>
      <div class="flex flex-wrap gap-4 text-cp-sm">
        <span>新增 {{ preview.added.length }}</span><span>已配置 {{ preview.unchanged.length }}</span><span>本次未发现 {{ preview.missing.length }}</span>
      </div>
      <p class="m-0 text-cp-xs text-cp-text-secondary">
        未发现的已配置模型保持不变。列表不证明 Responses 可用性，能力与价格仍待核验。
      </p>
      <div v-if="additions.length" class="grid max-h-52 gap-3 overflow-auto py-1">
        <div v-for="model in additions" :key="model" class="flex min-w-0 items-start gap-2">
          <BaseCheckbox :label="`添加 ${model}`" :model-value="selected.includes(model)" :disabled="disabled || loading || stale || !!failure" @update:model-value="select(model, $event)" />
          <span class="break-all font-mono text-cp-xs">{{ model }}</span>
        </div>
      </div>
      <p v-else class="m-0 text-cp-sm text-cp-text-secondary">
        没有可添加的新模型
      </p>
      <details v-if="preview.missing.length" class="text-cp-xs text-cp-text-secondary">
        <summary class="cursor-pointer">
          本次未发现的已配置模型
        </summary>
        <div class="mt-2 max-h-32 overflow-auto break-all font-mono">
          {{ preview.missing.join('、') }}
        </div>
      </details>
      <BaseButton v-if="additions.length" variant="ghost" class="justify-self-start" :disabled="disabled || loading || stale || !!failure || !accepted.length || models.length + accepted.length > 1000" @click="append">
        <template #icon>
          <Plus :size="16" />
        </template>
        添加 {{ accepted.length }} 项到草稿
      </BaseButton>
      <p v-if="models.length + accepted.length > 1000" role="alert" class="m-0 text-cp-xs text-cp-error">
        每个渠道最多配置 1000 个模型
      </p>
    </template>
    <ChannelDiscoveryHistory :channel="channel" :latest-generation="preview?.generation" />
  </section>
</template>
