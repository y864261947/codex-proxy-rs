import request from '../request'

export interface QuotaScopeRef { id: string, name: string, enabled: boolean }
export interface QuotaScopeWrite {
  name: string
  note: string | null
  enabled: boolean
  maxConcurrency: number
  requestsPerMinute: number
}
export interface QuotaScope extends QuotaScopeRef, QuotaScopeWrite {
  sourceCount: number
  createdAt: string
  updatedAt: string
}
export interface QuotaScopePage { items: QuotaScope[], total: number, configRevision: number }
interface QuotaScopeMutation { id: string, configRevision: number }

export function getQuotaScopes(params: { page: number, pageSize: number, search?: string }, signal?: AbortSignal) {
  return request<QuotaScopePage>({ url: '/api/admin/quota-scopes', params, signal })
}
export function createQuotaScope(data: QuotaScopeWrite) {
  return request<QuotaScopeMutation>({ url: '/api/admin/quota-scopes/create', method: 'POST', data })
}
export function updateQuotaScope(data: QuotaScopeWrite & { id: string }) {
  return request<QuotaScopeMutation>({ url: '/api/admin/quota-scopes/update', method: 'POST', data })
}
export function deleteQuotaScope(id: string) {
  return request<QuotaScopeMutation>({ url: '/api/admin/quota-scopes/delete', method: 'POST', data: { id } })
}
