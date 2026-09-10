import request from '../request'

export interface CustomerRef { id: string, name: string, enabled: boolean }
export interface CustomerWrite {
  name: string
  note: string | null
  enabled: boolean
  maxConcurrency: number
  requestsPerMinute: number
}
export interface Customer extends CustomerRef, CustomerWrite {
  keyCount: number
  createdAt: string
  updatedAt: string
}
export interface CustomerPage { items: Customer[], total: number, configRevision: number }
interface CustomerMutation { id: string, configRevision: number }

export function getCustomers(params: { page: number, pageSize: number, search?: string }, signal?: AbortSignal) {
  return request<CustomerPage>({ url: '/api/admin/customers', params, signal })
}
export function createCustomer(data: CustomerWrite) {
  return request<CustomerMutation>({ url: '/api/admin/customers/create', method: 'POST', data })
}
export function updateCustomer(data: CustomerWrite & { id: string }) {
  return request<CustomerMutation>({ url: '/api/admin/customers/update', method: 'POST', data })
}
export function deleteCustomer(id: string) {
  return request<CustomerMutation>({ url: '/api/admin/customers/delete', method: 'POST', data: { id } })
}
export function assignKeyCustomer(keyId: string, customerId: string | null) {
  return request<CustomerMutation>({ url: '/api/admin/customers/assign-key', method: 'POST', data: { keyId, customerId } })
}
