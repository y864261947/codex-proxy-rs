import type { RealtimeTraffic } from '@/api/modules/dashboard'
import { useDocumentVisibility, useIntervalFn } from '@vueuse/core'
import { onMounted, onScopeDispose, shallowRef, watch } from 'vue'
import { getRealtimeTraffic } from '@/api/modules/dashboard'
import { errorMessage } from '@/utils/async'

/** Lightweight polling shared by overview and monitoring; no overlapping requests. */
export function useRealtimeTraffic() {
  const snapshot = shallowRef<RealtimeTraffic | null>(null)
  const loading = shallowRef(false)
  const error = shallowRef('')
  const visibility = useDocumentVisibility()
  let disposed = false
  let controller: AbortController | undefined

  async function refresh() {
    if (disposed || loading.value || visibility.value === 'hidden')
      return
    loading.value = true
    controller = new AbortController()
    try {
      const result = await getRealtimeTraffic(controller.signal)
      if (!disposed) {
        snapshot.value = result
        error.value = ''
      }
    }
    catch (cause) {
      if (!disposed)
        error.value = errorMessage(cause, '实时指标暂时不可用')
    }
    finally {
      if (!disposed)
        loading.value = false
    }
  }

  useIntervalFn(() => void refresh(), 5000)
  onMounted(() => void refresh())
  watch(visibility, (value) => {
    if (value === 'visible')
      void refresh()
  })
  onScopeDispose(() => {
    disposed = true
    controller?.abort()
  })
  return { snapshot, loading, error, refresh }
}
