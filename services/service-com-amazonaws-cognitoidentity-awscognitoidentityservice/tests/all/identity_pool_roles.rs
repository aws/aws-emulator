// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for an identity pool's ROLE configuration: `SetIdentityPoolRoles`
//! and `GetIdentityPoolRoles`, and what the stored roles mean to
//! `GetCredentialsForIdentity`'s `CustomRoleArn`.
//!
//! Every case asserts a RULE over a table of inputs rather than one recorded call:
//! `SetIdentityPoolRoles` REPLACES a stored pool's whole role configuration and
//! `GetIdentityPoolRoles` reports back exactly that; a configuration naming something
//! other than an IAM role, or a role mapping missing what the model documents as
//! required for its `Type`, is refused and changes nothing; and a pool this service does
//! not store is the modelled `ResourceNotFoundException`.
//!
//! The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds.

use serde_json::{json, Value};

use crate::identity_pool_flow::{
    cognito_trust_policy, error_code, member, non_empty, Emulator, ABSENT_POOL_ID,
};

/// An IAM role ARN of the account this emulator serves:
/// `arn:aws:iam::<account>:role/<path and name>`, the form `RolesMap`'s value and
/// `MappingRule$RoleARN` (both documented "the Role ARN",
/// `smithy_query(query="shape",shape="RolesMap,MappingRule")`) carry. The account is the
/// emulator's own (`engine::emulator_account_id`) because a role an identity pool is
/// configured with is a role the caller passes to Amazon Cognito, and a service is only
/// passed a role of the account it acts for. `pub(crate)` so a sibling suite in this test
/// target spells a role ARN the same way rather than keeping a second copy of this helper.
pub(crate) fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:role/{name}", engine::emulator_account_id())
}

/// `MappingRuleMatchType`'s values, from
/// `smithy_query(query="shape",shape="MappingRuleMatchType")`.
const MATCH_TYPES: [&str; 4] = ["Contains", "Equals", "NotEqual", "StartsWith"];

/// A `RulesConfiguration.Rules` list of `count` rules, each naming its own role and
/// cycling through every modelled match type.
fn rules(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|position| {
                json!({
                    "Claim": format!("claim-{position}"),
                    "MatchType": MATCH_TYPES[position % MATCH_TYPES.len()],
                    "Value": format!("value-{position}"),
                    "RoleARN": role_arn(&format!("mapped-{position}")),
                })
            })
            .collect(),
    )
}

/// The `SetIdentityPoolRoles` request for one configuration: `RoleMappings` is sent only
/// when the row carries some, so a row with `None` exercises the member's absence.
fn set_request(pool_id: &str, roles: &Value, role_mappings: Option<&Value>) -> Value {
    let mut request = json!({"IdentityPoolId": pool_id, "Roles": roles.clone()});
    if let Some(role_mappings) = role_mappings {
        request["RoleMappings"] = role_mappings.clone();
    }
    request
}

/// Every configuration shape the model admits, as `(case, Roles, RoleMappings)`: the rows
/// differ in how many roles they name, which kinds, how many providers they map, each
/// mapping's `Type`, and how many rules a mapping carries — up to the 25 the model's own
/// `RulesConfigurationType$Rules` doc publishes ("You can specify up to 25 rules per
/// identity provider"). The last row configures nothing, which is what makes the table
/// also a cover of REPLACEMENT: it is applied to a pool that already has roles.
fn configurations() -> Vec<(&'static str, Value, Option<Value>)> {
    vec![
        (
            "the authenticated role alone",
            json!({"authenticated": role_arn("Cognito_poolAuth_Role")}),
            None,
        ),
        (
            "the unauthenticated role alone",
            json!({"unauthenticated": role_arn("Cognito_poolUnauth_Role")}),
            None,
        ),
        (
            // The partition of the account's own ARNs, and a role under a path: the role
            // IAM holds is named by the LAST segment of the resource, so a path changes
            // nothing about which role is named.
            "both roles, one of them under a path",
            json!({
                "authenticated": role_arn("Cognito_poolAuth_Role"),
                "unauthenticated": role_arn("service-role/unauth"),
            }),
            None,
        ),
        (
            "a Token mapping beside both roles",
            json!({
                "authenticated": role_arn("Cognito_poolAuth_Role"),
                "unauthenticated": role_arn("Cognito_poolUnauth_Role"),
            }),
            Some(json!({
                "graph.facebook.com": {
                    "Type": "Token",
                    "AmbiguousRoleResolution": "AuthenticatedRole",
                },
            })),
        ),
        (
            "a Rules mapping with one rule",
            json!({"authenticated": role_arn("Cognito_poolAuth_Role")}),
            Some(json!({
                "cognito-idp.us-east-1.amazonaws.com/us-east-1_abcdefghi:client": {
                    "Type": "Rules",
                    "AmbiguousRoleResolution": "Deny",
                    "RulesConfiguration": {"Rules": rules(1)},
                },
            })),
        ),
        (
            "two providers mapped at once",
            json!({"authenticated": role_arn("Cognito_poolAuth_Role")}),
            Some(json!({
                "accounts.google.com": {
                    "Type": "Rules",
                    "AmbiguousRoleResolution": "AuthenticatedRole",
                    "RulesConfiguration": {"Rules": rules(4)},
                },
                "appleid.apple.com": {
                    "Type": "Token",
                    "AmbiguousRoleResolution": "Deny",
                },
            })),
        ),
        (
            "a mapping carrying the published maximum of 25 rules",
            json!({"authenticated": role_arn("Cognito_poolAuth_Role")}),
            Some(json!({
                "www.amazon.com": {
                    "Type": "Rules",
                    "AmbiguousRoleResolution": "Deny",
                    "RulesConfiguration": {"Rules": rules(25)},
                },
            })),
        ),
        ("no role of either kind", json!({}), None),
    ]
}

