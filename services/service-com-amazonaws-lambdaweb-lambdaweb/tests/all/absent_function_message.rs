// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what this service answers when the WEB FUNCTION a request
//! addresses is not stored: the modelled `ResourceNotFoundException`, with ONE
//! sentence naming the function, from every operation that resolves one.
//!
//! Every assertion goes over the wire through the same [`Emulator`] harness the
//! `web_function_lifecycle` suite uses: a request is built, dispatched through the
//! production router, and only the HTTP status, the `x-amzn-errortype` header and
//! the JSON body are inspected.
//!
//! The tests are tables: the rows differ along how the addressed name misses (never
//! created, differing only in case, extending or a prefix of a stored name), which
//! form the request named it in (the name or the ARN), and which operation carried
//! it — the deleted function of the cascade check is only one row.
//!
//! The suite also pins the rule that decides WHICH answer an addressed identifier
//! earns: the documented length rule for the name form of an addressing member. A
//! value past that bound is a malformed parameter (the modelled
//! `ValidationException`), not a resource the store does not hold, and the bound
//! applies to the name form only — the ARN form carries the modelled length bound
//! alone and is resolved like any other.

use serde_json::{json, Value};

use super::web_function_lifecycle::{
    build_config, expected_arn, percent_encoded, role_arn, Emulator, Reply,
};

/// The region every request is signed for: the Region a function, and therefore its
/// ARN, is created in.
const REGION: &str = "us-east-1";

/// A `revisionId` no revision carries. `RevisionId` is `@length(1..256)` with no
/// pattern (source: `smithy_query(query="operation",
/// operation="GetWebFunctionRevision")`), so any string is a legal label and the
/// FUNCTION is what the handler fails to resolve first.
const SOME_REVISION_ID: &str = "00000000-0000-0000-0000-000000000000";

/// An `endpointName` no endpoint carries.
const SOME_ENDPOINT_NAME: &str = "an-endpoint";

/// The sentence every operation reports an absent web function with: the fixed text
/// `The function ` … ` does not exist.` around the one variable part, the identifier
/// the request addressed the function by. Written out here rather than taken from
/// the service's helper, so the test pins the wording. Shared with the sibling suite
/// of a listing that resolves a function too.
pub(crate) fn absent_function_message(identifier: &str) -> String {
    format!("The function {identifier} does not exist.")
}

/// The sentence the resource-policy readers report for a function that EXISTS and
/// carries no policy — a different class, same code and status, which is why the
/// absent-function sentence has to be AWS's.
const POLICY_NOT_FOUND_MESSAGE: &str = "The function does not have a policy.";

/// A valid resource-based policy document, for the one call in the table that
/// carries a `@required` policy.
fn a_policy(resource_arn: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "invoke",
            "Effect": "Allow",
            "Principal": { "AWS": "111122223333" },
            "Action": "lambda:InvokeFunction",
            "Resource": resource_arn,
        }],
    })
    .to_string()
}

/// How an operation's input lets a caller name the web function it addresses — the
/// member's own modelled constraints decide which forms reach the handler at all.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Address {
    /// `functionName` is `@length(1..256)` with no pattern and documents both forms
    /// ("The function name … The function ARN"), so a name and an ARN both arrive.
    NameOrArn,
    /// `resource` / `resourceArn` is `ResourceArn`, whose pattern
    /// `^arn:[^:]+:lambda:[^:]+:\d{12}:web-function/[^/]+$` admits the ARN form
    /// only (source: `smithy_query(query="operation", operation="ListTags")`), so a
    /// bare name never reaches the handler.
    ArnOnly,
}

/// One request: the operation that carries it, and the wire call that addresses one
/// identifier with it.
struct Call {
    operation: &'static str,
    address: Address,
    method: &'static str,
    path: String,
    body: Option<Value>,
}

