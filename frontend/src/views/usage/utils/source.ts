import type { UpstreamSource } from '@/api'

export function upstreamSourceText(source?: UpstreamSource | null): string {
  if (!source)
    return '未记录来源'
  const kind = source.kind === 'channel' ? '渠道' : '号池'
  return `${kind} · ${source.name || source.id}`
}