/// RULE: `SetIdentityPoolRoles` replaces a stored pool's whole role configuration, and
/// `GetIdentityPoolRoles` reports exactly that configuration back — every role and every
/// role mapping, and nothing a previous call set.
#[tokio::test]
async fn get_identity_pool_roles_reports_what_the_last_set_stored() {
    let emulator = Emulator::new();
    let pool_id = emulator.pool("roles-round-trip", true).await;

    for (case, roles, role_mappings) in configurations() {
        let answered = emulator
            .ok(
                "SetIdentityPoolRoles",
                &set_request(&pool_id, &roles, role_mappings.as_ref()),
            )
            .await;
        assert!(
            answered.is_null() || answered == json!({}),
            "case {case:?}: SetIdentityPoolRoles answers an empty output, got {answered}"
        );

        let got = emulator
            .ok("GetIdentityPoolRoles", &json!({"IdentityPoolId": &pool_id}))
            .await;
        assert_eq!(
            member(&got, "IdentityPoolId"),
            Some(json!(pool_id)),
            "case {case:?}: GetIdentityPoolRoles names the pool it was asked about, got {got}"
        );
        // A configuration naming no role of either kind leaves the pool with no role
        // configuration, which the read reports by leaving the member out.
        let expected_roles = Some(roles.clone()).filter(non_empty);
        let actual_roles = member(&got, "Roles");
        assert_eq!(
            actual_roles, expected_roles,
            "case {case:?}: expected the stored Roles {expected_roles:?}, got {got}"
        );
        match &role_mappings {
            Some(role_mappings) => assert_eq!(
                &got["RoleMappings"], role_mappings,
                "case {case:?}: expected the stored RoleMappings {role_mappings}, got {got}"
            ),
            None => assert!(
                got["RoleMappings"].is_null(),
                "case {case:?}: a configuration set without RoleMappings reports none, got {got}"
            ),
        }
    }
}

/// RULE: a stored pool nobody set roles for has NO role configuration — which is a 200
/// naming the pool and carrying neither optional member, not a not-found and not a role it
/// was never given.
#[tokio::test]
async fn a_pool_nobody_set_roles_for_reports_no_role_configuration() {
    let emulator = Emulator::new();

    for (case, allow_unauthenticated) in [
        ("a pool allowing unauthenticated identities", true),
        ("a pool forbidding them", false),
    ] {
        let pool_id = emulator.pool("roles-unset", allow_unauthenticated).await;
        let got = emulator
            .ok("GetIdentityPoolRoles", &json!({"IdentityPoolId": &pool_id}))
            .await;
        assert_eq!(
            member(&got, "IdentityPoolId"),
            Some(json!(pool_id)),
            "case {case:?}: expected the pool it was asked about, got {got}"
        );
        assert_eq!(
            member(&got, "Roles"),
            None,
            "case {case:?}: expected no Roles member, got {got}"
        );
        assert_eq!(
            member(&got, "RoleMappings"),
            None,
            "case {case:?}: expected no RoleMappings, got {got}"
        );
    }
}

