-- 全站业务逻辑请求硬限额；0 仅表示本层不限，仍保留其他层限制。
alter table runtime_settings
  add column global_max_concurrency bigint not null default 0,
  add column global_requests_per_minute bigint not null default 0,
  add constraint runtime_settings_global_limits_ck check (
    global_max_concurrency between 0 and 9007199254740991
    and global_requests_per_minute between 0 and 9007199254740991
  );
