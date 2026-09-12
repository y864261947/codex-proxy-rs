<script setup lang="ts">
import BasePageHeader from '@/components/base/BasePageHeader.vue'
import RealtimeTrafficCard from '@/components/RealtimeTrafficCard.vue'
import AccountOverviewCard from '@/views/dashboard/components/AccountOverviewCard.vue'
import DashboardHeartbeat from '@/views/dashboard/components/DashboardHeartbeat.vue'
import RequestHealthTimelineCard from '@/views/dashboard/components/RequestHealthTimelineCard.vue'
import { useDashboard } from '@/views/dashboard/composables/useDashboard'

const { accountUsage, poolSummary, capacityInfo, rotationStrategy, healthTimeline, lastRefreshedAt, trendError } = useDashboard()
</script>

<template>
  <div class="w-full">
    <BasePageHeader title="监控中心">
      <template #description>
        <span>号池与请求健康统计</span>
        <DashboardHeartbeat :updated-at="lastRefreshedAt" />
      </template>
    </BasePageHeader>
    <p v-if="trendError" role="status" class="mt-4 text-sm text-cp-error-text">
      号池与健康统计更新失败：{{ trendError }}。请以最近成功采样时间为准。
    </p>
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
