import request from '../request'

export interface ChannelFields {
  name: string
  note: string | null
  enabled: boolean
  priority: number
  weight: number
  maxConcurrency: number
  requestsPerMinute: number
  quotaScopeId: string | null
}
export interface Channel extends ChannelFields {
  id: string
  provider: string
  connectionRevision: string
  createdAt: string
  updatedAt: string
}
export interface ChannelPage { items: Channel[], total: number, configRevision: number }
export interface ResponsesChannelConfig {
  baseUrl: string
  models: string[]
  organization: string | null
  project: string | null
  apiKey?: string
}
export interface ChannelConnection {
  id: string
  provider: string
  connectionRevision: string
  config: Omit<ResponsesChannelConfig, 'apiKey'> & { hasApiKey: boolean }
}
interface ChannelMutation { id: string, configRevision: number }
export function getChannels(params: { page: number, pageSize: number, search?: string, provider?: string }, signal?: AbortSignal) {
  return request<ChannelPage>({ url: '/api/admin/channels', params, signal })
}
export function getChannelProviders(signal?: AbortSignal) {
  return request<string[]>({ url: '/api/admin/channels/providers', signal })
}
export function getChannelConnection(id: string, signal?: AbortSignal) {
  return request<ChannelConnection>({ url: '/api/admin/channels/connection', params: { id }, signal })
}
export function createChannel(data: ChannelFields & { provider: string, config: ResponsesChannelConfig }) {
  return request<ChannelMutation>({ url: '/api/admin/channels/create', method: 'POST', data })
}
export function updateChannel(data: ChannelFields & { id: string, expectedRevision: string, config?: ResponsesChannelConfig }) {
  return request<ChannelMutation>({ url: '/api/admin/channels/update', method: 'POST', data })
}
export function deleteChannel(id: string, expectedRevision: string) {
  return request<ChannelMutation>({ url: '/api/admin/channels/delete', method: 'POST', data: { id, expectedRevision } })
}
