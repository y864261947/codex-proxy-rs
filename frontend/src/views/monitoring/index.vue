<script setup lang="ts">
import BasePageHeader from '@/components/base/BasePageHeader.vue'
import RealtimeTrafficCard from '@/components/RealtimeTrafficCard.vue'
import AccountOverviewCard from '@/views/dashboard/components/AccountOverviewCard.vue'
import RequestHealthTimelineCard from '@/views/dashboard/components/RequestHealthTimelineCard.vue'
import { useDashboard } from '@/views/dashboard/composables/useDashboard'

const { accountUsage, poolSummary, capacityInfo, rotationStrategy, healthTimeline } = useDashboard()
</script>

<template>
  <div class="w-full">
    <BasePageHeader title="监控中心" description="查看当前负载、号池容量和请求健康趋势" />
    <RealtimeTrafficCard class="mt-6" />
    <AccountOverviewCard
      class="mt-6"
      :accounts="accountUsage"
      :pool="poolSummary"
      :capacity="capacityInfo"
      :rotation-strategy="rotationStrategy"
    />
    <RequestHealthTimelineCard :timeline="healthTimeline" class="mt-6" />
  </div>
</template>
