create sequence channel_model_discovery_generation as bigint minvalue 1 no cycle;

create table channel_model_discoveries (
    channel_id text primary key references upstream_channels(id) on delete cascade,
    connection_revision bigint not null check (connection_revision > 0),
    generation bigint not null check (generation > 0),
    fetched_at timestamptz not null,
    added text[] not null,
    missing text[] not null,
    unchanged text[] not null,
    check (cardinality(added) + cardinality(unchanged) <= 1000),
    check (cardinality(missing) + cardinality(unchanged) between 1 and 1000),
    check (array_position(added, null) is null),
    check (array_position(missing, null) is null),
    check (array_position(unchanged, null) is null)
);