/// Every implemented operation that resolves a web function, addressing it by
/// `identifier`.
///
/// `DeleteWebFunction` resolves one the same way, but it is not in this table: it
/// asks for the function not to exist, so a function that is not stored already
/// satisfies it. The `delete_web_function_idempotence` suite pins that.
fn calls_for(identifier: &str) -> Vec<Call> {
    let label = percent_encoded(identifier);
    vec![
        Call {
            operation: "GetWebFunction",
            address: Address::NameOrArn,
            method: "GET",
            path: format!("/web-functions/{label}"),
            body: None,
        },
        Call {
            operation: "CreateWebFunctionRevision",
            address: Address::NameOrArn,
            method: "POST",
            path: format!("/web-functions/{label}/revisions"),
            body: Some(json!({
                "buildConfig": build_config("python3.13", "demo-bucket", "app.zip", None),
                "serviceConfig": { "executionRoleArn": role_arn("absent-function") },
            })),
        },
        Call {
            operation: "GetWebFunctionRevision",
            address: Address::NameOrArn,
            method: "GET",
            path: format!("/web-functions/{label}/revisions/{SOME_REVISION_ID}"),
            body: None,
        },
        Call {
            operation: "ListWebFunctionRevisions",
            address: Address::NameOrArn,
            method: "POST",
            path: format!("/web-functions/{label}/list-revisions"),
            body: Some(json!({})),
        },
        Call {
            operation: "GetWebFunctionEndpoint",
            address: Address::NameOrArn,
            method: "GET",
            path: format!("/web-functions/{label}/endpoints/{SOME_ENDPOINT_NAME}"),
            body: None,
        },
        Call {
            operation: "GetResourcePolicy",
            address: Address::ArnOnly,
            method: "GET",
            path: format!("/resource-policy/{label}"),
            body: None,
        },
        Call {
            operation: "PutResourcePolicy",
            address: Address::ArnOnly,
            method: "PUT",
            path: format!("/resource-policy/{label}"),
            body: Some(json!({ "Policy": a_policy(identifier) })),
        },
        Call {
            operation: "DeleteResourcePolicy",
            address: Address::ArnOnly,
            method: "DELETE",
            path: format!("/resource-policy/{label}"),
            body: None,
        },
        Call {
            operation: "ListTags",
            address: Address::ArnOnly,
            method: "GET",
            path: format!("/tags/{label}"),
            body: None,
        },
        Call {
            operation: "TagResource",
            address: Address::ArnOnly,
            method: "POST",
            path: format!("/tags/{label}"),
            body: Some(json!({ "Tags": { "env": "prod" } })),
        },
        Call {
            operation: "UntagResource",
            address: Address::ArnOnly,
            method: "DELETE",
            path: format!("/tags/{label}?tagKeys=env"),
            body: None,
        },
    ]
}

/// Create a web function with one revision and return its `functionArn`.
async fn create_function(emulator: &Emulator, name: &str) -> String {
    let created = emulator
        .send(
            "PUT",
            "/web-functions",
            REGION,
            Some(json!({
                "functionName": name,
                "revisionConfig": {
                    "buildConfig": build_config("python3.13", "demo-bucket", "app.zip", None),
                    "serviceConfig": { "executionRoleArn": role_arn(name) },
                },
            })),
        )
        .await;
    assert_eq!(
        created.status, 202,
        "case {name:?}: CreateWebFunction expected 202, got {} with {:?}",
        created.status, created.body
    );
    created
        .string("functionArn")
        .unwrap_or_else(|| panic!("case {name:?}: the create reports a functionArn"))
}

/// Assert `reply` is the modelled not-found for `identifier`, whatever carried it.
fn assert_absent_function(case: &str, identifier: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 404,
        "case {case}: expected HTTP 404, got {} with {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error(),
        "ResourceNotFoundException",
        "case {case}: expected ResourceNotFoundException, got {:?}",
        reply.body
    );
    let expected = absent_function_message(identifier);
    assert_eq!(
        reply.string("message").as_deref(),
        Some(expected.as_str()),
        "case {case}: expected the message {expected:?}, got {:?}",
        reply.body
    );
}

