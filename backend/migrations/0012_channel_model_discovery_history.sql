alter table channel_model_discoveries
    drop constraint channel_model_discoveries_pkey,
    add primary key (channel_id, generation);
