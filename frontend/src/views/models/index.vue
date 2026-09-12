<script setup lang="ts">
import type { CatalogModel, CatalogSourceKind, CatalogSupport } from '@/api'
import { computed, ref } from 'vue'
import { useRouter } from 'vue-router'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCard from '@/components/base/BaseCard.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import BasePageHeader from '@/components/base/BasePageHeader.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import BaseTablePagination from '@/components/base/BaseTable/BaseTablePagination.vue'
import { defineTableColumns } from '@/components/base/BaseTable/columns'
import BaseTable from '@/components/base/BaseTable/index.vue'
import { useModelCatalog } from './composables/useModelCatalog'

const router = useRouter()
const { models, providers, provider, sourceKind, readiness, search, revision, providerGenerations, loadedAt, loading, loadError, load, resize, pagination } = useModelCatalog()
const selected = ref<CatalogModel | null>(null)
const selectedRevision = ref('')
const selectedGeneration = ref('')
const detailsOpen = ref(false)
const sourceLabels: Record<CatalogSourceKind, string> = { channel: 'API 渠道', account_pool: '自建号池', unpooled: '未分池账号', provider_catalog: '仅适配器目录' }
const supportLabels: Record<CatalogSupport, string> = { native: '原生支持', emulated: '网关适配', unsupported: '不支持', unknown: '未知' }
const featureLabels: Record<string, string> = { tools: '工具调用', vision: '图片输入', reasoning: '推理控制', json_schema: '结构化输出', native_continuation: '原生会话续接' }
const operationLabels: Record<string, string> = { generate: '响应生成', generate_image: '图片生成', search: '独立搜索' }
const providerOptions = computed(() => [{ label: '全部适配器', value: '' }, ...providers.value.map(value => ({ label: value, value }))])
const sourceOptions = [{ label: '全部来源', value: '' }, ...Object.entries(sourceLabels).map(([value, label]) => ({ label, value }))]
const readyOptions = [{ label: '全部配置状态', value: '' }, { label: '配置就绪', value: 'ready' }, { label: '配置未就绪', value: 'not_ready' }]
const columns = defineTableColumns<CatalogModel>([
  { key: 'identity', label: '模型 / 公开名称', kind: 'identity', size: 'xl' },
  { key: 'provider', label: '适配器', kind: 'status' },
  { key: 'source', label: '实际来源', kind: 'identity', size: 'xl' },
  { key: 'capabilities', label: '已登记操作', size: 'xl' },
  { key: 'configuration', label: '配置状态', kind: 'status' },
  { key: 'actions', label: '操作', kind: 'actions' },
])
function sourceName(model: CatalogModel) {
  return model.source.name || model.source.id || sourceLabels[model.source.kind]
}
function inspect(model: CatalogModel) {
  selected.value = model
  selectedRevision.value = revision.value
  selectedGeneration.value = providerGenerations.value[model.provider] ?? '未知'
  detailsOpen.value = true
}
function openSource(model: CatalogModel) {
  const path = model.source.kind === 'channel' ? '/channels' : model.source.kind === 'account_pool' ? '/pools/groups' : '/pools/accounts'
  void router.push(path)
}
function limit(value: number | null) {
  return value === null ? '未知' : value === 0 ? '不限制' : value.toLocaleString()
}
</script>

