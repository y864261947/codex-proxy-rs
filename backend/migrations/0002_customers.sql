-- Customers aggregate limits across their downstream keys; they are not login users.
create table customers (
    id text primary key check (id ~ '^cust_[A-Za-z0-9_-]+$' and length(id) <= 128),
    name text not null unique check (length(btrim(name)) between 1 and 128),
    note text check (length(note) <= 1024),
    enabled boolean not null default true,
    max_concurrency bigint not null default 0 check (max_concurrency between 0 and 9007199254740991),
    requests_per_minute bigint not null default 0 check (requests_per_minute between 0 and 9007199254740991),
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);

-- Deletion cannot silently remove a customer's limits from existing keys.
alter table client_api_keys add column customer_id text references customers(id) on delete restrict;
create index client_api_keys_customer_idx on client_api_keys(customer_id) where customer_id is not null;

-- Frozen at admission. Deliberately independent of the current key/customer rows.
alter table model_requests add column customer_ref text;
create index model_requests_customer_started_idx on model_requests(customer_ref, started_at) where customer_ref is not null;
