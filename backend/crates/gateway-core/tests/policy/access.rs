use std::collections::BTreeSet;

use gateway_core::policy::{
    AccessGroupId, AccessGroupPolicy, AdmissionScopeId, ClientApiKeyId, ClientPolicy, CustomerId,
    CustomerPolicy, RateLimits,
};

use super::{account_scope, plaintext};

#[test]
fn exact_public_model_permissions_and_four_independent_scopes_are_frozen() {
    let group = AccessGroupPolicy {
        id: AccessGroupId::new("access_team").expect("access group"),
        enabled: true,
        limits: RateLimits {
            max_concurrency: 10,
            requests_per_minute: 200,
        },
        allowed_models: BTreeSet::from(["published-alias".to_owned()]),
        channel_ids: std::collections::BTreeSet::new(),
        pool_group_ids: BTreeSet::new(),
    };
    let legacy = ClientPolicy::new(
        ClientApiKeyId::new("key_team").expect("key"),
        plaintext("sk_team"),
        account_scope(),
        true,
        RateLimits {
            max_concurrency: 2,
            requests_per_minute: 20,
        },
    )
    .with_customer(Some(CustomerPolicy {
        id: CustomerId::new("cust_team").expect("customer"),
        enabled: true,
        limits: RateLimits {
            max_concurrency: 4,
            requests_per_minute: 60,
        },
    }));
    assert!(legacy.allows_model("any-existing-model"));
    let policy = legacy.with_access_group(Some(group.clone()));
    assert!(policy.allows_model("published-alias"));
    assert!(!policy.allows_model("upstream-model"));
    assert!(!policy.allows_model("Published-alias"));
    let scopes = policy.admission_scopes();
    assert_eq!(
        scopes
            .iter()
            .map(|scope| scope.limits.max_concurrency)
            .collect::<Vec<_>>(),
        [2, 4, 10, 0]
    );
    assert_eq!(
        scopes[2].id,
        AdmissionScopeId::AccessGroup(group.id.clone())
    );
    let mut disabled = group.clone();
    disabled.enabled = false;
    let disabled = policy.clone().with_access_group(Some(disabled));
    assert!(!disabled.enabled());
    assert!(disabled.authorize().is_err());
    assert!(!disabled.allows_model("published-alias"));
    let mut empty = group;
    empty.allowed_models.clear();
    assert!(
        !policy
            .clone()
            .with_access_group(Some(empty))
            .allows_model("published-alias")
    );
    assert!(
        policy.allows_model("published-alias"),
        "old policy is immutable"
    );
}

#[test]
fn access_group_permissions_reject_ambiguous_or_unbounded_configuration() {
    for id in ["", "grp_team", "access_", "access_中文", "access_bad/path"] {
        assert!(AccessGroupId::new(id).is_err());
    }
    let mut group = AccessGroupPolicy {
        id: AccessGroupId::new("access_test").expect("group"),
        enabled: true,
        limits: RateLimits::unlimited(),
        allowed_models: BTreeSet::new(),
        channel_ids: std::collections::BTreeSet::new(),
        pool_group_ids: BTreeSet::new(),
    };
    assert!(group.validate().is_ok());
    for model in ["*", " model", "model ", "", "bad\nmodel"] {
        group.allowed_models = BTreeSet::from([model.to_owned()]);
        assert!(group.validate().is_err());
    }
    group.allowed_models = BTreeSet::from(["vendor/model".to_owned()]);
    assert!(group.validate().is_ok());
    group.limits.max_concurrency = 9_007_199_254_740_992;
    assert!(group.validate().is_err());
    group.limits = RateLimits::unlimited();
    group.channel_ids = (0..257)
        .map(|n| gateway_core::identity::ChannelId::new(format!("chan_{n}")).expect("channel"))
        .collect();
    assert!(group.validate().is_err());
}