#[tokio::test]
async fn every_operation_reports_an_absent_web_function_with_one_sentence() {
    // RULE: when the web function a request addresses is not stored, this service
    // answers the modelled `ResourceNotFoundException` — HTTP 404, code
    // `ResourceNotFoundException` — and states the absent FUNCTION in one sentence,
    // `The function <identifier> does not exist.`, whose only variable part is the
    // identifier the request named the function by, verbatim. The rows span how the
    // name misses, which form it arrived in, and which operation carried it.
    let emulator = Emulator::new();
    let held_name = "absent-message-fn";
    let held = create_function(&emulator, held_name).await;

    // Each row: what makes the addressed name miss, and the name itself.
    let absent: [(&str, &str); 4] = [
        ("a function that was never created", "never-created"),
        ("a name that differs only in case", "Absent-Message-Fn"),
        ("a name that extends the stored one", "absent-message-fn-2"),
        ("a name that is a prefix of the stored one", "absent-message"),
    ];

    for (row, name) in absent {
        let arn = expected_arn(REGION, name);
        for (form, identifier) in [("the name", name.to_string()), ("the ARN", arn.clone())] {
            for call in calls_for(&identifier) {
                if call.address == Address::ArnOnly && form == "the name" {
                    // The member's pattern admits no bare name, so this request
                    // never reaches the handler at all.
                    continue;
                }
                let reply = emulator
                    .send(call.method, &call.path, REGION, call.body.clone())
                    .await;
                let case = format!(
                    "{row}, addressed by {form} ({identifier}), carried by {}",
                    call.operation
                );
                assert_absent_function(&case, &identifier, &reply);
            }
        }
    }

    // The control: the function that WAS created is still there, addressed by both
    // forms — so the rows above are about absence, not about every call failing.
    for (form, identifier) in [("the name", held_name.to_string()), ("the ARN", held)] {
        let reply = emulator
            .send(
                "GET",
                &format!("/web-functions/{}", percent_encoded(&identifier)),
                REGION,
                None,
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case the stored function addressed by {form}: expected 200, got {} with {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.string("functionName").as_deref(),
            Some(held_name),
            "case the stored function addressed by {form}: expected {held_name:?}, got {:?}",
            reply.body
        );
    }
}

#[tokio::test]
async fn after_a_delete_the_sub_resource_readers_report_the_function_as_absent() {
    // RULE: a function's resource-based policy and its tags live in the function's
    // own record, so its delete takes them with it — and their readers, addressed at
    // the function that is gone, report the FUNCTION as absent with the same one
    // sentence every other operation states, not the sub-resource's own absence and
    // not an empty answer.
    let emulator = Emulator::new();
    let name = "absent-after-delete";
    let arn = create_function(&emulator, name).await;
    let encoded = percent_encoded(&arn);

    // While the function is there, the policy reader distinguishes "no policy yet"
    // from "no function": same code and status, a different sentence.
    let no_policy = emulator
        .send("GET", &format!("/resource-policy/{encoded}"), REGION, None)
        .await;
    assert_eq!(
        no_policy.status, 404,
        "case a stored function carrying no policy: expected HTTP 404, got {} with {:?}",
        no_policy.status, no_policy.body
    );
    assert_eq!(
        no_policy.string("message").as_deref(),
        Some(POLICY_NOT_FOUND_MESSAGE),
        "case a stored function carrying no policy: expected the policy sentence, got {:?}",
        no_policy.body
    );

    // Give the function both sub-resources and read them back.
    let put = emulator
        .send(
            "PUT",
            &format!("/resource-policy/{encoded}"),
            REGION,
            Some(json!({ "Policy": a_policy(&arn) })),
        )
        .await;
    assert_eq!(
        put.status, 200,
        "case the policy to cascade: PutResourcePolicy expected 200, got {} with {:?}",
        put.status, put.body
    );
    let tagged = emulator
        .send(
            "POST",
            &format!("/tags/{encoded}"),
            REGION,
            Some(json!({ "Tags": { "env": "prod" } })),
        )
        .await;
    assert_eq!(
        tagged.status, 204,
        "case the tags to cascade: TagResource expected 204, got {} with {:?}",
        tagged.status, tagged.body
    );
    for (operation, path) in [
        ("GetResourcePolicy", format!("/resource-policy/{encoded}")),
        ("ListTags", format!("/tags/{encoded}")),
    ] {
        let reply = emulator.send("GET", &path, REGION, None).await;
        assert_eq!(
            reply.status, 200,
            "case the sub-resources before the delete ({operation}): expected 200, got {} \
             with {:?}",
            reply.status, reply.body
        );
    }

    let deleted = emulator
        .send(
            "DELETE",
            &format!("/web-functions/{}", percent_encoded(name)),
            REGION,
            None,
        )
        .await;
    assert_eq!(
        deleted.status, 204,
        "case the function's delete: expected 204, got {} with {:?}",
        deleted.status, deleted.body
    );

    // Every reader and writer of a sub-resource now reports the function.
    for call in calls_for(&arn) {
        if call.address != Address::ArnOnly {
            continue;
        }
        let reply = emulator
            .send(call.method, &call.path, REGION, call.body.clone())
            .await;
        assert_absent_function(
            &format!("after the function's delete, carried by {}", call.operation),
            &arn,
            &reply,
        );
    }
}

/// The documented bound on the NAME form of a member that ADDRESSES a stored resource:
/// "The length constraint applies only to the full ARN. If you specify only the
/// function name, it is limited to 64 characters in length." (source:
/// `smithy_query(query="doc", shape="GetWebFunctionRequest", member="functionName")`;
/// the identical sentence is on `endpointName`). Written out here rather than taken from
/// the service's own constant, so the test pins the bound.
const ADDRESSED_NAME_MAX_CHARS: usize = 64;

/// The sentence a value past that bound is refused with, naming the member that carried
/// it — the sentence the create paths already state for the same clause of the same rule.
fn too_long_message(member: &str) -> String {
    format!("Member must have length less than or equal to {ADDRESSED_NAME_MAX_CHARS}: {member}")
}

/// A name of exactly `len` characters satisfying every OTHER clause of the naming rule
/// (letters only, neither a leading nor a trailing hyphen or underscore), so length is
/// the only thing a row varies.
fn name_of_length(len: usize) -> String {
    "n".repeat(len)
}

/// Every IMPLEMENTED operation that addresses a web function by `functionName`,
/// addressing it with `identifier`: the not-found table's rows that take either form,
/// plus the operations missing from it because an absent function earns them something
/// other than that 404 (`DeleteWebFunction` settles as its idempotent success) or
/// because they also write (`DeleteWebFunctionRevision`, `CreateWebFunctionEndpoint`,
/// `UpdateWebFunctionEndpoint`).
///
/// `ListWebFunctionEndpoints` and `DeleteWebFunctionEndpoint` are not here: both are
/// still scaffolded, so they answer 501 to every request.
fn function_addressing_calls(identifier: &str) -> Vec<Call> {
    let label = percent_encoded(identifier);
    let mut calls: Vec<Call> = calls_for(identifier)
        .into_iter()
        .filter(|call| call.address == Address::NameOrArn)
        .collect();
    calls.push(Call {
        operation: "DeleteWebFunction",
        address: Address::NameOrArn,
        method: "DELETE",
        path: format!("/web-functions/{label}"),
        body: None,
    });
    calls.push(Call {
        operation: "DeleteWebFunctionRevision",
        address: Address::NameOrArn,
        method: "DELETE",
        path: format!("/web-functions/{label}/revisions/{SOME_REVISION_ID}"),
        body: None,
    });
    calls.push(Call {
        operation: "CreateWebFunctionEndpoint",
        address: Address::NameOrArn,
        method: "PUT",
        path: format!("/web-functions/{label}/endpoints"),
        body: Some(json!({
            "endpointName": SOME_ENDPOINT_NAME,
            "endpointType": "HomeRegion",
            "authType": "ApplicationManaged",
        })),
    });
    calls.push(Call {
        operation: "UpdateWebFunctionEndpoint",
        address: Address::NameOrArn,
        method: "PATCH",
        path: format!("/web-functions/{label}/endpoints/{SOME_ENDPOINT_NAME}"),
        body: Some(json!({})),
    });
    calls
}

/// Every IMPLEMENTED operation that addresses an ENDPOINT of `function` by
/// `endpointName`, addressing it with `identifier`. `CreateWebFunctionEndpoint` is not
/// here: its `endpointName` NAMES the endpoint it creates rather than addressing a
/// stored one, so the create path's own naming rule holds it.
fn endpoint_addressing_calls(function: &str, identifier: &str) -> Vec<Call> {
    let function_label = percent_encoded(function);
    let label = percent_encoded(identifier);
    vec![
        Call {
            operation: "GetWebFunctionEndpoint",
            address: Address::NameOrArn,
            method: "GET",
            path: format!("/web-functions/{function_label}/endpoints/{label}"),
            body: None,
        },
        Call {
            operation: "UpdateWebFunctionEndpoint",
            address: Address::NameOrArn,
            method: "PATCH",
            path: format!("/web-functions/{function_label}/endpoints/{label}"),
            body: Some(json!({})),
        },
    ]
}

/// Assert `reply` is the modelled refusal of a `member` value past the documented bound.
fn assert_name_too_long(case: &str, member: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 400,
        "case {case}: expected HTTP 400, got {} with {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error(),
        "ValidationException",
        "case {case}: expected ValidationException, got {:?}",
        reply.body
    );
    let expected = too_long_message(member);
    assert_eq!(
        reply.string("message").as_deref(),
        Some(expected.as_str()),
        "case {case}: expected the message {expected:?}, got {:?}",
        reply.body
    );
}

