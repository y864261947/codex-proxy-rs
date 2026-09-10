use gateway_core::policy::{
    AdmissionScopeId, ClientApiKeyId, ClientPolicy, CustomerId, CustomerPolicy, RateLimits,
};

use super::{account_scope, plaintext};

#[test]
fn customer_disable_denies_enabled_keys_and_keeps_independent_limits() {
    let customer = CustomerPolicy {
        id: CustomerId::new("cust_shared").expect("customer id"),
        enabled: false,
        limits: RateLimits {
            max_concurrency: 8,
            requests_per_minute: 60,
        },
    };
    let policy = ClientPolicy::new(
        ClientApiKeyId::new("key_child").expect("key id"),
        plaintext("sk_child_secret"),
        account_scope(),
        true,
        RateLimits {
            max_concurrency: 3,
            requests_per_minute: 30,
        },
    )
    .with_customer(Some(customer.clone()));
    assert!(policy.authorize().is_err());
    let scopes = policy.admission_scopes();
    assert_eq!(scopes.len(), 2);
    assert_eq!(scopes[0].limits.max_concurrency, 3);
    assert_eq!(scopes[1].id, AdmissionScopeId::Customer(customer.id));
    assert_eq!(scopes[1].limits.requests_per_minute, 60);
}

#[test]
fn unassigned_keys_keep_a_single_scope_and_scope_types_cannot_collide() {
    let policy = ClientPolicy::new(
        ClientApiKeyId::new("cust_same").expect("key id"),
        plaintext("sk_child_secret"),
        account_scope(),
        true,
        RateLimits::unlimited(),
    );
    assert!(policy.authorize().is_ok());
    let scopes = policy.admission_scopes();
    assert_eq!(scopes.len(), 1);
    assert_ne!(
        scopes[0].id.to_string(),
        AdmissionScopeId::Customer(CustomerId::new("cust_same").expect("customer id")).to_string()
    );
    assert!(CustomerId::new("invalid").is_err());
}
