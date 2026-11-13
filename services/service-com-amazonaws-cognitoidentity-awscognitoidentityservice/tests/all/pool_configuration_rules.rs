// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the rules an identity pool's own CONFIGURATION has to satisfy, and
//! for the two writes of a pool being serialized.
//!
//! Every case asserts a RULE over a table of inputs rather than one recorded call:
//!
//! * a configuration STRING is admitted only if it satisfies the `@pattern` its shape
//!   declares READ AS AWS READS IT — the `\w` class of a modelled expression stands for
//!   ASCII letters, digits and the underscore alone — so a value carrying a letter outside
//!   ASCII is refused ahead of the handler with HTTP 400 `ValidationException`, in the
//!   clause that names the member, the value and the expression;
//! * a TAG is admitted only if its key and its value are spelled with the characters AWS's
//!   tagging rules publish (letters, numbers, spaces and `_ . : / = + - @`), so a value
//!   carrying an emoji is refused `InvalidParameterException` while letters of any script
//!   are stored byte for byte;
//! * an explicitly EMPTY `SupportedLoginProviders` map configures no login provider, so the
//!   pool is reported with the member absent rather than with an empty map;
//! * a configuration naming more Amazon Cognito user pool providers than the account's
//!   published quota admits is refused `LimitExceededException`;
//! * a role the pool is configured with is a role the caller PASSES to Amazon Cognito, so a
//!   value naming no IAM role of this account is refused `NotAuthorizedException`;
//! * a pool is reconfigured as a whole, so a write that meets another write of the same pool
//!   already in flight is refused `ConcurrentModificationException` and stores nothing.
//!
//! Each rule is asserted on EVERY operation it holds for — `CreateIdentityPool`,
//! `UpdateIdentityPool` and (for the tag rule) `TagResource` — rather than on the one the
//! issue cited. The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds.

use serde_json::{json, Map, Value};
use service_com_amazonaws_cognitoidentity_awscognitoidentityservice::state::identity_pool_arn;
use service_com_amazonaws_cognitoidentity_awscognitoidentityservice::state::pool_configuration::{
    INVALID_TAG_CHARACTERS_MESSAGE, MAX_USER_POOL_PROVIDERS_PER_IDENTITY_POOL,
};

use crate::identity_pool_flow::{error_code, member, Emulator};

/// The status this service answers every client error with
/// (`crate::service::modeled_error_status`): the error class travels in the body and in the
/// error-type header rather than in the status.
const CLIENT_ERROR_STATUS: u16 = 400;

/// The message an error response carries.
fn error_message(body: &Value) -> String {
    body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("an error response carries a message: {body}"))
        .to_string()
}

/// A `CreateIdentityPool` request carrying `extras` beside the two required members.
fn create_request(name: &str, extras: &Value) -> Value {
    let mut request = Map::new();
    request.insert("IdentityPoolName".to_string(), json!(name));
    request.insert("AllowUnauthenticatedIdentities".to_string(), json!(false));
    for (key, value) in extras.as_object().expect("an extras object") {
        request.insert(key.clone(), value.clone());
    }
    Value::Object(request)
}

/// An `UpdateIdentityPool` request for `identity_pool_id`, carrying `extras` beside the
/// members an update always states.
fn update_request(identity_pool_id: &str, name: &str, extras: &Value) -> Value {
    let mut request = create_request(name, extras);
    request
        .as_object_mut()
        .expect("the request is an object")
        .insert("IdentityPoolId".to_string(), json!(identity_pool_id));
    request
}

/// A `CognitoIdentityProviders` list naming `count` Amazon Cognito user pools, each its own
/// pool with its own app client.
fn user_pool_providers(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|position| {
                json!({
                    "ProviderName": format!(
                        "cognito-idp.us-east-1.amazonaws.com/us-east-1_pool{position:04}"
                    ),
                    "ClientId": format!("client{position:04}"),
                })
            })
            .collect(),
    )
}

