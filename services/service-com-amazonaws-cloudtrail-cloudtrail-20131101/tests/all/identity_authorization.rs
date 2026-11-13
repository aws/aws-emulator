// SPDX-License-Identifier: CC0-1.0

//! WHO may call this service, driven end to end over the real wire path — an `awsJson1_1`
//! `POST /` through the same `MultiServiceRouter` the production router serves — so what is
//! asserted is what a client holding those credentials sees, status, error code and message.
//!
//! THE RULE UNDER TEST: every operation of this service is authorized against the
//! identity-based IAM policies in force on the principal its credentials name, for the
//! action `cloudtrail:<OperationName>` on the resource the request addresses, BEFORE the
//! operation acts on anything. A request no readable statement allows (IAM's default deny),
//! or one an explicit `Deny` covers, is answered HTTP 403 `AccessDeniedException` carrying
//! IAM's own sentence and changes nothing; a request a matching `Allow` covers is served.
//!
//! The IAM user, its access key and its policies live in the IAM store of the SAME shared
//! engine the service reads — the records `iam:CreateUser`, `iam:CreateAccessKey` and
//! `iam:PutUserPolicy` write — so the documents under test are the ones that service
//! stores, and the principal a refusal blames is the one those records name.
//!
//! The tables span what the rule generalises over: the document in force (none at all,
//! exactly this action, this whole service's namespace, another service's namespace, this
//! operation's name under the wrong namespace, everything, an explicit deny beside an
//! allow), the OPERATION (one that addresses no resource, one that addresses a trail by
//! name, one that addresses a list of them, and a write), the RESOURCE an allow is scoped
//! to (every resource, the trail the request names, another trail of the same account), and
//! the kind of caller (an IAM user's long-term key and the account's own published
//! credential).

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;

use crate::emulator::{Emulator, AMBIENT_ACCESS_KEY_ID, HOME_REGION};

/// The access key id the IAM user's calls are signed with, as `iam:CreateAccessKey` mints
/// one for a user.
const USER_KEY_ID: &str = "AKIAEXAMPLECTAUDITOR1";

/// The IAM user every denied row speaks for, and the name of the inline policy it carries.
const USER: &str = "trail-auditor";
const POLICY_NAME: &str = "inline";

/// The refusal an unauthorized call is answered with: the shape this service's model
/// declares for it and the status that shape's `@httpError` states.
const ACCESS_DENIED: (u16, &str) = (403, "AccessDeniedException");

/// The bucket every trail in this suite delivers to; the harness records it for the call.
const BUCKET: &str = "authz-trail-logs";

/// The ARN the account holds the IAM user under — what a refusal names as the caller.
fn user_arn() -> String {
    format!(
        "arn:aws:iam::{}:user/{USER}",
        engine::emulator_account_id()
    )
}

/// The ARN of the trail called `name`, as this service mints one in [`HOME_REGION`].
fn trail_arn(name: &str) -> String {
    format!(
        "arn:aws:cloudtrail:{HOME_REGION}:{}:trail/{name}",
        engine::emulator_account_id()
    )
}

/// Record the IAM user [`USER_KEY_ID`] belongs to, as `iam:CreateUser` and
/// `iam:CreateAccessKey` record one: the user, and the key record that maps the access key
/// id to it.
fn given_user(engine: &Arc<engine::InMemoryEngine>) {
    let minted = aws_smithy_types::DateTime::from_secs(0);
    let state = iam::service_state(&**engine).expect("the in-memory engine carries the IAM store");
    let mut store = state.lock();
    store.users.insert(
        iam::name_key(USER),
        service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::User {
            path: "/".to_string(),
            user_name: USER.to_string(),
            user_id: "AIDAEXAMPLECTAUDITOR1".to_string(),
            arn: user_arn(),
            create_date: minted,
            password_last_used: None,
            permissions_boundary: None,
            tags: None,
        },
    );
    store.access_keys.insert(
        USER_KEY_ID.to_string(),
        iam::AccessKeyRecord {
            user_key: iam::name_key(USER),
            status: iam::ACCESS_KEY_ACTIVE_STATUS.to_string(),
            create_date: minted,
        },
    );
}