#[tokio::test]
async fn a_name_past_the_documented_bound_is_refused_by_every_operation_that_addresses_one() {
    // RULE: a member that addresses an already-stored resource takes either the name or
    // the ARN, and the documented length rule for the NAME form bounds it at 64
    // characters; a value past that bound is a parameter outside the allowed range, so
    // every operation that resolves such a value answers the modelled
    // `ValidationException` (HTTP 400) BEFORE consulting the store, rather than the
    // not-found of a resource the store does not hold. The rows walk how far past the
    // bound the value is, up to the modelled maximum the boundary admits, and which
    // operation carried it.
    let emulator = Emulator::new();
    let rows: [(&str, usize); 3] = [
        ("one character past the documented bound", ADDRESSED_NAME_MAX_CHARS + 1),
        ("well past the documented bound", 128),
        ("at the modelled maximum for the member", 256),
    ];

    for (row, len) in rows {
        let name = name_of_length(len);
        for call in function_addressing_calls(&name) {
            let reply = emulator
                .send(call.method, &call.path, REGION, call.body.clone())
                .await;
            let case = format!(
                "{row} ({len} characters), carried by {} as functionName",
                call.operation
            );
            assert_name_too_long(&case, "functionName", &reply);
        }
    }

    // The control: nothing was stored by any of those calls, so the refusals above are
    // refusals and not writes.
    let listed = emulator.send("POST", "/web-functions", REGION, Some(json!({}))).await;
    assert_eq!(
        listed.field("functions").and_then(Value::as_array).map(Vec::len),
        Some(0),
        "case the store after every refused call: expected no function, got {:?}",
        listed.body
    );
}

