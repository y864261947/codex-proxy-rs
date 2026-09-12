use gateway_core::identity::{ChannelId, QuotaScopeId};

#[test]
fn channel_and_shared_quota_ids_are_distinct_bounded_stable_references() {
    let channel = ChannelId::new("chan_Example-0123").expect("channel");
    let quota = QuotaScopeId::new("quota_Example-0123").expect("shared quota");
    assert_eq!(channel.as_str(), "chan_Example-0123");
    assert_eq!(quota.to_string(), "quota_Example-0123");
    assert!(ChannelId::new(quota.as_str()).is_err());
    assert!(QuotaScopeId::new(channel.as_str()).is_err());
    for suffix in ["", " ", "中文", "a/b", "a?b", "a\nb", "a@b"] {
        assert!(ChannelId::new(format!("chan_{suffix}")).is_err());
        assert!(QuotaScopeId::new(format!("quota_{suffix}")).is_err());
    }
    assert!(ChannelId::new(format!("chan_{}", "a".repeat(123))).is_ok());
    assert!(ChannelId::new(format!("chan_{}", "a".repeat(124))).is_err());
    assert!(QuotaScopeId::new(format!("quota_{}", "a".repeat(122))).is_ok());
    assert!(QuotaScopeId::new(format!("quota_{}", "a".repeat(123))).is_err());
}
