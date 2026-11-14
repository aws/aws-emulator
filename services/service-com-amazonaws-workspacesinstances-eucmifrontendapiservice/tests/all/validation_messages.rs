// SPDX-License-Identifier: CC0-1.0

//! What this service's `ValidationException` MESSAGE says when the boundary
//! refuses a request, driven over the wire this service really speaks
//! (awsJson1_0, `POST /`, `X-Amz-Target: EUCMIFrontendAPIService.<Operation>`).
//!
//! The rule under test, stated once and asserted over a set of inputs rather
//! than one recorded request:
//!
//! > a refused request names, per violated constraint, the offending MEMBER PATH
//! > and the CONSTRAINT it violated and nothing of the value the caller sent (an
//! > absent member being reported as the `null` it was); a violation on an
//! > ELEMENT of a collection is reported against the collection member itself
//! > with the element's constraint nested inside
//! > `Member must satisfy constraint: [ … ]`; and a `@pattern` is quoted without
//! > the `^` / `$` anchors the model spells it with.
//!
//! Every row below differs from the others along what that rule generalises
//! over: the constraint class (`@range`, `@length`, `@pattern`, enum value set,
//! `@required`), the bound (two different `MaxResults` shapes, 1..25 and 1..600),
//! the operation, the member, the depth (a scalar member, a scalar list element,
//! a member inside a structure element) and the number of violations in one
//! request. The recorded cases from the issue are three of those rows.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_workspacesinstances_eucmifrontendapiservice as wsinst;
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix from the crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "EUCMIFrontendAPIService";

/// `WorkspaceInstanceId`'s `@pattern` as the service advertises it: the modelled
/// `^wsinst-[0-9a-zA-Z]{8,63}$` without its anchors
/// (`smithy_query(query="shape",shape="WorkspaceInstanceId")`).
const ID_PATTERN: &str = "wsinst-[0-9a-zA-Z]{8,63}";

/// `TagKey`'s `@pattern` as advertised: the modelled
/// `^([\p{L}\p{Z}\p{N}_.:/=+\-@]+)$` without its anchors
/// (`smithy_query(query="shape",shape="TagKey")`).
const TAG_KEY_PATTERN: &str = r"([\p{L}\p{Z}\p{N}_.:/=+\-@]+)";

/// `ClientToken`'s `@pattern` as advertised: the modelled `^[\x20-\x7E]+$`
/// without its anchors — printable ASCII
/// (`smithy_query(query="operation",operation="CreateWorkspaceInstance")`).
const CLIENT_TOKEN_PATTERN: &str = r"[\x20-\x7E]+";

/// A router carrying this service and the service that owns the EC2 instance a WorkSpace
/// Instance is, over one shared engine — the same `EmulatedService::new(engine)` seam
/// `core/router` mounts both through. Amazon EC2 is mounted because a create launches a
/// real instance there.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are the emulator's concern, not this service's.
    router.set_request_authentication(false);
    router.register_service(wsinst::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_ec2_amazonec2::EmulatedService::new(Arc::clone(&engine)),
    );
    // The network and the images the create fixtures name, in the account's own EC2.
    crate::workspace_instance_lifecycle::seed_account_network(&engine);
    router
}

/// One awsJson1_0 call. Returns the HTTP status and the raw body text (the text,
/// because a row asserts what the body does NOT contain as well as what it does).
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, String) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "workspaces-instances.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/workspaces-instances/aws4_request, SignedHeaders=host, Signature=0",
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    (status, String::from_utf8_lossy(&raw).into_owned())
}

