use gateway_core::channel::{ChannelRevision, ProviderChannelConfig};
use serde_json::{Map, Value};

#[test]
fn channel_config_redacts_both_keys_and_values_and_bounds_the_opaque_envelope() {
    let config = ProviderChannelConfig::new(Map::from_iter([(
        "sensitive-key-name".to_owned(),
        Value::String("sensitive-token-value".to_owned()),
    )]))
    .expect("opaque config");
    let debug = format!("{config:?}");
    assert!(!debug.contains("sensitive-key-name"));
    assert!(!debug.contains("sensitive-token-value"));
    assert_eq!(
        config.expose_to_provider()["sensitive-key-name"],
        "sensitive-token-value"
    );
    assert!(ProviderChannelConfig::new(Map::new()).is_err());
    assert!(
        ProviderChannelConfig::new(Map::from_iter([(
            "key".to_owned(),
            Value::String("x".repeat(65_536))
        )]))
        .is_err()
    );
    assert!(ChannelRevision::new(0).is_err());
    assert!(ChannelRevision::new(u64::MAX).is_err());
    assert_eq!(ChannelRevision::new(1).expect("revision").get(), 1);
}
