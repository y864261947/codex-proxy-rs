<script setup lang="ts">
import type { ApiKey } from '@/api'
import { ref, watch } from 'vue'

import BaseCard from '@/components/base/BaseCard.vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import BaseConfirmModal from '@/components/base/BaseConfirmModal.vue'
import BasePageHeader from '@/components/base/BasePageHeader.vue'
import BaseTablePagination from '@/components/base/BaseTable/BaseTablePagination.vue'
import BaseTable from '@/components/base/BaseTable/index.vue'
import LastUsedAtCell from '@/components/LastUsedAtCell.vue'
import { useAccountGroupCatalog } from '@/composables/useAccountGroupCatalog'
import { usePageSelection } from '@/composables/usePageSelection'
import ApiKeyAccessGroupModal from './components/ApiKeyAccessGroupModal.vue'
import ApiKeyActions from './components/ApiKeyActions.vue'
import ApiKeyCreateModal from './components/ApiKeyCreateModal.vue'
import ApiKeyCustomerModal from './components/ApiKeyCustomerModal.vue'
import ApiKeyFilters from './components/ApiKeyFilters.vue'
import ApiKeyIdentityCell from './components/ApiKeyIdentityCell.vue'
import ApiKeyPrefixCell from './components/ApiKeyPrefixCell.vue'
import ApiKeyScopeCell from './components/ApiKeyScopeCell.vue'
import ApiKeyStatusBadge from './components/ApiKeyStatusBadge.vue'
import ApiKeyUseModal from './components/ApiKeyUseModal.vue'
import { useApiKeyMutations } from './composables/useApiKeyMutations'
import { useApiKeysQuery } from './composables/useApiKeysQuery'
import { useApiKeyUse } from './composables/useApiKeyUse'
import { apiKeyColumns } from './constants'

const selectedIds = ref<Set<string>>(new Set())
const customerKey = ref<ApiKey | null>(null)
const customerOpen = ref(false)
const accessKey = ref<ApiKey | null>(null)
const accessOpen = ref(false)
function assignAccessGroup(key: ApiKey) {
  accessKey.value = key
  accessOpen.value = true
}
function assignCustomer(key: ApiKey) {
  customerKey.value = key
  customerOpen.value = true
}

const {
  loading,
  apiKeys,
  loadApiKeys,
  searchQuery,
  sort,
  apiKeyPagination,
  handlePageChange,
  handlePageSizeChange,
  handleSortChange,
} = useApiKeysQuery()

const {
  groups,
  loading: loadingGroups,
  loadGroups,
} = useAccountGroupCatalog({ immediate: false })

const {
  showFormModal,
  showDeleteModal,
  showSingleDeleteModal,
  showKeyModal,
  showAllAccountsConfirm,
  createdKey,
  createdKeyName,
  editingKey,
  pendingDeleteKey,
  savingKey,
  deletingKey,
  batchDeleting,
  updatingStatusKeyIds,
  revealingKeyIds,
  form,
  openCreate,
  openEdit,
  requestSave,
  confirmAllAccountsScope,
  requestDeleteKey,
  handleDelete,
  handleBatchDelete,
  handleToggleStatus,
  copyToClipboard,
  revealPlaintextKey,
  copyApiKey,
} = useApiKeyMutations({ selectedIds, reload: loadApiKeys })

const { allSelected, indeterminate, selectedRowKeys, toggleSelection, toggleAll } = usePageSelection(
  apiKeys,
  selectedIds,
)

const {
  showUseKeyModal,
  selectedUseKey,
  openAiBaseUrl,
  importCreatedKeyToCcs,
  openUseKeyModal,
  importToCcs,
} = useApiKeyUse({
  createdKey,
  createdKeyName,
  revealPlaintextKey,
})

watch(
  showFormModal,
  open => open && void loadGroups(),
)
</script>

