-- Existing pools keep their previous unlimited, equal-preference behavior.
alter table account_groups
    add column source_priority integer not null default 1 check (source_priority between 1 and 65535),
    add column source_weight integer not null default 1 check (source_weight between 1 and 65535),
    add column max_concurrency bigint not null default 0 check (max_concurrency between 0 and 9007199254740991),
    add column requests_per_minute bigint not null default 0 check (requests_per_minute between 0 and 9007199254740991),
    add column quota_scope_id text check (quota_scope_id ~ '^quota_[A-Za-z0-9_-]+$' and length(quota_scope_id) <= 128);
