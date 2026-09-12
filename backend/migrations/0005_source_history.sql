-- 来源采用稳定历史引用，不随活配置删除或改名发生变化。
alter table model_requests
  add column source_kind text,
  add column source_ref text,
  add column source_name_snapshot text,
  add constraint model_requests_source_ck check (
    (source_kind is null and source_ref is null and source_name_snapshot is null)
    or coalesce((
      ((source_kind = 'channel' and source_ref ~ '^chan_[A-Za-z0-9_-]+$'
         and provider_account_id is null and provider_account_ref is null)
       or (source_kind = 'pool' and source_ref ~ '^grp_[0-9a-f]{32}$'))
      and length(source_ref) <= 128
      and (source_name_snapshot is null or (length(source_name_snapshot) between 1 and 128
        and btrim(source_name_snapshot) = source_name_snapshot and source_name_snapshot !~ '[[:cntrl:]]'))
    ), false)
  );

alter table model_requests drop constraint model_requests_send_state_ck;
alter table model_requests add constraint model_requests_send_state_ck check (
  upstream_send_state in ('not_sent', 'sent', 'ambiguous') and (
    (attempt_count = 0 and upstream_send_state = 'not_sent') or
    (attempt_count > 0 and provider_kind is not null and upstream_transport is not null
      and (provider_account_ref is not null or coalesce((source_kind = 'channel' and source_ref is not null), false)))
  )
);

-- 渠道独占接入分组可明确记录没有账号权限，不伪装成全账号或非空号池。
alter table model_requests drop constraint model_requests_routing_scope_ck;
alter table model_requests add constraint model_requests_routing_scope_ck check (
  routing_scope in ('legacy_provider', 'all', 'groups', 'none')
);
alter table model_requests drop constraint model_requests_routing_group_names_ck;
alter table model_requests add constraint model_requests_routing_group_names_ck check (
  jsonb_typeof(routing_group_names_snapshot) = 'array' and (
    (routing_scope in ('legacy_provider', 'all', 'none')
      and cardinality(routing_group_refs) = 0 and jsonb_array_length(routing_group_names_snapshot) = 0)
    or (routing_scope = 'groups' and cardinality(routing_group_refs) > 0
      and array_position(routing_group_refs, null) is null
      and jsonb_array_length(routing_group_names_snapshot) = cardinality(routing_group_refs))
  )
);

alter table ops_events
  add column source_kind text,
  add column source_ref text,
  add column source_name_snapshot text,
  add constraint ops_events_source_ck check (
    (source_kind is null and source_ref is null and source_name_snapshot is null)
    or coalesce((
      ((source_kind = 'channel' and source_ref ~ '^chan_[A-Za-z0-9_-]+$'
         and provider_account_id is null and provider_account_ref is null)
       or (source_kind = 'pool' and source_ref ~ '^grp_[0-9a-f]{32}$'))
      and length(source_ref) <= 128
      and (source_name_snapshot is null or (length(source_name_snapshot) between 1 and 128
        and btrim(source_name_snapshot) = source_name_snapshot and source_name_snapshot !~ '[[:cntrl:]]'))
    ), false)
  );

create index model_requests_source_idx on model_requests (source_kind, source_ref, started_at desc, id desc)
  where source_ref is not null;
create index ops_events_source_idx on ops_events (source_kind, source_ref, created_at desc, id desc)
  where source_ref is not null;