<template>
  <div class="flex h-full min-h-0 w-full flex-col">
    <BasePageHeader title="模型管理" description="按真实来源查看运行目录；同名模型不合并，能力缺失保持未知" />
    <div class="mt-5 flex flex-wrap items-center justify-between gap-3 rounded-cp border border-cp-border bg-cp-bg-container px-4 py-3 text-cp-sm">
      <div class="grid gap-1">
        <strong>运行模型目录</strong>
        <span class="text-cp-xs text-cp-text-secondary">只读取当前快照，不查询上游、不修改模型、不发起生成测试。</span>
      </div>
      <div class="text-right text-cp-xs text-cp-text-secondary">
        <span v-if="revision" class="font-mono">配置版本 {{ revision }}</span>
        <span v-if="loadedAt" class="ml-3">视图读取于 {{ loadedAt }}</span>
      </div>
    </div>
    <BaseCard class="mt-4 flex min-h-125 flex-col">
      <template #header>
        <div class="flex flex-wrap items-center gap-3">
          <BaseInput v-model="search" class="min-w-52 flex-1" aria-label="搜索模型目录" placeholder="搜索模型、别名或来源" />
          <BaseSelect v-model="provider" class="min-w-36" :options="providerOptions" aria-label="筛选适配器" />
          <BaseSelect v-model="sourceKind" class="min-w-36" :options="sourceOptions" aria-label="筛选来源类型" />
          <BaseSelect v-model="readiness" class="min-w-36" :options="readyOptions" aria-label="筛选配置状态" />
          <BaseButton variant="ghost" :disabled="loading" @click="load()">
            刷新视图
          </BaseButton>
        </div>
      </template>
      <template #body>
        <div v-if="loadError" role="alert" class="mb-4 flex flex-wrap items-center gap-3 text-cp-error">
          {{ loadError }}{{ loadedAt ? '；当前保留上次成功结果，可能已过时。' : '' }}
          <BaseButton variant="ghost" :disabled="loading" @click="load()">
            重试
          </BaseButton>
        </div>
        <BaseTable :columns="columns" :rows="models" row-key="identityKey" :loading="loading" empty-text="没有匹配的运行模型；请检查筛选条件，或先配置渠道模型和号池账号">
          <template #identity="{ row }">
            <div class="grid gap-1">
              <strong class="break-all font-mono">{{ row.upstreamModel }}</strong>
              <span v-if="row.displayName && row.displayName !== row.upstreamModel" class="text-cp-xs text-cp-text-secondary">{{ row.displayName }}</span>
              <span class="max-w-sm break-all text-cp-xs text-cp-text-secondary">公开名称：{{ row.publicNames.join('、') || '无（已被映射）' }}</span>
            </div>
          </template>
          <template #source="{ row }">
            <div class="grid gap-1">
              <span class="break-all">{{ sourceName(row) }}</span>
              <span class="text-cp-xs text-cp-text-secondary">{{ sourceLabels[row.source.kind] }}</span>
            </div>
          </template>
          <template #capabilities="{ row }">
            <span class="text-cp-sm">{{ row.operations.map(operation => operationLabels[operation] || operation).join(' / ') || '未知' }}</span>
          </template>
          <template #configuration="{ row }">
            <span :class="row.configurationReady ? 'text-cp-success' : 'text-cp-warning'">{{ row.configurationReady ? '配置就绪' : '配置未就绪' }}</span>
          </template>
          <template #actions="{ row }">
            <BaseButton variant="ghost" @click="inspect(row)">
              详情
            </BaseButton>
          </template>
        </BaseTable>
        <BaseTablePagination :pagination="pagination" :loading="loading" @page-change="load" @page-size-change="resize" />
      </template>
    </BaseCard>
    <p class="mt-3 text-cp-xs leading-relaxed text-cp-text-secondary">
      每行是“模型 × 来源”。配置就绪不代表上游健康、账号有额度或某个 Key 获得授权；实际调用仍受分组、模型映射和容量限制。未出现在运行快照中的停用渠道不会列出。
    </p>
    <BaseModal v-model="detailsOpen" title="模型来源详情" description="本视图不是连通性测试结果，也不包含价格或扣费结论" size="lg">
      <div v-if="selected" class="grid gap-5">
        <div class="grid gap-1">
          <strong class="break-all font-mono text-cp-lg">{{ selected.upstreamModel }}</strong>
          <p v-if="selected.description" class="m-0 text-cp-sm text-cp-text-secondary">
            {{ selected.description }}
          </p>
          <span class="text-cp-xs text-cp-text-secondary">{{ selected.provider }} · {{ sourceName(selected) }} · 配置版本 {{ selectedRevision }}</span>
        </div>
        <dl class="m-0 grid grid-cols-2 gap-x-5 gap-y-3 text-cp-sm">
          <dt class="text-cp-text-secondary">
            公开名称
          </dt><dd class="m-0 break-all">
            {{ selected.publicNames.join('、') || '无（已被映射）' }}
          </dd>
          <dt class="text-cp-text-secondary">
            来源 ID
          </dt><dd class="m-0 break-all font-mono">
            {{ selected.source.id || '无独立来源身份' }}
          </dd>
          <dt class="text-cp-text-secondary">
            连接版本
          </dt><dd class="m-0 font-mono">
            {{ selected.source.connectionRevision || '不适用' }}
          </dd>
          <dt class="text-cp-text-secondary">
            优先级 / 权重
          </dt><dd class="m-0">
            {{ selected.source.priority ?? '不适用' }} / {{ selected.source.weight ?? '不适用' }}
          </dd>
          <dt class="text-cp-text-secondary">
            来源并发 / RPM
          </dt><dd class="m-0">
            {{ limit(selected.source.maxConcurrency) }} / {{ limit(selected.source.requestsPerMinute) }}
          </dd>
          <dt class="text-cp-text-secondary">
            共享配额
          </dt><dd class="m-0 break-all font-mono">
            {{ selected.source.quotaScopeId || '未关联' }}
          </dd>
          <dt class="text-cp-text-secondary">
            上下文 / 最大输出
          </dt><dd class="m-0">
            {{ selected.contextWindowTokens?.toLocaleString() ?? '未知' }} / {{ selected.maxOutputTokens?.toLocaleString() ?? '未知' }}
          </dd>
          <dt class="text-cp-text-secondary">
            目录代数
          </dt><dd class="m-0 font-mono">
            {{ selectedGeneration }}
          </dd>
          <dt class="text-cp-text-secondary">
            客户端默认隐藏
          </dt><dd class="m-0">
            {{ selected.hidden ? '是' : '否' }}（不等于访问控制）
          </dd>
        </dl>
        <section class="grid gap-3 rounded-cp border border-cp-border p-4">
          <h3 class="m-0 text-cp-sm font-emphasis">
            能力证据
          </h3>
          <div v-for="(support, feature) in selected.features" :key="feature" class="flex items-center justify-between gap-3 text-cp-sm">
            <span>{{ featureLabels[feature] || feature }}</span><span :class="support === 'unknown' ? 'text-cp-text-secondary' : 'text-cp-text'">{{ supportLabels[support] }}</span>
          </div>
          <p class="m-0 text-cp-xs leading-relaxed text-cp-text-secondary">
            来自当前适配器目录，不代表此账号已实测。{{ selected.upstreamValidatesFeatures ? '该来源将部分参数校验交给上游；未知不能当作已支持。' : '仅展示适配器明确登记的能力。' }}
          </p>
        </section>
        <p class="m-0 text-cp-xs text-cp-text-secondary">
          这里展示来源默认调度参数；接入分组可覆盖优先级和权重，但不能覆盖来源容量。
        </p>
      </div>
      <template #footer>
        <BaseButton v-if="selected" variant="ghost" @click="openSource(selected)">
          前往来源管理
        </BaseButton><BaseButton variant="primary" @click="detailsOpen = false">
          关闭详情
        </BaseButton>
      </template>
    </BaseModal>
  </div>
</template>
