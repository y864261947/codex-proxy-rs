import type { AccountGroup } from '@/api'

import { onMounted, onScopeDispose, shallowRef } from 'vue'
import { getAccountGroups } from '@/api'
import { toast } from '@/components/base/BaseToast'
import { errorMessage } from '@/utils/async'

export function useAccountGroupCatalog(options: { immediate?: boolean } = {}) {
  const groups = shallowRef<AccountGroup[]>([])
  const loading = shallowRef(false)
  const loadError = shallowRef('')
  const loaded = shallowRef(false)
  let sequence = 0
  let disposed = false

  async function loadGroups() {
    const current = ++sequence
    loading.value = true
    loadError.value = ''
    try {
      const first = await getAccountGroups({ page: 1, pageSize: 200 })
      const items = [...first.items]
      for (let page = 2; page <= first.page.totalPages; page += 1) {
        const result = await getAccountGroups({ page, pageSize: first.page.pageSize })
        items.push(...result.items)
      }
      if (disposed || current !== sequence)
        return []
      groups.value = items
      loaded.value = true
      return items
    }
    catch (error: unknown) {
      if (disposed || current !== sequence)
        return []
      loadError.value = errorMessage(error, '账号分组加载失败')
      toast.error(loadError.value)
      return []
    }
    finally {
      if (!disposed && current === sequence)
        loading.value = false
    }
  }

  if (options.immediate !== false) {
    onMounted(() => {
      void loadGroups()
    })
  }

  onScopeDispose(() => {
    disposed = true
  })
  return {
    groups,
    loading,
    loadError,
    loaded,
    loadGroups,
  }
}
