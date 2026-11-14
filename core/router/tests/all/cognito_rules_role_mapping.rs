// SPDX-License-Identifier: CC0-1.0

//! Claim-driven role selection, end to end across the two Cognito services.
//!
//! RULE: a user pool states a user's own attributes as the claims of the identity token a
//! sign-in issues — each under the name the attribute is stored under, a custom attribute
//! under the `custom:` prefix Amazon Cognito gives it — and an identity pool whose
//! `RoleMappings` entry for that provider is of `Type` `Rules` resolves the vended role
//! from those claims: "rules are evaluated in order. The first one to match specifies the
//! role" (`RulesConfigurationType$Rules`), each rule matching the claim it names under its
//! own `MatchType` (`Equals`, `Contains`, `StartsWith` or `NotEqual`, the whole of
//! `MappingRuleMatchType`), and an exchange no rule matches falling back to what the
//! mapping's `AmbiguousRoleResolution` states.
//!
//! Only a cross-service suite can pin this: the custom attribute, the user and the sign-in
//! are `cognito-idp`'s, the role mapping and the vend are `cognito-identity`'s, the roles
//! are `iam:CreateRole`'s, and which role a vended session stands for is read from
//! `sts:GetCallerIdentity` — four services over ONE engine, as `core/router/src/lib.rs`
//! mounts them.
//!
//! The rows differ along what the rule generalises over: the `MatchType` each rule carries,
//! the order two matching rules are declared in, the claim value the signed-in user carries,
//! and whether the token carries the claim a rule names at all.
//!
//! Every call goes over the wire. The sign-in and the vend legs carry no credential scope at
//! all (their models list no auth scheme, or make it optional); the setup legs carry one, on
//! a router with request authentication off as every suite in this crate runs it. The wire
//! harness itself is the one the sibling `cognito_group_role_mapping` suite holds, which
//! drives the same four services.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};

// The wire harness is the sibling suite's: it drives the same four services over one
// router, so the dispatch helpers, the role and caller-identity helpers and the constants
// they read are reused from there rather than copied.
use crate::cognito_group_role_mapping::{
    caller_arn, claims_of, create_role, ok, session_arn, BOOT_STACK_SIZE, GOOD_PASSWORD, REGION,
};

/// The custom attribute the pool declares, as the pool stores it: a `Schema` entry named
/// `dept` is stored as `custom:dept` (`SchemaAttributeType$Name`).
const DEPARTMENT_CLAIM: &str = "custom:dept";

/// A claim name no attribute of the pool is stored under, so no token carries it.
const ABSENT_CLAIM: &str = "custom:absent";

/// One rule of a row's mapping: the claim it reads, its `MatchType`, the value it compares
/// against, and which of the two rule roles it names.
struct Rule<'a> {
    claim: &'a str,
    match_type: &'a str,
    value: &'a str,
    role: usize,
}

/// One row: the case, the user whose sign-in is exchanged, the rules the mapping declares
/// in order, and the name of the role the vended session must stand for.
struct Row<'a> {
    case: &'a str,
    username: &'a str,
    rules: Vec<Rule<'a>>,
    expected_role: &'a str,
}

