-- Channels require explicit downstream group authorization; no backfill grants access.
create table access_group_channels (
    access_group_id text not null references access_groups(id) on delete cascade,
    channel_id text not null references upstream_channels(id) on delete restrict,
    primary key (access_group_id, channel_id)
);
create index access_group_channels_channel_idx on access_group_channels(channel_id);
