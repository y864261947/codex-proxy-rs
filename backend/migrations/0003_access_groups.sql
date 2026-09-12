-- Access groups authorize downstream models and explicit account pools.
-- Empty permissions deny access; legacy keys remain unbound and unchanged.
create table access_groups (
    id text primary key check (id ~ '^access_[A-Za-z0-9_-]+$' and length(id) <= 128),
    name text not null unique check (length(btrim(name)) between 1 and 128),
    note text check (length(note) <= 1024),
    enabled boolean not null default true,
    max_concurrency bigint not null default 0 check (max_concurrency between 0 and 9007199254740991),
    requests_per_minute bigint not null default 0 check (requests_per_minute between 0 and 9007199254740991),
    allowed_models text[] not null default '{}' check (cardinality(allowed_models) <= 2048),
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);

create table access_group_pools (
    access_group_id text not null references access_groups(id) on delete cascade,
    account_group_id text not null references account_groups(id) on delete restrict,
    primary key (access_group_id, account_group_id)
);
create index access_group_pools_pool_idx on access_group_pools(account_group_id);

alter table client_api_keys add column access_group_id text references access_groups(id) on delete restrict;
create index client_api_keys_access_group_idx on client_api_keys(access_group_id) where access_group_id is not null;

-- History must survive reassignment and removal of current configuration.
alter table model_requests add column access_group_ref text;
create index model_requests_access_group_started_idx on model_requests(access_group_ref, started_at) where access_group_ref is not null;