#[test]
fn rules_role_mapping_selects_the_first_matching_rules_role() {
    std::thread::Builder::new()
        .name("cognito-rules-role-mapping".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");
            let idp = "AWSCognitoIdentityProviderService";
            let identity = "AWSCognitoIdentityService";

            // ---- the user pool declaring the custom attribute, and its app client ----
            let user_pool_id = ok(
                &router,
                &runtime,
                Some("cognito-idp"),
                &format!("{idp}.CreateUserPool"),
                &json!({
                    "PoolName": "rules-role-mapping",
                    "Schema": [
                        {
                            "Name": "dept",
                            "AttributeDataType": "String",
                            "Mutable": true,
                        },
                    ],
                }),
            )["UserPool"]["Id"]
                .as_str()
                .expect("CreateUserPool reports the pool's Id")
                .to_string();
            let client_id = ok(
                &router,
                &runtime,
                Some("cognito-idp"),
                &format!("{idp}.CreateUserPoolClient"),
                &json!({
                    "UserPoolId": &user_pool_id,
                    "ClientName": "rules-role-mapping-app",
                    "ExplicitAuthFlows": ["ALLOW_USER_PASSWORD_AUTH"],
                }),
            )["UserPoolClient"]["ClientId"]
                .as_str()
                .expect("CreateUserPoolClient reports the client's ClientId")
                .to_string();
            let provider_name = format!("cognito-idp.{REGION}.amazonaws.com/{user_pool_id}");
            let pool_id = ok(
                &router,
                &runtime,
                Some("cognito-identity"),
                &format!("{identity}.CreateIdentityPool"),
                &json!({
                    "IdentityPoolName": "rules-role-mapping",
                    "AllowUnauthenticatedIdentities": false,
                    "CognitoIdentityProviders": [
                        {"ProviderName": &provider_name, "ClientId": &client_id},
                    ],
                }),
            )["IdentityPoolId"]
                .as_str()
                .expect("CreateIdentityPool reports an IdentityPoolId")
                .to_string();

            // ---- the roles: one per rule, plus the pool's own two ----
            let rule_role_names = ["rules-role-a", "rules-role-b"];
            let rule_roles: Vec<String> = rule_role_names
                .iter()
                .map(|name| create_role(&router, &runtime, name, &pool_id))
                .collect();
            let authenticated = create_role(&router, &runtime, "rules-pool-auth", &pool_id);
            let unauthenticated = create_role(&router, &runtime, "rules-pool-unauth", &pool_id);

            // ---- two users, carrying different values of the mapped claim ----
            let departments = [("engineer", "engineering"), ("accountant", "finance")];
            for (username, department) in departments {
                ok(
                    &router,
                    &runtime,
                    Some("cognito-idp"),
                    &format!("{idp}.AdminCreateUser"),
                    &json!({
                        "UserPoolId": &user_pool_id,
                        "Username": username,
                        "UserAttributes": [
                            {"Name": DEPARTMENT_CLAIM, "Value": department},
                        ],
                    }),
                );
                ok(
                    &router,
                    &runtime,
                    Some("cognito-idp"),
                    &format!("{idp}.AdminSetUserPassword"),
                    &json!({
                        "UserPoolId": &user_pool_id,
                        "Username": username,
                        "Password": GOOD_PASSWORD,
                        "Permanent": true,
                    }),
                );
            }

            // The rows: which rule of the declared list, if any, the signed-in user's claim
            // satisfies decides the vended role; an exchange no rule matches falls back to
            // the pool's authenticated role, which is what this mapping's
            // `AmbiguousRoleResolution` states.
            let rows = vec![
                Row {
                    case: "two matching rules: the first declared one specifies the role",
                    username: "engineer",
                    rules: vec![
                        Rule {
                            claim: DEPARTMENT_CLAIM,
                            match_type: "Equals",
                            value: "engineering",
                            role: 0,
                        },
                        Rule {
                            claim: DEPARTMENT_CLAIM,
                            match_type: "StartsWith",
                            value: "eng",
                            role: 1,
                        },
                    ],
                    expected_role: rule_role_names[0],
                },
                Row {
                    case: "the same two rules declared in the other order",
                    username: "engineer",
                    rules: vec![
                        Rule {
                            claim: DEPARTMENT_CLAIM,
                            match_type: "StartsWith",
                            value: "eng",
                            role: 1,
                        },
                        Rule {
                            claim: DEPARTMENT_CLAIM,
                            match_type: "Equals",
                            value: "engineering",
                            role: 0,
                        },
                    ],
                    expected_role: rule_role_names[1],
                },
                Row {
                    case: "Contains matches a value inside the claim",
                    username: "engineer",
                    rules: vec![Rule {
                        claim: DEPARTMENT_CLAIM,
                        match_type: "Contains",
                        value: "gineer",
                        role: 0,
                    }],
                    expected_role: rule_role_names[0],
                },
                Row {
                    case: "Contains the claim does not carry: the fallback role",
                    username: "accountant",
                    rules: vec![Rule {
                        claim: DEPARTMENT_CLAIM,
                        match_type: "Contains",
                        value: "gineer",
                        role: 0,
                    }],
                    expected_role: "rules-pool-auth",
                },
                Row {
                    case: "StartsWith matches the claim's prefix",
                    username: "accountant",
                    rules: vec![Rule {
                        claim: DEPARTMENT_CLAIM,
                        match_type: "StartsWith",
                        value: "fin",
                        role: 1,
                    }],
                    expected_role: rule_role_names[1],
                },
                Row {
                    case: "StartsWith another claim value's prefix: the fallback role",
                    username: "engineer",
                    rules: vec![Rule {
                        claim: DEPARTMENT_CLAIM,
                        match_type: "StartsWith",
                        value: "fin",
                        role: 1,
                    }],
                    expected_role: "rules-pool-auth",
                },
                Row {
                    case: "NotEqual matches a claim carrying another value",
                    username: "accountant",
                    rules: vec![Rule {
                        claim: DEPARTMENT_CLAIM,
                        match_type: "NotEqual",
                        value: "engineering",
                        role: 0,
                    }],
                    expected_role: rule_role_names[0],
                },
                Row {
                    case: "NotEqual the value the claim carries: the fallback role",
                    username: "engineer",
                    rules: vec![Rule {
                        claim: DEPARTMENT_CLAIM,
                        match_type: "NotEqual",
                        value: "engineering",
                        role: 0,
                    }],
                    expected_role: "rules-pool-auth",
                },
                Row {
                    case: "a rule on a claim the token does not carry: the fallback role",
                    username: "engineer",
                    rules: vec![Rule {
                        claim: ABSENT_CLAIM,
                        match_type: "Equals",
                        value: "engineering",
                        role: 1,
                    }],
                    expected_role: "rules-pool-auth",
                },
            ];

            for row in &rows {
                let case = row.case;
                // The identity pool's role configuration for this row: its two pool-wide
                // roles, and a `Rules` mapping of the user-pool provider's app client
                // declaring the row's rules in order.
                let rules: Vec<Value> = row
                    .rules
                    .iter()
                    .map(|rule| {
                        json!({
                            "Claim": rule.claim,
                            "MatchType": rule.match_type,
                            "Value": rule.value,
                            "RoleARN": &rule_roles[rule.role],
                        })
                    })
                    .collect();
                ok(
                    &router,
                    &runtime,
                    Some("cognito-identity"),
                    &format!("{identity}.SetIdentityPoolRoles"),
                    &json!({
                        "IdentityPoolId": &pool_id,
                        "Roles": {
                            "authenticated": &authenticated,
                            "unauthenticated": &unauthenticated,
                        },
                        "RoleMappings": {
                            format!("{provider_name}:{client_id}"): {
                                "Type": "Rules",
                                "AmbiguousRoleResolution": "AuthenticatedRole",
                                "RulesConfiguration": {"Rules": rules},
                            },
                        },
                    }),
                );

                // ---- the sign-in: an UNSIGNED InitiateAuth, as a client makes it ----
                let signed_in = ok(
                    &router,
                    &runtime,
                    None,
                    &format!("{idp}.InitiateAuth"),
                    &json!({
                        "ClientId": &client_id,
                        "AuthFlow": "USER_PASSWORD_AUTH",
                        "AuthParameters": {
                            "USERNAME": row.username,
                            "PASSWORD": GOOD_PASSWORD,
                        },
                    }),
                );
                let id_token = signed_in["AuthenticationResult"]["IdToken"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: the sign-in must issue an IdToken: {signed_in}")
                    })
                    .to_string();

                // The claim the mapping reads is the user's stored attribute, which is what
                // makes the rules decidable at all.
                let claims = claims_of(&id_token);
                let expected_department = departments
                    .iter()
                    .find(|(username, _)| *username == row.username)
                    .map(|(_, department)| Value::String((*department).to_string()))
                    .expect("every row names one of the created users");
                let claimed_department = claims.get(DEPARTMENT_CLAIM).cloned();
                assert_eq!(
                    claimed_department,
                    Some(expected_department.clone()),
                    "case {case:?}: expected the identity token to claim {DEPARTMENT_CLAIM} \
                     {expected_department}, got {claimed_department:?} in {claims}"
                );
                assert!(
                    claims.get(ABSENT_CLAIM).is_none(),
                    "case {case:?}: no attribute is stored under {ABSENT_CLAIM}, so no claim \
                     is, got {claims}"
                );

                // ---- the exchange: an UNSIGNED GetId, then the vend ----
                let logins = json!({&provider_name: &id_token});
                let identity_id = ok(
                    &router,
                    &runtime,
                    None,
                    &format!("{identity}.GetId"),
                    &json!({"IdentityPoolId": &pool_id, "Logins": &logins}),
                )["IdentityId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: GetId reports an IdentityId"))
                    .to_string();
                let vended = ok(
                    &router,
                    &runtime,
                    None,
                    &format!("{identity}.GetCredentialsForIdentity"),
                    &json!({"IdentityId": &identity_id, "Logins": &logins}),
                );
                let access_key_id = vended["Credentials"]["AccessKeyId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                assert!(
                    !access_key_id.is_empty(),
                    "case {case:?}: the vend must carry an AccessKeyId, got {vended}"
                );
                let arn = caller_arn(&router, &runtime, case, &access_key_id);
                assert_eq!(
                    arn,
                    session_arn(row.expected_role),
                    "case {case:?}: expected a session of {}, got {arn}",
                    row.expected_role
                );
            }
        })
        .expect("spawn the cognito rules role-mapping thread")
        .join()
        .expect("a Rules role mapping must select the matching rule's role");
}