<template>
  <div class="flex h-full min-h-0 w-full flex-col overflow-hidden">
    <BasePageHeader
      class="h-17"
      title="API 密钥"
      description="创建和管理 API 密钥，并设置每个密钥可以使用的账号"
    />

    <BaseCard
      class="mt-5 flex h-[calc(100dvh-136px)] min-h-125 flex-col"
    >
      <template #header>
        <ApiKeyFilters
          v-model:search="searchQuery"
          :batch-deleting="batchDeleting"
          :selected-count="selectedIds.size"
          @create="openCreate"
          @delete-selected="showDeleteModal = true"
        />
      </template>

      <template #body>
        <div class="flex h-full min-h-0 flex-col">
          <BaseTable
            class="min-h-0 flex-1"
            :columns="apiKeyColumns"
            :rows="apiKeys"
            :loading="loading"
            :selected-row-keys="selectedRowKeys"
            :sort="sort"
            empty-text="暂无 API Key"
            @sort-change="handleSortChange"
          >
            <template #header-selection>
              <BaseCheckbox
                :model-value="allSelected"
                :indeterminate="indeterminate"
                label="选择当前页密钥"
                @update:model-value="toggleAll"
              />
            </template>
            <template #selection="{ row }">
              <BaseCheckbox
                :model-value="selectedIds.has(row.id)"
                label="选择密钥"
                @update:model-value="toggleSelection(row.id)"
              />
            </template>
            <template #identity="{ row }">
              <div class="grid min-w-0 gap-1">
                <ApiKeyIdentityCell :api-key="row" />
                <button type="button" class="max-w-full truncate text-left text-cp-xs text-cp-primary-text hover:underline focus-visible:outline-2 focus-visible:outline-cp-primary" :title="`设置 ${row.name} 的客户归属`" @click.stop="assignCustomer(row)">
                  客户：{{ row.customer?.name || '未分配' }}{{ row.customer && !row.customer.enabled ? '（停用）' : '' }}
                </button>
              </div>
            </template>
            <template #prefix="{ row }">
              <ApiKeyPrefixCell
                :prefix="row.prefix"
                :revealing="revealingKeyIds.has(row.id)"
                @copy="copyApiKey(row)"
              />
            </template>
            <template #scope="{ row }">
              <button type="button" class="w-full rounded-cp hover:bg-cp-fill-quaternary focus-visible:outline-2 focus-visible:outline-cp-primary" :aria-label="`设置 ${row.name} 的接入分组`" @click.stop="assignAccessGroup(row)">
                <ApiKeyScopeCell :api-key="row" />
              </button>
            </template>
            <template #enabled="{ row }">
              <ApiKeyStatusBadge :api-key="row" />
            </template>
            <template #lastUsedAt="{ row }">
              <LastUsedAtCell :value="row.lastUsedAt" />
            </template>
            <template #actions="{ row }">
              <ApiKeyActions
                :api-key="row"
                :deleting="deletingKey"
                :revealing="revealingKeyIds.has(row.id)"
                :updating-status="updatingStatusKeyIds.has(row.id)"
                @edit="openEdit"
                @delete="requestDeleteKey"
                @import-ccs="importToCcs"
                @toggle="handleToggleStatus"
                @use="openUseKeyModal"
              />
            </template>
          </BaseTable>
          <BaseTablePagination
            :pagination="apiKeyPagination"
            :loading="loading"
            @page-change="handlePageChange"
            @page-size-change="handlePageSizeChange"
          />
        </div>
      </template>
    </BaseCard>

    <ApiKeyCreateModal
      v-model="showFormModal"
      v-model:created-open="showKeyModal"
      v-model:form="form"
      :groups="groups"
      :group-loading="loadingGroups"
      :editing="Boolean(editingKey)"
      :created-key="createdKey"
      :saving="savingKey"
      @copy="copyToClipboard"
      @save="requestSave"
      @import-ccs="importCreatedKeyToCcs"
    />
    <ApiKeyCustomerModal v-model="customerOpen" :api-key="customerKey" @saved="loadApiKeys" />
    <ApiKeyAccessGroupModal v-model="accessOpen" :api-key="accessKey" @saved="loadApiKeys" />

    <ApiKeyUseModal
      v-model="showUseKeyModal"
      :api-key="selectedUseKey"
      :api-base-url="openAiBaseUrl"
      @copy="copyToClipboard"
    />

    <BaseConfirmModal
      v-model="showAllAccountsConfirm"
      title="授予全部账号权限"
      description="保存后，该密钥可以使用所有账号。"
      confirm-text="确认授予全部账号"
      :loading="savingKey"
      @confirm="confirmAllAccountsScope"
    >
      <p class="m-0">
        该密钥可以使用所有账号，包括以后新增和未分组的账号。
      </p>
    </BaseConfirmModal>

    <BaseConfirmModal
      v-model="showDeleteModal"
      title="确认删除"
      description="删除后这些 API Key 将立即失效，此操作不可撤销"
      destructive
      confirm-text="确认删除"
      :loading="batchDeleting"
      @confirm="handleBatchDelete"
    >
      <p class="m-0">
        确定删除选中的 {{ selectedIds.size }} 个 API Key 吗？
      </p>
    </BaseConfirmModal>

    <BaseConfirmModal
      v-model="showSingleDeleteModal"
      title="删除 API Key"
      description="删除后该 API Key 将立即失效，此操作不可撤销"
      destructive
      confirm-text="确认删除"
      :loading="deletingKey"
      @confirm="handleDelete"
    >
      <p class="m-0">
        确定删除 {{ pendingDeleteKey?.name || pendingDeleteKey?.prefix || '该 API Key' }} 吗？
      </p>
    </BaseConfirmModal>
  </div>
</template>
