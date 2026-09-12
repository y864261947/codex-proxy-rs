alter table upstream_channels
    add column discovery_interval_minutes integer check (discovery_interval_minutes between 5 and 1440),
    add column discovery_next_due_at timestamptz,
    add column discovery_attempt bigint check (discovery_attempt > 0),
    add column discovery_attempted_at timestamptz,
    add column discovery_completed_at timestamptz,
    add column discovery_succeeded boolean,
    add constraint channel_discovery_schedule_provider check (discovery_interval_minutes is null or provider_kind = 'openai_api'),
    add constraint channel_discovery_schedule_due check ((discovery_interval_minutes is null) = (discovery_next_due_at is null)),
    add constraint channel_discovery_schedule_attempt check ((discovery_attempt is null) = (discovery_attempted_at is null)),
    add constraint channel_discovery_schedule_completion check (
        (discovery_completed_at is null) = (discovery_succeeded is null)
        and (discovery_completed_at is null or discovery_attempted_at is not null)
    );

create index upstream_channels_discovery_due_idx
    on upstream_channels (discovery_next_due_at, id)
    where enabled and discovery_interval_minutes is not null;
