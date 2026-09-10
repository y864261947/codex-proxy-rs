<script setup lang="ts">
import { computed, onMounted, onScopeDispose, ref } from 'vue'
import { getGlobalAdmissionSettings, updateGlobalAdmissionSettings } from '@/api'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCard from '@/components/base/BaseCard.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseForm from '@/components/base/BaseForm/index.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import { toast } from '@/components/base/BaseToast'
import { errorMessage } from '@/utils/async'

const maxConcurrency = ref('')
const requestsPerMinute = ref('')
const loaded = ref(false)
const loading = ref(false)
const saving = ref(false)
const loadError = ref('')
const saveError = ref('')
let disposed = false
let controller: AbortController | undefined
const valid = computed(() => [maxConcurrency.value, requestsPerMinute.value].every(value => value.trim() !== '' && Number.isSafeInteger(Number(value)) && Number(value) >= 0))

async function load() {
  if (loading.value || saving.value || disposed)
    return
  controller = new AbortController()
  loading.value = true
  loadError.value = ''
  try {
    const result = await getGlobalAdmissionSettings(controller.signal)
    if (disposed)
      return
    maxConcurrency.value = String(result.maxConcurrency)
    requestsPerMinute.value = String(result.requestsPerMinute)
    loaded.value = true
  }
  catch (error: unknown) {
    if (!disposed)
      loadError.value = errorMessage(error, '全站限额加载失败')
  }
  finally {
    if (!disposed)
      loading.value = false
  }
}

async function save() {
  if (!loaded.value || !valid.value || loading.value || saving.value || loadError.value)
    return
  saving.value = true
  saveError.value = ''
  try {
    await updateGlobalAdmissionSettings({ maxConcurrency: Number(maxConcurrency.value), requestsPerMinute: Number(requestsPerMinute.value) })
    if (!disposed)
      toast.success('全站限额已保存')
  }
  catch (error: unknown) {
    if (!disposed)
      saveError.value = errorMessage(error, '全站限额保存失败，请重新读取确认当前值')
  }
  finally {
    if (!disposed)
      saving.value = false
  }
}
onMounted(() => void load())
onScopeDispose(() => {
  disposed = true
  controller?.abort()
})
</script>

<template>
  <BaseCard title="全站请求限额" description="所有下游 Key 共享；与客户、接入分组、Key 和来源限额同时生效">
    <p v-if="loadError" role="alert" class="mt-0 text-cp-sm text-cp-error">
      {{ loadError }}
    </p>
    <BaseForm class="max-w-6xl sm:grid-cols-2">
      <BaseFormItem label="全站最大并发" description="已准入的业务请求数；流式请求持续占用到终态，0 表示本层不限">
        <BaseInput v-model="maxConcurrency" type="number" aria-label="全站最大并发" :disabled="!loaded || loading || saving || !!loadError" />
      </BaseFormItem>
      <BaseFormItem label="全站准入 RPM" description="最近 60 秒准入的逻辑请求数；拒绝的请求和内部探针不计入，0 表示本层不限">
        <BaseInput v-model="requestsPerMinute" type="number" aria-label="全站准入 RPM" :disabled="!loaded || loading || saving || !!loadError" />
      </BaseFormItem>
    </BaseForm>
    <p class="text-cp-sm text-cp-text-secondary">
      调低上限不终止正在执行的请求，后续请求等待占用回落后才能准入。概览的入站 RPM 包含被拒绝的请求，统计口径与此处不同。
    </p>
    <p v-if="saveError" role="alert" class="text-cp-sm text-cp-error">
      {{ saveError }}
    </p>
    <div class="flex justify-end gap-2">
      <BaseButton variant="secondary" :disabled="loading || saving" @click="load">
        {{ loadError ? '重试读取' : '重新读取' }}
      </BaseButton>
      <BaseButton variant="primary" :loading="saving" :disabled="!loaded || !valid || loading || !!loadError" @click="save">
        保存全站限额
      </BaseButton>
    </div>
  </BaseCard>
</template>