/// Put `document` in force as the user's inline policy, replacing whatever it carried —
/// what `iam:PutUserPolicy` does. `None` takes every policy off it, which is the state
/// `iam:CreateUser` leaves a brand-new user in and what `iam:DeleteUserPolicy` returns it
/// to.
fn given_user_policy(engine: &Arc<engine::InMemoryEngine>, document: Option<&Value>) {
    let state = iam::service_state(&**engine).expect("the in-memory engine carries the IAM store");
    let mut store = state.lock();
    let key = iam::name_key(USER);
    match document {
        None => {
            store.user_inline_policies.remove(&key);
        }
        Some(document) => {
            store.user_inline_policies.insert(
                key,
                BTreeMap::from([(
                    iam::name_key(POLICY_NAME),
                    iam::InlinePolicy {
                        policy_name: POLICY_NAME.to_string(),
                        policy_document: document.to_string(),
                    },
                )]),
            );
        }
    }
}

/// A policy with one statement: `effect` on `actions` over `resources`.
fn policy(effect: &str, actions: Value, resources: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": effect, "Action": actions, "Resource": resources}],
    })
}

/// IAM's own refusal sentence, spelled out here rather than read from the code under test:
/// the principal that signed, the action it asked for, the resource clause for a request
/// that determined a resource (and none for one judged against every resource), and which
/// of IAM's rules withheld the grant.
fn denial(principal: &str, action: &str, resource: Option<&str>, reason: &str) -> String {
    match resource {
        Some(resource) => format!(
            "User: {principal} is not authorized to perform: {action} on resource: \
             {resource} {reason}"
        ),
        None => format!("User: {principal} is not authorized to perform: {action} {reason}"),
    }
}

/// IAM's default deny: no readable statement allows the action.
fn no_policy_allows(action: &str) -> String {
    format!("because no identity-based policy allows the {action} action")
}

/// The clause a refusal states when a readable document DENIES the action outright.
const EXPLICIT_DENY: &str = "with an explicit deny in an identity-based policy";

/// One row of the authorization table: a call to make, and whether IAM lets it through.
struct Row {
    case: &'static str,
    /// The document in force on the IAM user, or `None` for a user carrying none.
    document: Option<Value>,
    operation: &'static str,
    request: Value,
    /// The resource the refusal of this call names, or `None` where the call addresses no
    /// single resource and the sentence states no resource clause.
    resource: Option<String>,
    /// `None` for a call IAM lets through; the reason clause of the refusal otherwise.
    refused_because: Option<String>,
}