/// RULE: a pool-configuration string is admitted only if it satisfies the `@pattern` its
/// shape declares as AWS reads that expression — `\w` standing for ASCII letters, digits
/// and the underscore — so a value carrying anything else is refused ahead of the handler
/// with HTTP 400 `ValidationException` and nothing is stored.
///
/// The rows span the two configuration members that carry such a pattern
/// (`IdentityPoolName`, `^[\w\s+=,.@-]+$`, and `DeveloperProviderName`, `^[\w._-]+$`, from
/// `smithy_query(query="shape",shape="IdentityPoolName,DeveloperProviderName")`) and the
/// ways a value can leave the class: an accented letter, a letter of another script, an
/// emoji, and a punctuation mark the class does not name — beside the values each pattern
/// admits, including every character class it does name.
#[tokio::test]
async fn a_configuration_string_outside_its_modelled_pattern_is_refused() {
    let emulator = Emulator::new();

    // (case, the pool name, the DeveloperProviderName the row sends, admitted?)
    let rows: &[(&str, &str, Option<&str>, bool)] = &[
        ("ASCII letters alone", "plain-name", None, true),
        (
            "every character class the name pattern admits",
            "pool +=,.@-_name 1",
            None,
            true,
        ),
        ("a name of one character", "a", None, true),
        ("an accented letter in the name", "pool Café", None, false),
        ("a letter of another script in the name", "团队 pool", None, false),
        ("an emoji in the name", "pool \u{1F680}", None, false),
        (
            "a punctuation mark the name pattern does not admit",
            "pool/name",
            None,
            false,
        ),
        (
            "every character class the developer-provider pattern admits",
            "dev-provider-pool",
            Some("my.dev_provider-1"),
            true,
        ),
        (
            "an accented letter in the developer provider name",
            "dev-provider-accented",
            Some("dev.café"),
            false,
        ),
        (
            "a space in the developer provider name",
            "dev-provider-spaced",
            Some("dev provider"),
            false,
        ),
    ];

    for (case, name, developer_provider_name, admitted) in rows {
        let mut extras = Map::new();
        if let Some(developer_provider_name) = developer_provider_name {
            extras.insert(
                "DeveloperProviderName".to_string(),
                json!(developer_provider_name),
            );
        }
        let extras = Value::Object(extras);
        let (status, body) = emulator
            .call("CreateIdentityPool", &create_request(name, &extras))
            .await;

        if *admitted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the configuration to be stored, got {status} with \
                 {body}"
            );
            // And what was stored is what the readers report, byte for byte.
            let identity_pool_id = body["IdentityPoolId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a created pool carries an id: {body}"));
            let described = emulator
                .ok(
                    "DescribeIdentityPool",
                    &json!({"IdentityPoolId": identity_pool_id}),
                )
                .await;
            assert_eq!(
                described["IdentityPoolName"],
                json!(name),
                "case {case:?}: expected the name as sent, got {described}"
            );
            continue;
        }

        assert_eq!(
            (status, error_code(&body).as_str()),
            (CLIENT_ERROR_STATUS, "ValidationException"),
            "case {case:?}: expected the constraint refusal, got {status} with {body}"
        );
        // A refused create mints nothing: no pool of the account answers to that name.
        let listed = emulator.ok("ListIdentityPools", &json!({"MaxResults": 60})).await;
        let named = listed["IdentityPools"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: a listing carries IdentityPools: {listed}"))
            .iter()
            .any(|pool| pool["IdentityPoolName"] == json!(name));
        assert!(
            !named,
            "case {case:?}: a refused create must store no pool, got {listed}"
        );
    }
}

/// RULE: the same pattern rule holds for an UPDATE, which writes the same members — so a
/// refused update leaves the stored configuration exactly as it was.
#[tokio::test]
async fn an_update_outside_a_modelled_pattern_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let identity_pool_id = emulator.pool("update-pattern-pool", false).await;
    let before = emulator
        .ok(
            "DescribeIdentityPool",
            &json!({"IdentityPoolId": &identity_pool_id}),
        )
        .await;

    // (case, the name the update sends, the developer provider name it sends)
    let rows: &[(&str, &str, Option<&str>)] = &[
        ("an accented letter in the name", "renamed Café", None),
        ("an emoji in the name", "renamed \u{1F680}", None),
        (
            "an accented letter in the developer provider name",
            "renamed",
            Some("dev.café"),
        ),
    ];

    for (case, name, developer_provider_name) in rows {
        let mut extras = Map::new();
        if let Some(developer_provider_name) = developer_provider_name {
            extras.insert(
                "DeveloperProviderName".to_string(),
                json!(developer_provider_name),
            );
        }
        let (status, body) = emulator
            .call(
                "UpdateIdentityPool",
                &update_request(&identity_pool_id, name, &Value::Object(extras)),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (CLIENT_ERROR_STATUS, "ValidationException"),
            "case {case:?}: expected the constraint refusal, got {status} with {body}"
        );
        let after = emulator
            .ok(
                "DescribeIdentityPool",
                &json!({"IdentityPoolId": &identity_pool_id}),
            )
            .await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update must leave the pool as it was, got {after}"
        );
    }
}