/// RULE: a pool's roles belong to that pool alone — setting one pool's roles leaves every
/// other pool's configuration where it was, and deleting a pool takes its roles with it.
#[tokio::test]
async fn roles_belong_to_one_pool() {
    let emulator = Emulator::new();
    let pools = [
        emulator.pool("roles-first", true).await,
        emulator.pool("roles-second", true).await,
        emulator.pool("roles-third", false).await,
    ];

    for (index, pool_id) in pools.iter().enumerate() {
        let roles = json!({"authenticated": role_arn(&format!("pool-{index}-auth"))});
        emulator
            .ok("SetIdentityPoolRoles", &set_request(pool_id, &roles, None))
            .await;
    }
    for (index, pool_id) in pools.iter().enumerate() {
        let got = emulator
            .ok("GetIdentityPoolRoles", &json!({"IdentityPoolId": pool_id}))
            .await;
        assert_eq!(
            got["Roles"],
            json!({"authenticated": role_arn(&format!("pool-{index}-auth"))}),
            "case {index}: each pool keeps its own roles, got {got}"
        );
    }

    emulator
        .ok("DeleteIdentityPool", &json!({"IdentityPoolId": &pools[0]}))
        .await;
    let (status, body) = emulator
        .call("GetIdentityPoolRoles", &json!({"IdentityPoolId": &pools[0]}))
        .await;
    assert_eq!(
        status, 400,
        "a deleted pool's roles are gone with it, got {status} with {body}"
    );
    let got = emulator
        .ok("GetIdentityPoolRoles", &json!({"IdentityPoolId": &pools[1]}))
        .await;
    assert_eq!(
        got["Roles"],
        json!({"authenticated": role_arn("pool-1-auth")}),
        "deleting one pool leaves another pool's roles alone, got {got}"
    );
}

/// RULE: a role operation names a pool this service stores, or it is the modelled
/// `ResourceNotFoundException` — for the write as much as for the read.
#[tokio::test]
async fn roles_of_a_pool_that_is_not_stored_are_not_found() {
    let emulator = Emulator::new();
    // A pool whose id is well formed but was never created, and one whose roles were set
    // and whose pool was then deleted, are the same case: nothing is stored.
    let deleted_pool_id = emulator.pool("roles-deleted", true).await;
    emulator
        .ok(
            "SetIdentityPoolRoles",
            &set_request(
                &deleted_pool_id,
                &json!({"authenticated": role_arn("Cognito_poolAuth_Role")}),
                None,
            ),
        )
        .await;
    emulator
        .ok(
            "DeleteIdentityPool",
            &json!({"IdentityPoolId": &deleted_pool_id}),
        )
        .await;

    for pool_id in [ABSENT_POOL_ID.to_string(), deleted_pool_id] {
        for (case, operation, request) in [
            (
                "setting roles",
                "SetIdentityPoolRoles",
                set_request(
                    &pool_id,
                    &json!({"authenticated": role_arn("Cognito_poolAuth_Role")}),
                    None,
                ),
            ),
            (
                "getting roles",
                "GetIdentityPoolRoles",
                json!({"IdentityPoolId": &pool_id}),
            ),
        ] {
            let (status, body) = emulator.call(operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?} on {pool_id}: expected HTTP 400, got {status} with {body}"
            );
            assert!(
                error_code(&body).contains("ResourceNotFoundException"),
                "case {case:?} on {pool_id}: expected ResourceNotFoundException, got {body}"
            );
        }
    }
}

/// One refused configuration: the case, the `Roles` map it carries, the `RoleMappings` it
/// carries (`None` sends the member not at all) and the refusal it earns — the HTTP status
/// and the error code.
type RefusedConfiguration = (&'static str, Value, Option<Value>, (u16, &'static str));

