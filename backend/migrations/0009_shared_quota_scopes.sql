-- 多个来源共享实际上游项目配额，不复制渠道或账号身份。
create table upstream_quota_scopes (
    id text primary key check (id ~ '^quota_[A-Za-z0-9_-]+$' and char_length(id) <= 128),
    name text not null unique check (char_length(name) between 1 and 128 and name = btrim(name) and name !~ '[[:cntrl:]]'),
    note text check (char_length(note) <= 1024),
    enabled boolean not null default true,
    max_concurrency bigint not null default 0 check (max_concurrency between 0 and 9007199254740991),
    requests_per_minute bigint not null default 0 check (requests_per_minute between 0 and 9007199254740991),
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);

-- 早期只有引用的配置保持身份，但在管理员补全实际限额并启用前不能调用。
insert into upstream_quota_scopes (id, name, enabled)
select quota_scope_id, quota_scope_id, false from (
    select quota_scope_id from upstream_channels where quota_scope_id is not null
    union
    select quota_scope_id from account_groups where quota_scope_id is not null
) as existing;

alter table upstream_channels add constraint upstream_channels_quota_fk
    foreign key (quota_scope_id) references upstream_quota_scopes(id) on delete restrict;
alter table account_groups add constraint account_groups_quota_fk
    foreign key (quota_scope_id) references upstream_quota_scopes(id) on delete restrict;
create index upstream_channels_quota_idx on upstream_channels(quota_scope_id) where quota_scope_id is not null;
create index account_groups_quota_idx on account_groups(quota_scope_id) where quota_scope_id is not null;
