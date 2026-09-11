import request from '../request'

export type CatalogSourceKind = 'channel' | 'account_pool' | 'unpooled' | 'provider_catalog'
export type CatalogSupport = 'native' | 'emulated' | 'unsupported' | 'unknown'
export interface CatalogSource {
  kind: CatalogSourceKind
  id: string | null
  name: string | null
  connectionRevision: string | null
  priority: number | null
  weight: number | null
  maxConcurrency: number | null
  requestsPerMinute: number | null
  quotaScopeId: string | null
}
export interface CatalogModel {
  identityKey: string
  provider: string
  upstreamModel: string
  publicNames: string[]
  displayName: string | null
  description: string | null
  source: CatalogSource
  configurationReady: boolean
  operations: string[]
  features: Record<string, CatalogSupport>
  upstreamValidatesFeatures: boolean
  contextWindowTokens: number | null
  maxOutputTokens: number | null
  hidden: boolean
}
export interface ModelCatalogPage {
  items: CatalogModel[]
  total: number
  configRevision: string
  providerGenerations: Record<string, string>
  providers: string[]
}
export interface ModelCatalogQuery {
  page: number
  pageSize: number
  search?: string
  provider?: string
  sourceKind?: CatalogSourceKind
  configurationReady?: boolean
}
export function getModelCatalog(params: ModelCatalogQuery, signal?: AbortSignal) {
  return request<ModelCatalogPage>({ url: '/api/admin/model-catalog', params, signal })
}
