import type { RouteRecordRaw } from 'vue-router'

export const routes: RouteRecordRaw[] = [
  {
    path: '/login',
    name: 'login',
    component: () => import('@/views/login/index.vue'),
  },
  {
    path: '/',
    component: () => import('@/layout/index.vue'),
    children: [
      {
        path: '',
        name: 'dashboard',
        component: () => import('@/views/dashboard/index.vue'),
      },
      {
        path: 'accounts',
        redirect: '/pools/accounts',
      },
      {
        path: 'account-groups',
        redirect: '/pools/groups',
      },
      {
        path: 'channels/quotas',
        name: 'quota-scopes',
        component: () => import('@/views/quota-scopes/index.vue'),
      },
      {
        path: 'channels',
        name: 'channels',
        component: () => import('@/views/channels/index.vue'),
      },
      {
        path: 'pools',
        component: () => import('@/views/pools/index.vue'),
        children: [
          { path: '', redirect: '/pools/accounts' },
          { path: 'accounts', name: 'accounts', component: () => import('@/views/accounts/index.vue') },
          { path: 'groups', name: 'account-groups', component: () => import('@/views/account-groups/index.vue') },
        ],
      },
      {
        path: 'models',
        name: 'models',
        component: () => import('@/views/models/index.vue'),
      },
      {
        path: 'monitoring',
        name: 'monitoring',
        component: () => import('@/views/monitoring/index.vue'),
      },
      {
        path: 'api-keys',
        redirect: '/access/keys',
      },
      {
        path: 'access',
        component: () => import('@/views/access/index.vue'),
        children: [
          { path: '', redirect: '/access/keys' },
          { path: 'keys', name: 'api-keys', component: () => import('@/views/api-keys/index.vue') },
          { path: 'customers', name: 'customers', component: () => import('@/views/customers/index.vue') },
          { path: 'groups', name: 'access-groups', component: () => import('@/views/access-groups/index.vue') },
        ],
      },
      {
        path: 'usage',
        name: 'usage',
        component: () => import('@/views/usage/index.vue'),
      },
      {
        path: 'theme',
        name: 'theme',
        component: () => import('@/views/theme/index.vue'),
      },
      {
        path: 'settings',
        name: 'settings',
        component: () => import('@/views/settings/index.vue'),
      },
      {
        path: 'settings/backup',
        name: 'settings-backup',
        component: () => import('@/views/settings/index.vue'),
      },
    ],
  },
  {
    path: '/:pathMatch(.*)*',
    redirect: '/',
  },
]
