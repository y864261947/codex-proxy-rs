<script setup lang="ts">
import { RouterLink, RouterView } from 'vue-router'
</script>

<template>
  <div class="w-full">
    <nav aria-label="下游接入分区" class="mb-6 flex gap-2">
      <RouterLink
        v-for="tab in [{ label: '接入分组', path: '/access/groups' }, { label: 'API 密钥', path: '/access/keys' }, { label: '客户', path: '/access/customers' }]" :key="tab.path" :to="tab.path"
        class="rounded-cp px-4 py-2 text-sm font-emphasis text-cp-text-secondary hover:bg-(--cp-input-bg) focus-visible:outline-2 focus-visible:outline-cp-primary"
        active-class="bg-(--cp-input-bg) text-cp-primary-text!"
      >
        {{ tab.label }}
      </RouterLink>
    </nav>
    <RouterView />
  </div>
</template>
