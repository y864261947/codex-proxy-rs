use gateway_admin::model::channels::ChannelModelPreview;
use gateway_core::{channel::ChannelRevision, identity::ChannelId};

fn preview() -> ChannelModelPreview {
    ChannelModelPreview {
        id: ChannelId::new("chan_fixture").expect("id"),
        revision: ChannelRevision::new(1).expect("revision"),
        generation: 1,
        fetched_at: chrono::Utc::now(),
        added: vec!["new".to_owned()],
        missing: vec!["gone".to_owned()],
        unchanged: vec!["old".to_owned()],
    }
}

#[test]
fn discovery_snapshot_validation_rejects_corrupt_partitions_and_unbounded_ids() {
    preview().validate().expect("valid");
    for invalid in [
        vec!["new".to_owned(), "new".to_owned()],
        vec!["old".to_owned()],
        vec!["z".to_owned(), "a".to_owned()],
        vec!["".to_owned()],
        vec![" leading".to_owned()],
        vec!["x".repeat(257)],
        vec!["line\nbreak".to_owned()],
    ] {
        let mut value = preview();
        value.added = invalid;
        assert!(value.validate().is_err());
    }
    let mut value = preview();
    value.generation = 0;
    assert!(value.validate().is_err());
    value.generation = u64::MAX;
    assert!(value.validate().is_err());
    let mut value = preview();
    value.added = (0..1000).map(|index| format!("model-{index:04}")).collect();
    assert!(value.validate().is_err());
    value.unchanged.clear();
    value.validate().expect("exact upper bound");
    value.added.clear();
    value
        .validate()
        .expect("empty upstream with configured missing model");
}
