alter table access_groups
    add column allow_capacity_fallback boolean not null default true;

alter table access_group_pools
    add column priority_override integer check (priority_override between 1 and 65535),
    add column weight_override integer check (weight_override between 1 and 65535);

alter table access_group_channels
    add column priority_override integer check (priority_override between 1 and 65535),
    add column weight_override integer check (weight_override between 1 and 65535);
