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

#[test]
fn discovery_history_bounds_page_size_and_cursor_without_losing_precision() {
    use gateway_admin::model::{PageSize, channels::ChannelDiscoveryQuery};
    let mut query = ChannelDiscoveryQuery {
        id: preview().id,
        page_size: PageSize::new(50).expect("size"),
        before_generation: Some(9007199254740993),
    };
    query.validate().expect("large cursor");
    for cursor in [0, u64::MAX] {
        query.before_generation = Some(cursor);
        assert!(query.validate().is_err());
    }
    query.before_generation = None;
    query.page_size = PageSize::new(51).expect("generic size");
    assert!(query.validate().is_err());
}

#[test]
fn discovery_comparison_uses_observed_sets_not_configuration_partitions_or_timestamps() {
    use gateway_admin::model::channels::ChannelDiscoveryPair;
    let mut base = preview();
    base.added = vec!["a".to_owned()];
    base.unchanged = vec!["b".to_owned()];
    base.missing = vec!["configured-only".to_owned()];
    let mut target = base.clone();
    target.generation = 2;
    target.revision = ChannelRevision::new(2).expect("revision");
    target.fetched_at -= chrono::Duration::seconds(1);
    target.added = vec!["b".to_owned(), "c".to_owned()];
    target.unchanged.clear();
    target.missing = vec!["a".to_owned(), "configured-only".to_owned()];
    let comparison = ChannelDiscoveryPair {
        base: base.clone(),
        target,
    }
    .compare()
    .expect("compare across versions");
    assert_eq!(comparison.appeared, ["c"]);
    assert_eq!(comparison.disappeared, ["a"]);
    assert_eq!(comparison.unchanged, ["b"]);
    let mut target = base.clone();
    target.generation = 2;
    std::mem::swap(&mut target.added, &mut target.unchanged);
    let comparison = ChannelDiscoveryPair { base, target }
        .compare()
        .expect("configuration-only change");
    assert!(comparison.appeared.is_empty());
    assert!(comparison.disappeared.is_empty());
    assert_eq!(comparison.unchanged, ["a", "b"]);
}

#[test]
fn discovery_comparison_handles_empty_sets_and_rejects_invalid_identity_or_order() {
    use gateway_admin::model::channels::ChannelDiscoveryPair;
    let base = preview();
    let mut target = base.clone();
    target.generation = 2;
    target.added.clear();
    target.unchanged.clear();
    let comparison = ChannelDiscoveryPair {
        base: base.clone(),
        target: target.clone(),
    }
    .compare()
    .expect("empty target");
    assert_eq!(comparison.disappeared, ["new", "old"]);
    assert!(comparison.appeared.is_empty());
    let mut empty_base = target.clone();
    empty_base.generation = 1;
    let comparison = ChannelDiscoveryPair {
        base: empty_base,
        target: target.clone(),
    }
    .compare()
    .expect("both empty");
    assert!(
        comparison.appeared.is_empty()
            && comparison.disappeared.is_empty()
            && comparison.unchanged.is_empty()
    );
    for generation in [0, 1, u64::MAX] {
        target.generation = generation;
        assert!(
            ChannelDiscoveryPair {
                base: base.clone(),
                target: target.clone()
            }
            .compare()
            .is_err()
        );
    }
    target.generation = 2;
    target.id = ChannelId::new("chan_other").expect("id");
    assert!(ChannelDiscoveryPair { base, target }.compare().is_err());
}