/// RULE: a configuration naming a role Amazon Cognito may not be passed — a value that
/// names no IAM role of this account — is refused `NotAuthorizedException` (HTTP 403),
/// while one whose role MAPPING is missing what the model documents as required for its
/// `Type` is refused `InvalidParameterException` (HTTP 400); either way the pool keeps the
/// configuration it already had.
#[tokio::test]
async fn an_incomplete_or_non_role_configuration_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let pool_id = emulator.pool("roles-refusals", true).await;

    // The configuration every refusal below must leave untouched.
    let stored = json!({"authenticated": role_arn("Cognito_poolAuth_Role")});
    let stored_mappings = json!({
        "graph.facebook.com": {"Type": "Token", "AmbiguousRoleResolution": "Deny"},
    });
    emulator
        .ok(
            "SetIdentityPoolRoles",
            &set_request(&pool_id, &stored, Some(&stored_mappings)),
        )
        .await;

    /// The refusal a role a service may not be passed earns: the code of
    /// `NotAuthorizedException`, which is what a value naming no role of this account is,
    /// at the status this service answers every client error with
    /// (`crate::service::modeled_error_status`).
    const PASS_ROLE: (u16, &str) = (400, "NotAuthorizedException");
    /// The refusal a bad input parameter earns.
    const BAD_PARAMETER: (u16, &str) = (400, "InvalidParameterException");

    let refusals: Vec<RefusedConfiguration> = vec![
        // A role value that is long enough to satisfy `ARNString` (len 20..2048) but names
        // no IAM role of this account: a user, another service's resource, another
        // account's role, a role with no name at all, and a value that is no ARN. Every one
        // of them is the same answer, because the pass is decided about the value as the
        // role of an account that is not this one rather than parsed as a parameter.
        (
            "a user ARN where a role belongs",
            json!({"authenticated": format!("arn:aws:iam::{}:user/alice", engine::emulator_account_id())}),
            None,
            PASS_ROLE,
        ),
        (
            "another service's ARN",
            json!({"authenticated": "arn:aws:s3:::a-bucket-named-like-this"}),
            None,
            PASS_ROLE,
        ),
        (
            "another account's role",
            json!({"unauthenticated": "arn:aws:iam::123456789012:role/Cognito_poolUnauth_Role"}),
            None,
            PASS_ROLE,
        ),
        (
            "an account id that is not twelve digits",
            json!({"unauthenticated": "arn:aws:iam::12345:role/Cognito_poolUnauth_Role"}),
            None,
            PASS_ROLE,
        ),
        (
            "a role ARN with no role name",
            json!({"authenticated": format!("arn:aws:iam::{}:role/", engine::emulator_account_id())}),
            None,
            PASS_ROLE,
        ),
        (
            "a value that is not an ARN",
            json!({"authenticated": "not-an-arn-but-long-enough"}),
            None,
            PASS_ROLE,
        ),
        (
            "a mapped rule naming something other than a role",
            json!({"authenticated": role_arn("Cognito_poolAuth_Role")}),
            Some(json!({
                "accounts.google.com": {
                    "Type": "Rules",
                    "AmbiguousRoleResolution": "Deny",
                    "RulesConfiguration": {"Rules": [{
                        "Claim": "isAdmin",
                        "MatchType": "Equals",
                        "Value": "yes",
                        "RoleARN": format!("arn:aws:iam::{}:user/alice", engine::emulator_account_id()),
                    }]},
                },
            })),
            PASS_ROLE,
        ),
        // `RoleMapping$RulesConfiguration`: "If you specify Rules as the role mapping
        // type, RulesConfiguration is required."
        (
            "a Rules mapping with no RulesConfiguration",
            json!({"authenticated": role_arn("Cognito_poolAuth_Role")}),
            Some(json!({
                "accounts.google.com": {"Type": "Rules", "AmbiguousRoleResolution": "Deny"},
            })),
            BAD_PARAMETER,
        ),
        // `RoleMapping$AmbiguousRoleResolution`: "If you specify Token or Rules as the
        // Type, AmbiguousRoleResolution is required."
        (
            "a Token mapping with no AmbiguousRoleResolution",
            json!({"authenticated": role_arn("Cognito_poolAuth_Role")}),
            Some(json!({"accounts.google.com": {"Type": "Token"}})),
            BAD_PARAMETER,
        ),
        (
            "a Rules mapping with no AmbiguousRoleResolution",
            json!({"authenticated": role_arn("Cognito_poolAuth_Role")}),
            Some(json!({
                "accounts.google.com": {
                    "Type": "Rules",
                    "RulesConfiguration": {"Rules": rules(2)},
                },
            })),
            BAD_PARAMETER,
        ),
        // One rule past the 25 the model's `RulesConfigurationType$Rules` doc publishes.
        (
            "a mapping carrying 26 rules",
            json!({"authenticated": role_arn("Cognito_poolAuth_Role")}),
            Some(json!({
                "accounts.google.com": {
                    "Type": "Rules",
                    "AmbiguousRoleResolution": "Deny",
                    "RulesConfiguration": {"Rules": rules(26)},
                },
            })),
            BAD_PARAMETER,
        ),
    ];

    for (case, roles, role_mappings, (expected_status, expected_code)) in refusals {
        let (status, body) = emulator
            .call(
                "SetIdentityPoolRoles",
                &set_request(&pool_id, &roles, role_mappings.as_ref()),
            )
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains(expected_code),
            "case {case:?}: expected {expected_code}, got {body}"
        );

        let got = emulator
            .ok("GetIdentityPoolRoles", &json!({"IdentityPoolId": &pool_id}))
            .await;
        assert_eq!(
            got["Roles"], stored,
            "case {case:?}: a refused call leaves the stored Roles alone, got {got}"
        );
        assert_eq!(
            got["RoleMappings"], stored_mappings,
            "case {case:?}: a refused call leaves the stored RoleMappings alone, got {got}"
        );
    }
}

