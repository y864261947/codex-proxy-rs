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
export interface ChannelModelPreview {
  id: string
  connectionRevision: string
  generation: string
  fetchedAt: string
  added: string[]
  missing: string[]
  unchanged: string[]
}
export interface ChannelDiscoveryPage {
  items: ChannelModelPreview[]
  nextBeforeGeneration: string | null
}
export interface ChannelDiscoveryReference {
  generation: string
  connectionRevision: string
  fetchedAt: string
}
export interface ChannelDiscoveryComparison {
  id: string
  base: ChannelDiscoveryReference
  target: ChannelDiscoveryReference
  sameConnectionRevision: boolean
  appeared: string[]
  disappeared: string[]
  unchanged: string[]
}
export function compareChannelModelDiscoveries(id: string, baseGeneration: string, targetGeneration: string, signal?: AbortSignal) {
  return request<ChannelDiscoveryComparison>({ url: '/api/admin/channels/model-discoveries/compare', params: { id, baseGeneration, targetGeneration }, signal })
}
export function getChannelModelDiscoveries(id: string, beforeGeneration?: string, signal?: AbortSignal) {
  return request<ChannelDiscoveryPage>({ url: '/api/admin/channels/model-discoveries', params: { id, beforeGeneration, pageSize: 10 }, signal })
}
export function getChannelModelDiscovery(id: string, signal?: AbortSignal) {
  return request<ChannelModelPreview | null>({ url: '/api/admin/channels/model-discovery', params: { id }, signal })
}
export function discoverChannelModels(id: string, expectedRevision: string, signal?: AbortSignal) {
  return request<ChannelModelPreview>({ url: '/api/admin/channels/discover-models', method: 'POST', data: { id, expectedRevision }, signal, timeout: 20000 })
}
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
