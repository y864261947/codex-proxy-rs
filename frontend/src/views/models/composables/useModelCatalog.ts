import type { CatalogModel, CatalogSourceKind } from '@/api'
import { watchDebounced } from '@vueuse/core'
import { computed, onMounted, onScopeDispose, ref } from 'vue'
import { getModelCatalog } from '@/api'
import { errorMessage } from '@/utils/async'

export function useModelCatalog() {
  const models = ref<CatalogModel[]>([])
  const providers = ref<string[]>([])
  const provider = ref('')
  const sourceKind = ref<CatalogSourceKind | ''>('')
  const readiness = ref('')
  const search = ref('')
  const page = ref(1)
  const pageSize = ref(20)
  const total = ref(0)
  const revision = ref('')
  const providerGenerations = ref<Record<string, string>>({})
  const loadedAt = ref('')
  const loading = ref(false)
  const loadError = ref('')
  let controller: AbortController | undefined
  let sequence = 0
  let disposed = false

  async function load(target = page.value) {
    if (disposed)
      return
    controller?.abort()
    controller = new AbortController()
    const request = ++sequence
    loading.value = true
    loadError.value = ''
    try {
      const result = await getModelCatalog({
        page: target,
        pageSize: pageSize.value,
        search: search.value.trim() || undefined,
        provider: provider.value || undefined,
        sourceKind: sourceKind.value || undefined,
        configurationReady: readiness.value ? readiness.value === 'ready' : undefined,
      }, controller.signal)
      if (request !== sequence || disposed)
        return
      const lastPage = Math.max(1, Math.ceil(result.total / pageSize.value))
      if (target > lastPage) {
        await load(lastPage)
        return
      }
      models.value = result.items
      providers.value = result.providers
      revision.value = result.configRevision
      providerGenerations.value = result.providerGenerations
      total.value = result.total
      page.value = target
      loadedAt.value = new Date().toLocaleTimeString()
    }
    catch (error: unknown) {
      if (request === sequence && !disposed)
        loadError.value = errorMessage(error, '模型目录加载失败')
    }
    finally {
      if (request === sequence && !disposed)
        loading.value = false
    }
  }
  function resize(size: number) {
    pageSize.value = size
    void load(1)
  }
  watchDebounced([search, provider, sourceKind, readiness], () => {
    void load(1)
  }, { debounce: 250 })
  onMounted(() => {
    void load()
  })
  onScopeDispose(() => {
    disposed = true
    sequence++
    controller?.abort()
  })
  return { models, providers, provider, sourceKind, readiness, search, revision, providerGenerations, loadedAt, loading, loadError, load, resize, pagination: computed(() => ({ currentPage: page.value, pageSize: pageSize.value, total: total.value })) }
}