/// RULE: the clause a constraint refusal carries states how many of the request's values
/// failed, then the member, the value the request gave it and the expression that value had
/// to match — the expression as the EXPRESSION a value must satisfy, so without the anchors
/// that make it a whole-value match.
#[tokio::test]
async fn a_pattern_refusal_names_the_member_the_value_and_the_expression() {
    let emulator = Emulator::new();

    // (case, the request, the message it is refused with)
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "a pool name outside its pattern",
            create_request("pool Café", &json!({})),
            "1 validation error detected: Value 'pool Café' at 'identityPoolName' failed to \
             satisfy constraint: Member must satisfy regular expression pattern: \
             [\\w\\s+=,.@-]+"
                .to_string(),
        ),
        (
            "a developer provider name outside its pattern",
            create_request("named-pool", &json!({"DeveloperProviderName": "dev provider"})),
            "1 validation error detected: Value 'dev provider' at 'developerProviderName' \
             failed to satisfy constraint: Member must satisfy regular expression pattern: \
             [\\w._-]+"
                .to_string(),
        ),
    ];

    for (case, request, expected) in rows {
        let (status, body) = emulator.call("CreateIdentityPool", &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (CLIENT_ERROR_STATUS, "ValidationException"),
            "case {case:?}: expected the constraint refusal, got {status} with {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}

/// RULE: a tag is admitted on the CHARACTERS it is spelled with — letters of any script,
/// numbers, spaces and `_ . : / = + - @`, the set AWS's tagging rules publish — in its key
/// as in its value; an entry carrying anything else is refused HTTP 400
/// `InvalidParameterException` in the sentence that names that set, and nothing is stored.
///
/// Asserted on every operation that writes a pool's tags: the two that carry
/// `IdentityPoolTags` and the one that carries `Tags`.
#[tokio::test]
async fn a_tag_outside_the_published_character_set_is_refused_on_every_tag_write() {
    let emulator = Emulator::new();

    // (case, the tag key, the tag value, admitted?)
    let rows: &[(&str, &str, &str, bool)] = &[
        ("plain ASCII", "Environment", "Production", true),
        ("every published punctuation mark", "key_.:/=+-@", "value_.:/=+-@", true),
        ("letters of another script", "Équipe", "équipe identité", true),
        ("an empty value", "Unset", "", true),
        ("an emoji in the value", "Environment", "prod \u{1F680}", false),
        ("an emoji in the key", "env \u{1F680}", "prod", false),
        ("a comma in the value", "Environment", "prod,test", false),
        ("an apostrophe in the value", "Description", "pool d'identité", false),
    ];

    for (case, key, value, admitted) in rows {
        let tags = json!({*key: *value});

        // 1. The tag set a pool is CREATED with.
        let (status, created) = emulator
            .call(
                "CreateIdentityPool",
                &create_request("tag-charset-pool", &json!({"IdentityPoolTags": tags.clone()})),
            )
            .await;
        if *admitted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected CreateIdentityPool to store the tag, got {status} \
                 with {created}"
            );
        } else {
            assert_eq!(
                (status, error_code(&created).as_str()),
                (CLIENT_ERROR_STATUS, "InvalidParameterException"),
                "case {case:?}: expected CreateIdentityPool to refuse the tag, got {status} \
                 with {created}"
            );
            assert_eq!(
                error_message(&created),
                INVALID_TAG_CHARACTERS_MESSAGE,
                "case {case:?}: expected the published character set to be named, got {created}"
            );
        }

        // Every later write is made against a pool that does carry a tag set.
        let identity_pool_id = emulator.pool("tag-charset-target", false).await;
        let arn = identity_pool_arn(&identity_pool_id);

        // 2. The tag set an UPDATE states.
        let (status, updated) = emulator
            .call(
                "UpdateIdentityPool",
                &update_request(
                    &identity_pool_id,
                    "tag-charset-target",
                    &json!({"IdentityPoolTags": tags.clone()}),
                ),
            )
            .await;
        // 3. The tag set TagResource assigns.
        let (tagged_status, tagged) = emulator
            .call("TagResource", &json!({"ResourceArn": &arn, "Tags": tags.clone()}))
            .await;

        if *admitted {
            for (operation, status, body) in [
                ("UpdateIdentityPool", status, &updated),
                ("TagResource", tagged_status, &tagged),
            ] {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected {operation} to store the tag, got {status} with \
                     {body}"
                );
            }
            // And the stored value is the one that was sent, byte for byte.
            let listed = emulator
                .ok("ListTagsForResource", &json!({"ResourceArn": &arn}))
                .await;
            assert_eq!(
                listed["Tags"][*key],
                json!(value),
                "case {case:?}: expected the tag value as sent, got {listed}"
            );
            continue;
        }

        for (operation, status, body) in [
            ("UpdateIdentityPool", status, &updated),
            ("TagResource", tagged_status, &tagged),
        ] {
            assert_eq!(
                (status, error_code(body).as_str()),
                (CLIENT_ERROR_STATUS, "InvalidParameterException"),
                "case {case:?}: expected {operation} to refuse the tag, got {status} with {body}"
            );
            assert_eq!(
                error_message(body),
                INVALID_TAG_CHARACTERS_MESSAGE,
                "case {case:?}: expected {operation} to name the published character set, got \
                 {body}"
            );
        }
        // Nothing of a refused write is stored.
        let listed = emulator
            .ok("ListTagsForResource", &json!({"ResourceArn": &arn}))
            .await;
        assert_eq!(
            listed["Tags"],
            json!({}),
            "case {case:?}: a refused tag write must store nothing, got {listed}"
        );
    }
}

