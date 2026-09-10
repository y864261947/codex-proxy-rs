import type { Customer } from '@/api'
import { watchDebounced } from '@vueuse/core'
import { computed, onMounted, onScopeDispose, ref } from 'vue'
import { getCustomers } from '@/api'
import { errorMessage } from '@/utils/async'

export function useCustomersQuery(immediate = true) {
  const customers = ref<Customer[]>([])
  const search = ref('')
  const page = ref(1)
  const pageSize = ref(20)
  const total = ref(0)
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
      const result = await getCustomers({ page: target, pageSize: pageSize.value, search: search.value.trim() || undefined }, controller.signal)
      if (request !== sequence || disposed)
        return
      const lastPage = Math.max(1, Math.ceil(result.total / pageSize.value))
      if (target > lastPage) {
        await load(lastPage)
        return
      }
      customers.value = result.items
      total.value = result.total
      page.value = target
    }
    catch (error: unknown) {
      if (request === sequence && !disposed)
        loadError.value = errorMessage(error, '客户加载失败')
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
    if (immediate)
      void load()
  })
  onScopeDispose(() => {
    disposed = true
    sequence++
    controller?.abort()
  })
  return { customers, search, loading, loadError, load, resize, pagination: computed(() => ({ currentPage: page.value, pageSize: pageSize.value, total: total.value })) }
}
