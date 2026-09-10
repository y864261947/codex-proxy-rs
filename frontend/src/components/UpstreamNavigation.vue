<template>
  <nav aria-label="上游管理分区" class="mt-5 flex gap-2">
    <RouterLink to="/channels" class="rounded-lg px-4 py-2 text-cp-sm transition-colors" :class="$route.name === 'channels' ? 'bg-cp-primary-container text-cp-primary-text' : 'text-cp-text-secondary hover:bg-(--cp-input-bg)'" :aria-current="$route.name === 'channels' ? 'page' : undefined">
      上游渠道
    </RouterLink>
    <RouterLink to="/channels/quotas" class="rounded-lg px-4 py-2 text-cp-sm transition-colors" :class="$route.name === 'quota-scopes' ? 'bg-cp-primary-container text-cp-primary-text' : 'text-cp-text-secondary hover:bg-(--cp-input-bg)'" :aria-current="$route.name === 'quota-scopes' ? 'page' : undefined">
      共享配额
    </RouterLink>
  </nav>
</template>