/// RULE: an explicitly empty `SupportedLoginProviders` map configures no login provider, and
/// a pool configured with none is answered with the member ABSENT — the same answer as for
/// a request that never carried the member — so "configured but empty" and "not configured"
/// are one state on the wire. A map with entries is reported as it was sent.
///
/// Asserted on every operation that answers with a pool: the create, the update and the
/// read.
#[tokio::test]
async fn an_empty_supported_login_providers_map_is_reported_as_an_absent_member() {
    let emulator = Emulator::new();

    // (case, the map the request sends, the entries the pool must report)
    let rows: &[(&str, Option<Value>, Option<Value>)] = &[
        ("no member at all", None, None),
        ("an explicitly empty map", Some(json!({})), None),
        (
            "one provider",
            Some(json!({"graph.facebook.com": "1234567890"})),
            Some(json!({"graph.facebook.com": "1234567890"})),
        ),
        (
            "two providers",
            Some(json!({
                "graph.facebook.com": "1234567890",
                "accounts.google.com": "client.apps.googleusercontent.com",
            })),
            Some(json!({
                "graph.facebook.com": "1234567890",
                "accounts.google.com": "client.apps.googleusercontent.com",
            })),
        ),
    ];

    for (case, sent, expected) in rows {
        let mut extras = Map::new();
        if let Some(sent) = sent {
            extras.insert("SupportedLoginProviders".to_string(), sent.clone());
        }
        let extras = Value::Object(extras);

        let created = emulator
            .ok("CreateIdentityPool", &create_request("providers-pool", &extras))
            .await;
        let identity_pool_id = created["IdentityPoolId"]
            .as_str()
            .expect("a created pool carries an id")
            .to_string();
        let described = emulator
            .ok(
                "DescribeIdentityPool",
                &json!({"IdentityPoolId": &identity_pool_id}),
            )
            .await;
        let updated = emulator
            .ok(
                "UpdateIdentityPool",
                &update_request(&identity_pool_id, "providers-pool", &extras),
            )
            .await;

        for (operation, answer) in [
            ("CreateIdentityPool", &created),
            ("DescribeIdentityPool", &described),
            ("UpdateIdentityPool", &updated),
        ] {
            let reported = member(answer, "SupportedLoginProviders");
            assert_eq!(
                reported,
                expected.clone(),
                "case {case:?}: expected {operation} to report {expected:?}, got {reported:?}"
            );
        }
    }
}