/// RULE: a call is served only when a readable identity-based statement allows
/// `cloudtrail:<OperationName>` on what the request addresses; otherwise it is refused 403
/// `AccessDeniedException` with IAM's sentence, and nothing of the request takes effect.
#[tokio::test]
async fn a_call_is_served_only_when_an_identity_policy_allows_its_action() {
    let emulator = Emulator::new();
    given_user(emulator.engine());
    // One trail the reading rows address, created by the account's own credential.
    emulator
        .call(
            "CreateTrail",
            &json!({"Name": "authz-read", "S3BucketName": BUCKET}),
        )
        .await
        .expect_ok("CreateTrail by the account credential");

    let list_action = "cloudtrail:ListTrails";
    let status_action = "cloudtrail:GetTrailStatus";
    let describe_action = "cloudtrail:DescribeTrails";
    let create_action = "cloudtrail:CreateTrail";
    let rows = vec![
        Row {
            case: "no policy at all, on a call that addresses no resource",
            document: None,
            operation: "ListTrails",
            request: json!({}),
            resource: None,
            refused_because: Some(no_policy_allows(list_action)),
        },
        Row {
            case: "no policy at all, on a call that addresses one trail",
            document: None,
            operation: "GetTrailStatus",
            request: json!({"Name": "authz-read"}),
            resource: Some(trail_arn("authz-read")),
            refused_because: Some(no_policy_allows(status_action)),
        },
        Row {
            case: "no policy at all, on a call that addresses a list of trails",
            document: None,
            operation: "DescribeTrails",
            request: json!({"trailNameList": ["authz-read"]}),
            resource: Some(trail_arn("authz-read")),
            refused_because: Some(no_policy_allows(describe_action)),
        },
        Row {
            case: "no policy at all, on a write",
            document: None,
            operation: "CreateTrail",
            request: json!({"Name": "authz-denied-write", "S3BucketName": BUCKET}),
            resource: Some(trail_arn("authz-denied-write")),
            refused_because: Some(no_policy_allows(create_action)),
        },
        Row {
            case: "exactly this action, on every resource",
            document: Some(policy("Allow", json!(list_action), json!("*"))),
            operation: "ListTrails",
            request: json!({}),
            resource: None,
            refused_because: None,
        },
        Row {
            case: "this whole service's namespace",
            document: Some(policy("Allow", json!("cloudtrail:*"), json!("*"))),
            operation: "GetTrailStatus",
            request: json!({"Name": "authz-read"}),
            resource: Some(trail_arn("authz-read")),
            refused_because: None,
        },
        Row {
            case: "every action of every service",
            document: Some(policy("Allow", json!("*"), json!("*"))),
            operation: "DescribeTrails",
            request: json!({"trailNameList": ["authz-read"]}),
            resource: Some(trail_arn("authz-read")),
            refused_because: None,
        },
        Row {
            case: "another service's namespace",
            document: Some(policy("Allow", json!("cloudtrail-data:*"), json!("*"))),
            operation: "ListTrails",
            request: json!({}),
            resource: None,
            refused_because: Some(no_policy_allows(list_action)),
        },
        Row {
            case: "this operation's name under another service's namespace",
            document: Some(policy("Allow", json!("logs:ListTrails"), json!("*"))),
            operation: "ListTrails",
            request: json!({}),
            resource: None,
            refused_because: Some(no_policy_allows(list_action)),
        },
        Row {
            case: "a grant of a DIFFERENT action of this service",
            document: Some(policy("Allow", json!("cloudtrail:DescribeTrails"), json!("*"))),
            operation: "ListTrails",
            request: json!({}),
            resource: None,
            refused_because: Some(no_policy_allows(list_action)),
        },
        Row {
            case: "an explicit deny beside an allow of everything",
            document: Some(json!({
                "Version": "2012-10-17",
                "Statement": [
                    {"Effect": "Allow", "Action": "*", "Resource": "*"},
                    {"Effect": "Deny", "Action": "cloudtrail:ListTrails", "Resource": "*"},
                ],
            })),
            operation: "ListTrails",
            request: json!({}),
            resource: None,
            refused_because: Some(EXPLICIT_DENY.to_string()),
        },
        Row {
            case: "a write the policy allows",
            document: Some(policy("Allow", json!("cloudtrail:CreateTrail"), json!("*"))),
            operation: "CreateTrail",
            request: json!({"Name": "authz-allowed-write", "S3BucketName": BUCKET}),
            resource: Some(trail_arn("authz-allowed-write")),
            refused_because: None,
        },
    ];

    for Row {
        case,
        document,
        operation,
        request,
        resource,
        refused_because,
    } in rows
    {
        given_user_policy(emulator.engine(), document.as_ref());
        let reply = emulator
            .call_in_as(operation, USER_KEY_ID, HOME_REGION, &request)
            .await;
        match refused_because {
            None => {
                reply.expect_ok(case);
            }
            Some(reason) => {
                reply.expect_error(case, ACCESS_DENIED.0, ACCESS_DENIED.1);
                let action = format!("cloudtrail:{operation}");
                let expected = denial(&user_arn(), &action, resource.as_deref(), &reason);
                assert_eq!(
                    reply.message(),
                    expected,
                    "case {case:?}: expected IAM's sentence {expected:?}, got {:?}",
                    reply.message()
                );
            }
        }
    }

    // The refused write stored nothing, and the allowed one did: the decision is taken
    // before the operation acts, so a denial cannot leave a trail behind.
    let listed = emulator
        .call("ListTrails", &json!({}))
        .await
        .expect_ok("ListTrails by the account credential")
        .get("Trails")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let names: Vec<String> = listed
        .iter()
        .map(|trail| {
            trail
                .get("Name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert!(
        !names.contains(&"authz-denied-write".to_string())
            && names.contains(&"authz-allowed-write".to_string()),
        "case \"a refused write stores nothing\": expected only the allowed trail, got \
         {names:?}"
    );
}

/// RULE: the decision is taken against the resource the request ADDRESSES, so a grant
/// scoped to one trail's ARN covers calls on that trail and no other — and a call
/// addressing several resources needs the grant to cover every one of them.
#[tokio::test]
async fn a_grant_is_read_for_the_resource_the_request_addresses() {
    let emulator = Emulator::new();
    given_user(emulator.engine());
    for name in ["authz-granted", "authz-other"] {
        emulator
            .call(
                "CreateTrail",
                &json!({"Name": name, "S3BucketName": BUCKET}),
            )
            .await
            .expect_ok(name);
    }
    given_user_policy(
        emulator.engine(),
        Some(&policy(
            "Allow",
            json!("cloudtrail:*"),
            json!(trail_arn("authz-granted")),
        )),
    );

    // (case, the operation, the request, the resource a refusal names — `None` for a call
    // the grant covers)
    let rows: Vec<(&str, &str, Value, Option<String>)> = vec![
        (
            "the granted trail, addressed by its bare name",
            "GetTrailStatus",
            json!({"Name": "authz-granted"}),
            None,
        ),
        (
            "the granted trail, addressed by its ARN",
            "GetTrailStatus",
            json!({"Name": trail_arn("authz-granted")}),
            None,
        ),
        (
            "another trail of the same account",
            "GetTrailStatus",
            json!({"Name": "authz-other"}),
            Some(trail_arn("authz-other")),
        ),
        (
            "both trails at once, one of them ungranted",
            "DescribeTrails",
            json!({"trailNameList": ["authz-granted", "authz-other"]}),
            Some(trail_arn("authz-other")),
        ),
        (
            "both trails at once, both granted",
            "DescribeTrails",
            json!({"trailNameList": ["authz-granted", "authz-granted"]}),
            None,
        ),
        (
            "a call addressing no trail at all, which the scoped grant does not cover",
            "ListTrails",
            json!({}),
            // Judged against every resource of this service, which the ARN-scoped
            // statement says nothing about — so the sentence states no resource clause.
            Some(String::new()),
        ),
    ];

    for (case, operation, request, refused_on) in rows {
        let reply = emulator
            .call_in_as(operation, USER_KEY_ID, HOME_REGION, &request)
            .await;
        match refused_on {
            None => {
                reply.expect_ok(case);
            }
            Some(resource) => {
                reply.expect_error(case, ACCESS_DENIED.0, ACCESS_DENIED.1);
                let action = format!("cloudtrail:{operation}");
                let named = (!resource.is_empty()).then_some(resource.as_str());
                let expected =
                    denial(&user_arn(), &action, named, &no_policy_allows(&action));
                assert_eq!(
                    reply.message(),
                    expected,
                    "case {case:?}: expected IAM's sentence {expected:?}, got {:?}",
                    reply.message()
                );
            }
        }
    }
}

/// RULE: the account's own published credential holds no identity-based policy, so it is
/// served — and granting the action to an IAM user that was refused makes the identical
/// call succeed, while taking the policy off refuses it again.
#[tokio::test]
async fn a_grant_and_its_removal_decide_the_very_next_call() {
    let emulator = Emulator::new();
    given_user(emulator.engine());
    emulator
        .call(
            "CreateTrail",
            &json!({"Name": "authz-sequence", "S3BucketName": BUCKET}),
        )
        .await
        .expect_ok("CreateTrail by the account credential");

    // (case, the access key id the call is signed with, the document in force, whether the
    // call is served)
    let rows: Vec<(&str, &str, Option<Value>, bool)> = vec![
        (
            "the account's own published credential, with no policy anywhere",
            AMBIENT_ACCESS_KEY_ID,
            None,
            true,
        ),
        ("the IAM user before any grant", USER_KEY_ID, None, false),
        (
            "the same user once the action is granted",
            USER_KEY_ID,
            Some(policy("Allow", json!("cloudtrail:ListTrails"), json!("*"))),
            true,
        ),
        (
            "the same user once the policy is taken off again",
            USER_KEY_ID,
            None,
            false,
        ),
    ];

    for (case, access_key_id, document, served) in rows {
        if access_key_id == USER_KEY_ID {
            given_user_policy(emulator.engine(), document.as_ref());
        }
        let reply = emulator
            .call_in_as("ListTrails", access_key_id, HOME_REGION, &json!({}))
            .await;
        if served {
            let trails = reply
                .expect_ok(case)
                .get("Trails")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert!(
                !trails.is_empty(),
                "case {case:?}: expected the account's trails, got {trails:?}"
            );
        } else {
            reply.expect_error(case, ACCESS_DENIED.0, ACCESS_DENIED.1);
        }
    }
}

/// RULE: a call the authorization decision refused is still a management event of the
/// account — recorded under its own name with the refusal's code and message — and its
/// record carries NO request parameters, because the call reached no operation to be
/// dispatched with them.
#[tokio::test]
async fn a_refused_call_is_recorded_without_the_parameters_it_never_carried() {
    let emulator = Emulator::new();
    given_user(emulator.engine());
    emulator
        .call(
            "CreateTrail",
            &json!({"Name": "authz-recorded", "S3BucketName": BUCKET}),
        )
        .await
        .expect_ok("CreateTrail by the account credential");

    // Two refused calls of different kinds: a read that addresses one trail and a write
    // that would have deleted it.
    for (case, operation, request) in [
        (
            "a refused read",
            "GetTrailStatus",
            json!({"Name": "authz-recorded"}),
        ),
        (
            "a refused write",
            "DeleteTrail",
            json!({"Name": "authz-recorded"}),
        ),
    ] {
        emulator
            .call_in_as(operation, USER_KEY_ID, HOME_REGION, &request)
            .await
            .expect_error(case, ACCESS_DENIED.0, ACCESS_DENIED.1);

        let events = emulator
            .call(
                "LookupEvents",
                &json!({
                    "LookupAttributes": [
                        {"AttributeKey": "EventName", "AttributeValue": operation},
                    ],
                }),
            )
            .await
            .expect_ok(case)
            .get("Events")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            !events.is_empty(),
            "case {case:?}: a refused call is still recorded, got {events:?}"
        );
        let raw = events[0]
            .get("CloudTrailEvent")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let document: Value = serde_json::from_str(&raw)
            .unwrap_or_else(|error| panic!("case {case:?}: {raw:?} is not JSON ({error})"));
        let action = format!("cloudtrail:{operation}");
        let expected = denial(
            &user_arn(),
            &action,
            Some(&trail_arn("authz-recorded")),
            &no_policy_allows(&action),
        );
        assert_eq!(
            (
                document.get("errorCode").and_then(Value::as_str),
                document.get("errorMessage").and_then(Value::as_str),
                document.get("requestParameters"),
            ),
            (
                Some(ACCESS_DENIED.1),
                Some(expected.as_str()),
                Some(&Value::Null),
            ),
            "case {case:?}: expected the refusal recorded without its parameters, got \
             {document}"
        );
    }

    // The refused delete changed nothing: the trail is still there to be read.
    emulator
        .call("GetTrailStatus", &json!({"Name": "authz-recorded"}))
        .await
        .expect_ok("the trail a refused delete left alone");
}
