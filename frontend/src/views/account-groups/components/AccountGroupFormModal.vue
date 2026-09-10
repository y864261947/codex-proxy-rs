<script setup lang="ts">
import type { AccountGroupFormValue } from '../composables/useAccountGroups'
import type { AccountGroup } from '@/api'
import { computed } from 'vue'

import BaseButton from '@/components/base/BaseButton.vue'
import BaseColorPicker from '@/components/base/BaseColorPicker/index.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseForm from '@/components/base/BaseForm/index.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import BaseNumberInput from '@/components/base/BaseNumberInput.vue'
import BaseTextarea from '@/components/base/BaseTextarea.vue'
import { ACCOUNT_GROUP_COLOR_PRESETS } from '../constants'

const props = defineProps<{
  group: AccountGroup | null
  saving: boolean
}>()
const emit = defineEmits<{
  save: []
}>()
const open = defineModel<boolean>({ required: true })
const form = defineModel<AccountGroupFormValue>('form', { required: true })
const title = computed(() => props.group ? '编辑分组' : '创建分组')
const description = computed(() => props.group
  ? '设置号池用途、来源优先级和共享容量。'
  : '创建后，可在账号管理中将账号加入这个分组。')
const controlsValid = computed(() => {
  const controls = form.value.sourceControls
  return [controls.priority, controls.weight].every(value => Number.isSafeInteger(value) && value >= 1 && value <= 65535)
    && [controls.maxConcurrency, controls.requestsPerMinute].every(value => Number.isSafeInteger(value) && value >= 0)
    && (!controls.quotaScopeId || (controls.quotaScopeId.length <= 128 && /^quota_[\w-]+$/.test(controls.quotaScopeId)))
})
</script>

<template>
  <BaseModal
    v-model="open"
    :title="title"
    :description="description"
    size="lg"
    :dismissible="!saving"
  >
    <BaseForm class="grid gap-5">
      <BaseFormItem label="分组名称" required>
        <BaseInput
          v-model="form.name"
          aria-label="分组名称"
          placeholder="例如：生产账号"
          :disabled="saving"
        />
      </BaseFormItem>
      <BaseFormItem label="分组颜色" required>
        <BaseColorPicker
          v-model="form.color"
          label="选择分组颜色"
          :presets="ACCOUNT_GROUP_COLOR_PRESETS"
          :disabled="saving"
        />
      </BaseFormItem>
      <BaseFormItem label="描述（可选）">
        <BaseTextarea
          v-model="form.description"
          aria-label="分组描述"
          :rows="4"
          placeholder="说明这个分组的用途..."
          :disabled="saving"
        />
      </BaseFormItem>
      <div class="grid gap-5 sm:grid-cols-2">
        <BaseFormItem label="来源优先级" description="1 最高，较大的数值为后续来源">
          <BaseNumberInput v-model="form.sourceControls.priority" label="来源优先级" :min="1" :max="65535" :disabled="saving" />
        </BaseFormItem>
        <BaseFormItem label="同级权重" description="只在相同优先级间比较">
          <BaseNumberInput v-model="form.sourceControls.weight" label="同级权重" :min="1" :max="65535" :disabled="saving" />
        </BaseFormItem>
        <BaseFormItem label="号池并发上限" description="0 为本层不限，账号限额继续生效">
          <BaseNumberInput v-model="form.sourceControls.maxConcurrency" label="号池并发上限" :min="0" :disabled="saving" />
        </BaseFormItem>
        <BaseFormItem label="号池 RPM 上限" description="所有接入分组共用此上限；0 为本层不限">
          <BaseNumberInput v-model="form.sourceControls.requestsPerMinute" label="号池 RPM 上限" :min="0" :disabled="saving" />
        </BaseFormItem>
      </div>
      <BaseFormItem label="共享配额范围（可选）" description="填写已有配额范围 ID，共享同一上游容量时使用">
        <BaseInput :model-value="form.sourceControls.quotaScopeId || ''" aria-label="共享配额范围" placeholder="quota_…" :disabled="saving" @update:model-value="form.sourceControls.quotaScopeId = $event || null" />
      </BaseFormItem>
    </BaseForm>

    <template #footer>
      <BaseButton variant="ghost" :disabled="saving" @click="open = false">
        取消
      </BaseButton>
      <BaseButton
        variant="primary"
        :loading="saving"
        :disabled="!form.name.trim() || !controlsValid"
        @click="emit('save')"
      >
        保存分组
      </BaseButton>
    </template>
  </BaseModal>
</template>