/// RULE: an identity pool may be configured with as many Amazon Cognito user pool providers
/// as the account's published quota admits ("Amazon Cognito user pool providers per
/// identity pool", default 50) and no more; one past it is refused HTTP 400
/// `LimitExceededException` and nothing is stored.
///
/// The rows sit on the bound and one past it, on the create and on the update alike.
#[tokio::test]
async fn a_configuration_past_the_user_pool_provider_quota_is_refused() {
    let emulator = Emulator::new();
    let quota = MAX_USER_POOL_PROVIDERS_PER_IDENTITY_POOL;

    // (case, how many providers the configuration names, admitted?)
    let rows: Vec<(String, usize, bool)> = vec![
        ("one provider".to_string(), 1, true),
        (format!("the published quota of {quota} providers"), quota, true),
        (format!("one past the quota, {} providers", quota + 1), quota + 1, false),
    ];

    for (case, count, admitted) in rows {
        let extras = json!({"CognitoIdentityProviders": user_pool_providers(count)});
        let (status, created) = emulator
            .call("CreateIdentityPool", &create_request("quota-pool", &extras))
            .await;

        if admitted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the configuration to be stored, got {status} with \
                 {created}"
            );
            let configured = created["CognitoIdentityProviders"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: the providers come back: {created}"))
                .len();
            assert_eq!(
                configured, count,
                "case {case:?}: expected {count} providers stored, got {configured}"
            );
            continue;
        }

        assert_eq!(
            (status, error_code(&created).as_str()),
            (CLIENT_ERROR_STATUS, "LimitExceededException"),
            "case {case:?}: expected the quota refusal, got {status} with {created}"
        );

        // The same rule on an UPDATE of a pool that is inside the quota: the update is
        // refused and the stored configuration is untouched.
        let identity_pool_id = emulator.pool("quota-update-pool", false).await;
        emulator
            .ok(
                "UpdateIdentityPool",
                &update_request(
                    &identity_pool_id,
                    "quota-update-pool",
                    &json!({"CognitoIdentityProviders": user_pool_providers(1)}),
                ),
            )
            .await;
        let (status, refused) = emulator
            .call(
                "UpdateIdentityPool",
                &update_request(&identity_pool_id, "quota-update-pool", &extras),
            )
            .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (CLIENT_ERROR_STATUS, "LimitExceededException"),
            "case {case:?}: expected the quota refusal from the update, got {status} with \
             {refused}"
        );
        let described = emulator
            .ok(
                "DescribeIdentityPool",
                &json!({"IdentityPoolId": &identity_pool_id}),
            )
            .await;
        let still = described["CognitoIdentityProviders"]
            .as_array()
            .map_or(0, Vec::len);
        assert_eq!(
            still, 1,
            "case {case:?}: a refused update must leave the stored providers alone, got \
             {described}"
        );
    }
}

/// RULE: a role an identity pool is configured with is a role the caller PASSES to Amazon
/// Cognito, so it has to name an IAM role of the caller's own account; a value that names
/// another account's role, or no role at all, is refused `NotAuthorizedException` in the one
/// sentence that rule is worded with, and the pool keeps the configuration it had.
///
/// The rows span both places a configuration names a role — the `Roles` map and a mapping
/// rule's `RoleARN` — and the ways a value can fail to name one of this account's roles.
#[tokio::test]
async fn a_role_the_account_does_not_own_is_refused_as_a_pass_role() {
    let emulator = Emulator::new();
    let identity_pool_id = emulator.pool("pass-role-pool", false).await;
    let own = |name: &str| format!("arn:aws:iam::{}:role/{name}", engine::emulator_account_id());

    // (case, the role value, admitted?)
    let rows: Vec<(&str, String, bool)> = vec![
        ("a role of this account", own("pool-auth"), true),
        ("a role of this account under a path", own("service-role/pool-auth"), true),
        (
            "another account's role",
            "arn:aws:iam::123456789012:role/pool-auth".to_string(),
            false,
        ),
        (
            "an account id that is not twelve digits",
            "arn:aws:iam::1234:role/pool-auth".to_string(),
            false,
        ),
        (
            "an IAM user of this account",
            format!("arn:aws:iam::{}:user/alice", engine::emulator_account_id()),
            false,
        ),
        (
            "another service's resource",
            "arn:aws:s3:::a-bucket-named-like-this".to_string(),
            false,
        ),
        (
            "a value that is no ARN at all",
            "not-an-arn-but-long-enough".to_string(),
            false,
        ),
    ];

    for (case, role_arn, admitted) in rows {
        // The same value in both places a configuration names a role.
        let requests = [
            (
                "Roles",
                json!({
                    "IdentityPoolId": &identity_pool_id,
                    "Roles": {"authenticated": &role_arn},
                }),
            ),
            (
                "a mapping rule's RoleARN",
                json!({
                    "IdentityPoolId": &identity_pool_id,
                    "Roles": {"authenticated": own("pool-auth")},
                    "RoleMappings": {
                        "accounts.google.com": {
                            "Type": "Rules",
                            "AmbiguousRoleResolution": "Deny",
                            "RulesConfiguration": {"Rules": [{
                                "Claim": "isAdmin",
                                "MatchType": "Equals",
                                "Value": "yes",
                                "RoleARN": &role_arn,
                            }]},
                        },
                    },
                }),
            ),
        ];

        for (member_named, request) in requests {
            let (status, body) = emulator.call("SetIdentityPoolRoles", &request).await;
            if admitted {
                assert_eq!(
                    status, 200,
                    "case {case:?} at {member_named}: expected the configuration to be \
                     stored, got {status} with {body}"
                );
                continue;
            }
            assert_eq!(
                (status, error_code(&body).as_str()),
                (CLIENT_ERROR_STATUS, "NotAuthorizedException"),
                "case {case:?} at {member_named}: expected the pass-role refusal, got \
                 {status} with {body}"
            );
            assert_eq!(
                error_message(&body),
                "Cross-account pass role is not allowed.",
                "case {case:?} at {member_named}: expected the pass-role sentence, got {body}"
            );
        }
    }
}