#[tokio::test]
async fn the_documented_bound_itself_is_addressable() {
    // RULE, the other side of the bound: a name OF the bound is within the rule, so it
    // addresses a resource like any other — the function stored under it is read back,
    // and one that is not stored earns the not-found rather than the refusal. This is
    // what makes the rows of the test above "past the bound".
    let emulator = Emulator::new();
    let name = name_of_length(ADDRESSED_NAME_MAX_CHARS);
    assert_eq!(
        name.chars().count(),
        ADDRESSED_NAME_MAX_CHARS,
        "the bound row is the bound, got {} characters",
        name.chars().count()
    );

    // Not stored yet: the not-found, from every operation that addresses it.
    for call in calls_for(&name) {
        if call.address != Address::NameOrArn {
            continue;
        }
        let reply = emulator
            .send(call.method, &call.path, REGION, call.body.clone())
            .await;
        assert_absent_function(
            &format!(
                "a name of the bound that is not stored, carried by {}",
                call.operation
            ),
            &name,
            &reply,
        );
    }

    // Stored: the create accepts the same name, and the reader answers it.
    let arn = create_function(&emulator, &name).await;
    for (form, identifier) in [("the name", name.clone()), ("the ARN", arn)] {
        let reply = emulator
            .send(
                "GET",
                &format!("/web-functions/{}", percent_encoded(&identifier)),
                REGION,
                None,
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case a function named at the bound, addressed by {form}: expected 200, got {} \
             with {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.string("functionName").as_deref(),
            Some(name.as_str()),
            "case a function named at the bound, addressed by {form}: expected {name:?}, \
             got {:?}",
            reply.body
        );
    }
}

#[tokio::test]
async fn the_bound_applies_to_the_name_form_only_and_never_to_the_arn_form() {
    // RULE: "The length constraint applies only to the full ARN" — a value in the ARN
    // form carries the member's modelled length bound alone, so an ARN longer than the
    // name bound, one whose resource segment is itself past that bound included, is
    // RESOLVED like any other and earns the not-found of the function it names, never
    // the length refusal. The rows walk the length of the name inside the ARN, up to the
    // one whose ARN is exactly the member's modelled maximum.
    let emulator = Emulator::new();
    let arn_overhead = expected_arn(REGION, "").chars().count();
    let rows: [(&str, usize); 4] = [
        ("a name within the bound", 8),
        ("a name of the bound", ADDRESSED_NAME_MAX_CHARS),
        ("a name past the bound", ADDRESSED_NAME_MAX_CHARS + 1),
        ("a name whose ARN is the modelled maximum", 256 - arn_overhead),
    ];

    for (row, len) in rows {
        let name = name_of_length(len);
        let arn = expected_arn(REGION, &name);
        assert!(
            arn.chars().count() <= 256,
            "case {row}: the ARN row has to be inside the modelled bound, got {} characters",
            arn.chars().count()
        );
        for call in function_addressing_calls(&arn) {
            let reply = emulator
                .send(call.method, &call.path, REGION, call.body.clone())
                .await;
            let case = format!(
                "{row} ({len} characters) addressed by its ARN, carried by {}",
                call.operation
            );
            assert_ne!(
                reply.string("message").as_deref(),
                Some(too_long_message("functionName").as_str()),
                "case {case}: the ARN form carries the modelled bound only, got {:?}",
                reply.body
            );
            if call.operation == "DeleteWebFunction" {
                // A delete of a function that is not stored asks for what already
                // holds, so it settles as its modelled success.
                assert_eq!(
                    reply.status, 204,
                    "case {case}: expected 204, got {} with {:?}",
                    reply.status, reply.body
                );
                continue;
            }
            assert_absent_function(&case, &arn, &reply);
        }
    }
}

#[tokio::test]
async fn the_endpoint_name_form_is_held_to_the_same_bound() {
    // RULE, one member along: `endpointName` documents the same sentence, so an endpoint
    // addressed by a name past the bound is the same malformed parameter, while a name of
    // the bound and an endpoint ARN of any admitted length are resolved — the endpoint
    // that is not stored on the function earns the endpoint's own not-found.
    let emulator = Emulator::new();
    let function = "endpoint-name-bound-fn";
    let function_arn = create_function(&emulator, function).await;

    for (row, len) in [
        ("one character past the documented bound", ADDRESSED_NAME_MAX_CHARS + 1),
        ("well past the documented bound", 200),
    ] {
        let endpoint = name_of_length(len);
        for call in endpoint_addressing_calls(function, &endpoint) {
            let reply = emulator
                .send(call.method, &call.path, REGION, call.body.clone())
                .await;
            let case = format!(
                "{row} ({len} characters), carried by {} as endpointName",
                call.operation
            );
            assert_name_too_long(&case, "endpointName", &reply);
        }
    }

    // A name of the bound, and the ARN form of a name past it, are both resolved: the
    // answer is the endpoint's own not-found, not the length refusal.
    let at_the_bound = name_of_length(ADDRESSED_NAME_MAX_CHARS);
    let past_the_bound = name_of_length(ADDRESSED_NAME_MAX_CHARS + 1);
    let resolved: [(&str, String); 2] = [
        ("a name of the bound", at_the_bound),
        (
            "the ARN of a name past the bound",
            format!("{function_arn}/endpoint/{past_the_bound}"),
        ),
    ];
    for (row, identifier) in resolved {
        for call in endpoint_addressing_calls(function, &identifier) {
            let reply = emulator
                .send(call.method, &call.path, REGION, call.body.clone())
                .await;
            let case = format!("{row}, carried by {}", call.operation);
            assert_eq!(
                reply.status, 404,
                "case {case}: expected HTTP 404, got {} with {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error(),
                "ResourceNotFoundException",
                "case {case}: expected ResourceNotFoundException, got {:?}",
                reply.body
            );
            let expected =
                format!("The endpoint {identifier} doesn't exist on web function {function}.");
            assert_eq!(
                reply.string("message").as_deref(),
                Some(expected.as_str()),
                "case {case}: expected the message {expected:?}, got {:?}",
                reply.body
            );
        }
    }
}
