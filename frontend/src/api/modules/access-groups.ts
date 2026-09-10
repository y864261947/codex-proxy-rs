import request from '../request'

export interface AccessGroupRef { id: string, name: string, enabled: boolean }
export interface AccessGroupWrite {
  allowedModels: string[]
  poolGroupIds: string[]
  channelIds: string[]
  name: string
  note: string | null
  enabled: boolean
  maxConcurrency: number
  requestsPerMinute: number
}
export interface AccessGroup extends AccessGroupRef, AccessGroupWrite {
  keyCount: number
  createdAt: string
  updatedAt: string
}
export interface AccessGroupPage { items: AccessGroup[], total: number, configRevision: number }
interface AccessGroupMutation { id: string, configRevision: number }

export function getAccessGroups(params: { page: number, pageSize: number, search?: string }, signal?: AbortSignal) {
  return request<AccessGroupPage>({ url: '/api/admin/access-groups', params, signal })
}
export function createAccessGroup(data: AccessGroupWrite) {
  return request<AccessGroupMutation>({ url: '/api/admin/access-groups/create', method: 'POST', data })
}
export function updateAccessGroup(data: AccessGroupWrite & { id: string }) {
  return request<AccessGroupMutation>({ url: '/api/admin/access-groups/update', method: 'POST', data })
}
export function deleteAccessGroup(id: string) {
  return request<AccessGroupMutation>({ url: '/api/admin/access-groups/delete', method: 'POST', data: { id } })
}
export function assignKeyAccessGroup(keyId: string, accessGroupId: string | null) {
  return request<AccessGroupMutation>({ url: '/api/admin/access-groups/assign-key', method: 'POST', data: { keyId, accessGroupId } })
}