/// One write of a pool: the case, the operation that makes it, and the request for the
/// pool the row is run against — built per pool, so the same write is made against the
/// contended pool and against another one.
type PoolWrite = (&'static str, &'static str, Box<dyn Fn(&str) -> Value>);

/// RULE: a pool is reconfigured as a whole, so a write that meets another write of the SAME
/// pool already in flight loses the race: it is refused HTTP 400
/// `ConcurrentModificationException` and stores nothing, while a write of another pool is
/// served. Once the write in flight has finished, the next one is served again — a claim
/// never outlives the request that took it.
///
/// Asserted on both operations that write a pool, and with the race driven by the claim a
/// handler itself takes rather than by two requests timed against each other.
#[tokio::test]
async fn a_write_meeting_another_write_of_the_same_pool_loses_the_race() {
    let emulator = Emulator::new();
    let contended = emulator.pool("contended-pool", false).await;
    let other = emulator.pool("other-pool", false).await;
    let own_role = format!(
        "arn:aws:iam::{}:role/pool-auth",
        engine::emulator_account_id()
    );

    // (case, the operation, a request for the pool the row names)
    let writes: Vec<PoolWrite> = vec![
        (
            "an update",
            "UpdateIdentityPool",
            Box::new(|pool_id: &str| {
                json!({
                    "IdentityPoolId": pool_id,
                    "IdentityPoolName": "renamed-under-contention",
                    "AllowUnauthenticatedIdentities": true,
                })
            }),
        ),
        (
            "a role write",
            "SetIdentityPoolRoles",
            Box::new(move |pool_id: &str| {
                json!({
                    "IdentityPoolId": pool_id,
                    "Roles": {"authenticated": own_role.clone()},
                })
            }),
        ),
    ];

    for (case, operation, request) in writes {
        let before = emulator
            .ok(
                "DescribeIdentityPool",
                &json!({"IdentityPoolId": &contended}),
            )
            .await;

        // One writer of the contended pool is in flight.
        let claim = emulator.pool_write_claim(&contended);

        let (status, body) = emulator.call(operation, &request(&contended)).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (CLIENT_ERROR_STATUS, "ConcurrentModificationException"),
            "case {case:?}: expected the parallel-modification refusal, got {status} with {body}"
        );
        let after = emulator
            .ok(
                "DescribeIdentityPool",
                &json!({"IdentityPoolId": &contended}),
            )
            .await;
        assert_eq!(
            after, before,
            "case {case:?}: the writer that lost the race must store nothing, got {after}"
        );

        // Another pool is not contended: the same call against it is served.
        let (status, served) = emulator.call(operation, &request(&other)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a write of another pool must be served, got {status} with {served}"
        );

        // The writer in flight finishes, and the next write of that pool is served again.
        drop(claim);
        let (status, served) = emulator.call(operation, &request(&contended)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the write after the race must be served, got {status} with {served}"
        );
    }
}