/// RULE: a `Roles` key is one of the two role kinds the modelled `RoleType`
/// (`^(un)?authenticated$`) admits; anything else is refused with HTTP 400 rather than
/// stored as a third role.
#[tokio::test]
async fn a_roles_key_that_is_not_a_role_kind_is_refused() {
    let emulator = Emulator::new();
    let pool_id = emulator.pool("roles-keys", true).await;

    for case in ["admin", "Authenticated", "unauthenticated-extra", ""] {
        let (status, body) = emulator
            .call(
                "SetIdentityPoolRoles",
                &json!({
                    "IdentityPoolId": &pool_id,
                    "Roles": {case: role_arn("Cognito_poolAuth_Role")},
                }),
            )
            .await;
        // Either the boundary validator refuses the modelled key pattern first, or the
        // handler refuses it with the modelled error for a bad parameter: both are HTTP
        // 400, and neither stores the key.
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        let got = emulator
            .ok("GetIdentityPoolRoles", &json!({"IdentityPoolId": &pool_id}))
            .await;
        assert_eq!(
            member(&got, "Roles"),
            None,
            "case {case:?}: a refused key is not stored, so no Roles member is reported, \
             got {got}"
        );
    }
}

/// RULE: a `CustomRoleArn` is resolved against the roles the pool has STORED — a role the
/// pool names, as one of its two roles or as a role one of its mapping rules resolves to,
/// is accepted; one the pool does not name is refused with the modelled
/// `InvalidIdentityPoolConfigurationException` (HTTP 400).
#[tokio::test]
async fn a_custom_role_arn_is_resolved_against_the_pools_stored_roles() {
    let emulator = Emulator::new();
    let pool_id = emulator.pool("roles-custom", true).await;
    // Naming a `CustomRoleArn` is something only an exchange whose provider offered several
    // roles may do, so the identity below is reached through the provider the pool holds a
    // role mapping for, and every vend presents that login.
    let logins = json!({"accounts.google.com": "a-google-access-token"});
    let identity_id = emulator.identity(&pool_id, Some(logins.clone())).await;

    // A vend is a session of the role it resolves, so every role the rows below name is
    // seeded in the IAM store of the same engine, trusting this pool's users.
    for role in [
        "Cognito_poolAuth_Role",
        "Cognito_poolUnauth_Role",
        "mapped-0",
        "mapped-1",
        "mapped-2",
    ] {
        emulator.seed_role(&role_arn(role), cognito_trust_policy(&pool_id));
    }

    emulator
        .ok(
            "SetIdentityPoolRoles",
            &set_request(
                &pool_id,
                &json!({
                    "authenticated": role_arn("Cognito_poolAuth_Role"),
                    "unauthenticated": role_arn("Cognito_poolUnauth_Role"),
                }),
                Some(&json!({
                    "accounts.google.com": {
                        "Type": "Rules",
                        "AmbiguousRoleResolution": "Deny",
                        "RulesConfiguration": {"Rules": rules(3)},
                    },
                })),
            ),
        )
        .await;

    for (case, custom_role_arn) in [
        ("the authenticated role", role_arn("Cognito_poolAuth_Role")),
        (
            "the unauthenticated role",
            role_arn("Cognito_poolUnauth_Role"),
        ),
        ("a role a mapping rule resolves to", role_arn("mapped-0")),
        ("the last mapped role", role_arn("mapped-2")),
    ] {
        let issued = emulator
            .ok(
                "GetCredentialsForIdentity",
                &json!({
                    "IdentityId": &identity_id,
                    "Logins": &logins,
                    "CustomRoleArn": &custom_role_arn,
                }),
            )
            .await;
        assert_eq!(
            issued["IdentityId"].as_str(),
            Some(identity_id.as_str()),
            "case {case:?}: expected credentials for {identity_id}, got {issued}"
        );
    }

    for (case, custom_role_arn) in [
        ("a role the pool never named", role_arn("some-other-role")),
        ("a rule role one past the mapped ones", role_arn("mapped-3")),
    ] {
        let (status, body) = emulator
            .call(
                "GetCredentialsForIdentity",
                &json!({
                    "IdentityId": &identity_id,
                    "Logins": &logins,
                    "CustomRoleArn": &custom_role_arn,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidIdentityPoolConfigurationException"),
            "case {case:?}: expected InvalidIdentityPoolConfigurationException, got {body}"
        );
    }
}