/// The awsJson error envelope's `__type` discriminator.
fn error_code(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .as_ref()
        .and_then(|v| v.get("__type"))
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `message` of a refusal, asserting first that the refusal is the modelled
/// `ValidationException` with HTTP 400.
async fn refusal_message(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
) -> String {
    let (status, raw) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?} on {operation}: expected HTTP 400, got {status} with {raw}"
    );
    let code = error_code(&raw);
    assert_eq!(
        code, "ValidationException",
        "case {case:?} on {operation}: expected ValidationException, got {code:?} with {raw}"
    );
    serde_json::from_str::<Value>(&raw)
        .ok()
        .as_ref()
        .and_then(|v| v.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?} on {operation}: no message in {raw}"))
        .to_string()
}

/// A valid `WorkspaceInstanceId` of `len` characters: the modelled prefix plus
/// `len - 7` alphanumerics (`len 15..70` leaves the suffix inside 8..63).
fn valid_id(len: usize) -> String {
    format!("wsinst-{}", "a".repeat(len - "wsinst-".len()))
}

/// One table row: a request, and the whole message its refusal must carry.
struct Case {
    /// What this row exercises, named in every assertion.
    name: &'static str,
    operation: &'static str,
    body: Value,
    message: String,
}

#[tokio::test]
async fn a_clause_names_the_member_path_and_the_constraint_only() {
    let router = router();
    let long_id = valid_id(71);
    let cases = vec![
        // `@range`, above the maximum — `MaxResults` on ListRegions is 1..25.
        Case {
            name: "maxResults above the 1..25 maximum",
            operation: "ListRegions",
            body: json!({ "MaxResults": 999 }),
            message: "1 validation error detected: Value at 'maxResults' failed to satisfy \
                      constraint: Member must have value less than or equal to 25"
                .to_string(),
        },
        // One past the published maximum, on another operation carrying the same
        // shape: the clause is about the member and its bound, not the operation.
        Case {
            name: "maxResults one past the 1..25 maximum",
            operation: "ListWorkspaceInstances",
            body: json!({ "MaxResults": 26 }),
            message: "1 validation error detected: Value at 'maxResults' failed to satisfy \
                      constraint: Member must have value less than or equal to 25"
                .to_string(),
        },
        // `@range`, below the minimum.
        Case {
            name: "maxResults one below the 1..25 minimum",
            operation: "ListWorkspaceInstances",
            body: json!({ "MaxResults": 0 }),
            message: "1 validation error detected: Value at 'maxResults' failed to satisfy \
                      constraint: Member must have value greater than or equal to 1"
                .to_string(),
        },
        // A DIFFERENT bound on a differently-shaped `MaxResults`:
        // `ListInstanceTypesMaxResults` is 1..600, so the clause states 600.
        Case {
            name: "maxResults one past the 1..600 maximum",
            operation: "ListInstanceTypes",
            body: json!({ "MaxResults": 601 }),
            message: "1 validation error detected: Value at 'maxResults' failed to satisfy \
                      constraint: Member must have value less than or equal to 600"
                .to_string(),
        },
        Case {
            name: "maxResults one below the 1..600 minimum",
            operation: "ListInstanceTypes",
            body: json!({ "MaxResults": 0 }),
            message: "1 validation error detected: Value at 'maxResults' failed to satisfy \
                      constraint: Member must have value greater than or equal to 1"
                .to_string(),
        },
        // `@pattern`, quoted WITHOUT the modelled anchors.
        Case {
            name: "workspaceInstanceId outside the modelled pattern",
            operation: "GetWorkspaceInstance",
            body: json!({ "WorkspaceInstanceId": "not-a-valid-workspace-instance-x" }),
            message: format!(
                "1 validation error detected: Value at 'workspaceInstanceId' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: {ID_PATTERN}"
            ),
        },
        // The same member on another operation: same clause.
        Case {
            name: "workspaceInstanceId outside the pattern, on a write",
            operation: "DeleteWorkspaceInstance",
            body: json!({ "WorkspaceInstanceId": "wsinst_has_an_underscore_x" }),
            message: format!(
                "1 validation error detected: Value at 'workspaceInstanceId' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: {ID_PATTERN}"
            ),
        },
        // Two violations in one request: `len 15..70`'s minimum and the pattern,
        // which no value can fail separately (the pattern pins the same bounds).
        Case {
            name: "workspaceInstanceId shorter than the modelled minimum",
            operation: "ListTagsForResource",
            body: json!({ "WorkspaceInstanceId": "wsinst-1234567" }),
            message: format!(
                "2 validation errors detected: Value at 'workspaceInstanceId' failed to satisfy \
                 constraint: Member must have length greater than or equal to 15; Value at \
                 'workspaceInstanceId' failed to satisfy constraint: Member must satisfy regular \
                 expression pattern: {ID_PATTERN}"
            ),
        },
        Case {
            name: "workspaceInstanceId longer than the modelled maximum",
            operation: "TagResource",
            body: json!({ "WorkspaceInstanceId": long_id, "Tags": [] }),
            message: format!(
                "2 validation errors detected: Value at 'workspaceInstanceId' failed to satisfy \
                 constraint: Member must have length less than or equal to 70; Value at \
                 'workspaceInstanceId' failed to satisfy constraint: Member must satisfy regular \
                 expression pattern: {ID_PATTERN}"
            ),
        },
        // `@required`: the one class that still reports a value, because `null`
        // is the shape of the request rather than something the caller sent.
        Case {
            name: "a required member absent",
            operation: "GetWorkspaceInstance",
            body: json!({}),
            message: "1 validation error detected: Value null at 'workspaceInstanceId' failed to \
                      satisfy constraint: Member must not be null"
                .to_string(),
        },
        // A `@sensitive` member: the pattern is advertised, the token is not.
        Case {
            name: "clientToken outside the printable-ASCII pattern",
            operation: "CreateWorkspaceInstance",
            body: json!({ "ClientToken": "tab\there", "ManagedInstance": {} }),
            message: format!(
                "1 validation error detected: Value at 'clientToken' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: \
                 {CLIENT_TOKEN_PATTERN}"
            ),
        },
        // A refused scalar LIST ELEMENT: reported against the list member, with
        // the element's constraint nested.
        Case {
            name: "a tagKeys element outside the TagKey pattern",
            operation: "UntagResource",
            body: json!({ "WorkspaceInstanceId": valid_id(23), "TagKeys": ["no questions?"] }),
            message: format!(
                "1 validation error detected: Value at 'tagKeys' failed to satisfy constraint: \
                 Member must satisfy constraint: [Member must satisfy regular expression \
                 pattern: {TAG_KEY_PATTERN}]"
            ),
        },
        // An element failing TWO of its constraints nests each of them.
        Case {
            name: "an empty tagKeys element",
            operation: "UntagResource",
            body: json!({ "WorkspaceInstanceId": valid_id(23), "TagKeys": [""] }),
            message: format!(
                "2 validation errors detected: Value at 'tagKeys' failed to satisfy constraint: \
                 Member must satisfy constraint: [Member must have length greater than or equal \
                 to 1]; Value at 'tagKeys' failed to satisfy constraint: Member must satisfy \
                 constraint: [Member must satisfy regular expression pattern: {TAG_KEY_PATTERN}]"
            ),
        },
        // The COLLECTION's own `@length` is a violation of the collection, not of
        // an element, so it is NOT nested: `TagKeyList` is `length 1..50`.
        Case {
            name: "an empty tagKeys list",
            operation: "UntagResource",
            body: json!({ "WorkspaceInstanceId": valid_id(23), "TagKeys": [] }),
            message: "1 validation error detected: Value at 'tagKeys' failed to satisfy \
                      constraint: Member must have length greater than or equal to 1"
                .to_string(),
        },
        // A member INSIDE a structure element keeps its own full path — the
        // collapse is for a violation ON the element, not for everything under a
        // list.
        Case {
            name: "a tag key inside a structure element",
            operation: "CreateWorkspaceInstance",
            body: json!({
                "ManagedInstance": {},
                "Tags": [{ "Key": "no questions?", "Value": "v" }],
            }),
            message: format!(
                "1 validation error detected: Value at 'tags.1.member.key' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: {TAG_KEY_PATTERN}"
            ),
        },
    ];

    for case in cases {
        let Case {
            name,
            operation,
            body,
            message,
        } = case;
        let actual = refusal_message(&router, name, operation, &body).await;
        assert_eq!(
            actual, message,
            "case {name:?} on {operation}: expected {message:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_refused_value_is_never_echoed_into_the_response() {
    let router = router();
    // One row per constraint class and per member kind, each carrying a value
    // distinctive enough that its presence anywhere in the body is unambiguous.
    let cases: Vec<(&str, &str, Value, Vec<String>)> = vec![
        (
            "a rejected integer",
            "ListRegions",
            json!({ "MaxResults": 31_337 }),
            vec!["31337".to_string()],
        ),
        (
            "a rejected id",
            "GetWorkspaceInstance",
            json!({ "WorkspaceInstanceId": "wsinst-NOT.A.LEGAL.SUFFIX" }),
            vec!["NOT.A.LEGAL.SUFFIX".to_string()],
        ),
        (
            "a rejected sensitive token",
            "CreateWorkspaceInstance",
            json!({ "ClientToken": "secret\u{7f}token", "ManagedInstance": {} }),
            vec!["secret".to_string(), "token".to_string()],
        ),
        (
            "a rejected enum element",
            "ListWorkspaceInstances",
            json!({ "ProvisionStates": ["NOT_A_REAL_STATE"] }),
            vec!["NOT_A_REAL_STATE".to_string()],
        ),
        (
            "a rejected tag key element",
            "UntagResource",
            json!({ "WorkspaceInstanceId": valid_id(23), "TagKeys": ["why?not"] }),
            vec!["why?not".to_string()],
        ),
        (
            "a rejected tag key inside a structure element",
            "CreateWorkspaceInstance",
            json!({ "ManagedInstance": {}, "Tags": [{ "Key": "why?not", "Value": "v" }] }),
            vec!["why?not".to_string()],
        ),
        (
            "a long rejected value, which the framework would elide rather than drop",
            "GetWorkspaceInstance",
            json!({ "WorkspaceInstanceId": format!("wsinst-{}", "?".repeat(400)) }),
            vec!["?".repeat(8), "bytes".to_string()],
        ),
    ];

    for (name, operation, body, rejected) in cases {
        let (status, raw) = call(&router, operation, &body).await;
        assert_eq!(
            status, 400,
            "case {name:?} on {operation}: expected HTTP 400, got {status} with {raw}"
        );
        let code = error_code(&raw);
        assert_eq!(
            code, "ValidationException",
            "case {name:?} on {operation}: expected ValidationException, got {code:?} with {raw}"
        );
        for fragment in rejected {
            assert!(
                !raw.contains(&fragment),
                "case {name:?} on {operation}: the response echoes the rejected \
                 {fragment:?}: {raw}"
            );
        }
    }
}

#[tokio::test]
async fn a_refused_collection_element_is_reported_against_the_collection_member() {
    let router = router();
    // The nested form, asserted in halves: the wrapper and the path exactly, and
    // the enum value SET for the values it must enumerate. The ORDER the set is
    // listed in is not pinned — nothing in the model or the docs fixes it.
    let opening = "1 validation error detected: Value at 'provisionStates' failed to satisfy \
                   constraint: Member must satisfy constraint: [Member must satisfy enum value \
                   set: [";
    // The value set generated from the model
    // (`smithy_query(query="shape",shape="ProvisionStateEnum")`).
    let modelled: BTreeSet<&str> = wsinst::ops::types::PROVISION_STATE_ENUM_VALUES
        .iter()
        .copied()
        .collect();

    for (name, states) in [
        ("one bad element", vec!["NOT_A_REAL_STATE"]),
        (
            "a bad element after a good one",
            vec!["ALLOCATED", "NOT_A_REAL_STATE"],
        ),
        (
            "a bad element before a good one",
            vec!["NOT_A_REAL_STATE", "ALLOCATED"],
        ),
        (
            "several bad elements, which name the member once",
            vec!["NOT_A_REAL_STATE", "ALSO_NOT_REAL", "STILL_NOT_REAL"],
        ),
    ] {
        let body = json!({ "ProvisionStates": states });
        let message = refusal_message(&router, name, "ListWorkspaceInstances", &body).await;
        let listed = message
            .strip_prefix(opening)
            .and_then(|rest| rest.strip_suffix("]]"))
            .unwrap_or_else(|| {
                panic!("case {name:?}: expected a message opening {opening:?}, got {message:?}")
            });
        let listed: BTreeSet<&str> = listed.split(", ").collect();
        assert_eq!(
            listed, modelled,
            "case {name:?}: expected the modelled value set {modelled:?}, got {listed:?}"
        );
        // No element index, and no step into the element, anywhere in the path.
        for spelling in ["provisionStates.", ".member", "1.member"] {
            assert!(
                !message.contains(spelling),
                "case {name:?}: the message reports an element path {spelling:?}: {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_value_inside_every_published_bound_is_not_refused() {
    let router = router();
    // Each published bound of each constrained input member, plus a value between
    // them: a refusal here would mean the formatter's rule was applied to a
    // request the model accepts.
    let cases: Vec<(&str, &str, Value)> = vec![
        ("maxResults at the 1..25 minimum", "ListRegions", json!({ "MaxResults": 1 })),
        ("maxResults at the 1..25 maximum", "ListRegions", json!({ "MaxResults": 25 })),
        (
            "maxResults at the 1..25 maximum, on a served operation",
            "ListWorkspaceInstances",
            json!({ "MaxResults": 25 }),
        ),
        (
            "maxResults at the 1..600 minimum",
            "ListInstanceTypes",
            json!({ "MaxResults": 1 }),
        ),
        (
            "maxResults at the 1..600 maximum",
            "ListInstanceTypes",
            json!({ "MaxResults": 600 }),
        ),
        (
            "an id at the len 15..70 minimum",
            "GetWorkspaceInstance",
            json!({ "WorkspaceInstanceId": valid_id(15) }),
        ),
        (
            "an id at the len 15..70 maximum",
            "GetWorkspaceInstance",
            json!({ "WorkspaceInstanceId": valid_id(70) }),
        ),
        (
            "every modelled provision state",
            "ListWorkspaceInstances",
            json!({ "ProvisionStates": wsinst::ops::types::PROVISION_STATE_ENUM_VALUES }),
        ),
        (
            "a tagKeys list at the length 1..50 minimum",
            "UntagResource",
            json!({ "WorkspaceInstanceId": valid_id(23), "TagKeys": ["Kéy.1:a/b=c+d-e@f"] }),
        ),
    ];

    for (name, operation, body) in cases {
        let (status, raw) = call(&router, operation, &body).await;
        let code = error_code(&raw);
        assert_ne!(
            code, "ValidationException",
            "case {name:?} on {operation}: a value inside the modelled bounds was refused \
             ({status}): {raw}"
        );
        // Whatever the operation then does with the accepted request — serve it
        // (200), report the resource absent (this service answers
        // `ResourceNotFoundException` with HTTP 400, see
        // `crate::state::RESOURCE_NOT_FOUND_STATUS`) or answer the scaffold's 501 —
        // it is past the boundary and no refusal of any kind was rendered.
        let past_boundary =
            matches!(status, 200 | 501) || (status == 400 && code == "ResourceNotFoundException");
        assert!(
            past_boundary,
            "case {name:?} on {operation}: expected the request to get past the boundary \
             (200, a not-found, or 501), got {status} with {raw}"
        );
    }
}
