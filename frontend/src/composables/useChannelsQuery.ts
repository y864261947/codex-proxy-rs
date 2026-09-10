import type { Channel } from '@/api'
import { watchDebounced } from '@vueuse/core'
import { computed, onMounted, onScopeDispose, ref } from 'vue'
import { getChannels } from '@/api'
import { errorMessage } from '@/utils/async'

export function useChannelsQuery() {
  const channels = ref<Channel[]>([])
  const search = ref('')
  const page = ref(1)
  const pageSize = ref(20)
  const total = ref(0)
  const loading = ref(false)
  const loaded = ref(false)
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
      const result = await getChannels({ page: target, pageSize: pageSize.value, search: search.value.trim() || undefined }, controller.signal)
      if (request !== sequence || disposed)
        return
      const lastPage = Math.max(1, Math.ceil(result.total / pageSize.value))
      if (target > lastPage) {
        await load(lastPage)
        return
      }
      channels.value = result.items
      loaded.value = true
      total.value = result.total
      page.value = target
    }
    catch (error: unknown) {
      if (request === sequence && !disposed)
        loadError.value = errorMessage(error, '渠道加载失败')
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
  watchDebounced(search, () => {
    void load(1)
  }, { debounce: 300 })
  onMounted(() => {
    void load()
  })
  onScopeDispose(() => {
    disposed = true
    sequence++
    controller?.abort()
  })
  return { channels, search, loading, loaded, loadError, load, resize, pagination: computed(() => ({ currentPage: page.value, pageSize: pageSize.value, total: total.value })) }
}
