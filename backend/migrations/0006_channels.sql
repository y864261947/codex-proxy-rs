-- 渠道身份独立于账号。连接配置只供 Provider 读取，不进入普通管理投影。
create table upstream_channels (
    id text primary key check (id ~ '^chan_[A-Za-z0-9_-]+$' and char_length(id) <= 128),
    provider_kind text not null check (octet_length(provider_kind) between 1 and 64 and provider_kind !~ '^__' and provider_kind !~ '[[:cntrl:]]'),
    name text not null unique check (char_length(name) between 1 and 128 and name = btrim(name) and name !~ '[[:cntrl:]]'),
    note text check (char_length(note) <= 1024),
    enabled boolean not null default true,
    priority integer not null default 1 check (priority between 1 and 65535),
    weight integer not null default 1 check (weight between 1 and 65535),
    max_concurrency bigint not null default 0 check (max_concurrency between 0 and 9007199254740991),
    requests_per_minute bigint not null default 0 check (requests_per_minute between 0 and 9007199254740991),
    quota_scope_id text check (quota_scope_id ~ '^quota_[A-Za-z0-9_-]+$' and char_length(quota_scope_id) <= 128),
    connection_revision bigint not null default 1 check (connection_revision > 0),
    provider_config_json jsonb not null check (jsonb_typeof(provider_config_json) = 'object' and provider_config_json <> '{}'::jsonb and octet_length(provider_config_json::text) <= 131072),
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);
create index upstream_channels_provider_idx on upstream_channels (provider_kind, enabled, id);
