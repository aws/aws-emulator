// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the two error-body rules this service follows, driven over
//! the wire it really speaks: awsJson1_0, `POST /`, with
//! `X-Amz-Target: EUCMIFrontendAPIService.<OperationName>` and the SigV4 signing
//! name `workspaces-instances` in the credential scope.
//!
//! Rule 1 — a client-input refusal names the offending member path and the
//! constraint that member violated, and reproduces NONE of the value that was
//! submitted. It holds for every constrained member of every operation, so the
//! table below spans operations (`ListInstanceTypes`, `ListRegions`,
//! `ListWorkspaceInstances`, `GetWorkspaceInstance`, `DeleteWorkspaceInstance`,
//! `TagResource`), constraint kinds (`@range` minimum and maximum, `@length`
//! minimum and maximum, `@pattern`, `@required`) and bounds (each published bound
//! and the value one past it) rather than the one recorded request.
//!
//! Rule 2 — a WorkSpace Instance id that names no stored record is answered with
//! `ResourceNotFoundException`, HTTP 400 and the `ResourceType` token
//! `WORKSPACE_INSTANCE`, from every operation that looks an instance up; the
//! `Message` is request-independent and the id travels in the modelled `ResourceId`
//! member.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_workspacesinstances_eucmifrontendapiservice as wsinst;
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix from the crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "EUCMIFrontendAPIService";

/// The error code a client-input refusal carries: the modelled `ValidationException`
/// (`@httpError 400`).
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The error code a missing WorkSpace Instance carries.
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The status this service answers `ResourceNotFoundException` with, from the wire
/// recordings for an unknown `wsinst-…` id — NOT the 404 the error shape declares.
/// Pinned here rather than read out of the crate, so the test measures what goes over
/// the wire against the recorded contract instead of against itself.
const NOT_FOUND_STATUS: u16 = 400;

/// The `ResourceType` token this service reports for a WorkSpace Instance.
const WORKSPACE_INSTANCE_RESOURCE_TYPE: &str = "WORKSPACE_INSTANCE";

/// The modelled `@pattern` of `WorkspaceInstanceId` as this service ADVERTISES it:
/// `^wsinst-[0-9a-zA-Z]{8,63}$` without the anchors the model spells it with
/// (`smithy_query(query="shape",shape="WorkspaceInstanceId")`; the unanchored form is
/// the rule `src/service/validation.rs` states, covered in depth by
/// `tests/all/validation_messages.rs`).
const WORKSPACE_INSTANCE_ID_PATTERN: &str = "wsinst-[0-9a-zA-Z]{8,63}";

/// A router carrying this service and the service that owns the EC2 instance a WorkSpace
/// Instance is, over one shared engine — the same `EmulatedService::new(engine)` seam
/// `core/router` mounts both through. Amazon EC2 is mounted because a create launches a
/// real instance there.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are the emulator's concern, not this service's; the tests drive
    // unsigned requests whose credential scope still names the service.
    router.set_request_authentication(false);
    router.register_service(wsinst::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_ec2_amazonec2::EmulatedService::new(Arc::clone(&engine)),
    );
    // The network and the images the create fixtures name, in the account's own EC2.
    crate::workspace_instance_lifecycle::seed_account_network(&engine);
    router
}

/// One awsJson1_0 call. Returns the HTTP status and the parsed JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
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
    let payload = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or_else(|e| {
            panic!(
                "{operation} answered a body that is not JSON ({e}): {}",
                String::from_utf8_lossy(&raw)
            )
        })
    };
    (status, payload)
}

/// The awsJson error code: the `__type` discriminator in the error envelope.
fn error_code(payload: &Value) -> String {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// The error body's message, whichever casing the envelope spells it with: the
/// boundary refusal renders the synthetic lower-case `message`, a modelled error
/// shape its own `Message` member.
fn message(case: &str, payload: &Value) -> String {
    payload
        .get("message")
        .or_else(|| payload.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: the error body carried no message: {payload}"))
        .to_string()
}

/// The envelope a single-violation refusal states.
fn one_error(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// The clause for a member that violated `requirement`.
fn clause(member_path: &str, requirement: &str) -> String {
    format!("Value at '{member_path}' failed to satisfy constraint: {requirement}")
}

/// The clause for a member the request left out: the absence is a fact of the
/// request's shape, not a value the caller sent.
fn null_clause(member_path: &str) -> String {
    format!("Value null at '{member_path}' failed to satisfy constraint: Member must not be null")
}

/// Assert a refused request answered the modelled `ValidationException` with the
/// clause `rule 1` states, and that neither the quoted echo form the framework
/// defaults to nor the submitted value itself appears anywhere in the body.
async fn assert_refused(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: Value,
    submitted: &str,
    expected: &str,
) {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 from {operation}, got {status} with {payload}"
    );
    assert_eq!(
        error_code(&payload),
        VALIDATION_EXCEPTION,
        "case {case:?}: expected {VALIDATION_EXCEPTION}, got {payload}"
    );
    let actual = message(case, &payload);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected message {expected:?}, got {actual:?}"
    );
    let echoed = format!("Value '{submitted}'");
    assert!(
        !actual.contains(&echoed),
        "case {case:?}: the message must not reproduce the submitted value, got {actual:?}"
    );
    if submitted.len() >= 3 {
        assert!(
            !actual.contains(submitted),
            "case {case:?}: the message must not reproduce the submitted value \
             {submitted:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_refusal_names_the_member_and_the_constraint_without_the_value() {
    let router = router();

    // Each row is one constrained member of one operation, broken in one way. The
    // bounds come from the model: `ListInstanceTypesMaxResults range 1..600`,
    // `MaxResults range 1..25`, `NextToken length 1..2048`, `WorkspaceInstanceId`
    // `pattern ^wsinst-[0-9a-zA-Z]{8,63}$`
    // (`smithy_query(query="shape",shape="ListInstanceTypesMaxResults,MaxResults,NextToken,WorkspaceInstanceId")`).
    // Every row is a value one past a published bound, or outside a declared class.
    let long_token = "a".repeat(2049);
    let rows: Vec<(&str, &str, Value, String, String)> = vec![
        (
            "ListInstanceTypes MaxResults one past the maximum",
            "ListInstanceTypes",
            json!({ "MaxResults": 601 }),
            "601".to_string(),
            one_error(&clause(
                "maxResults",
                "Member must have value less than or equal to 600",
            )),
        ),
        (
            "ListInstanceTypes MaxResults far above the maximum",
            "ListInstanceTypes",
            json!({ "MaxResults": 100_000 }),
            "100000".to_string(),
            one_error(&clause(
                "maxResults",
                "Member must have value less than or equal to 600",
            )),
        ),
        (
            "ListInstanceTypes MaxResults one below the minimum",
            "ListInstanceTypes",
            json!({ "MaxResults": 0 }),
            "0".to_string(),
            one_error(&clause(
                "maxResults",
                "Member must have value greater than or equal to 1",
            )),
        ),
        (
            "ListRegions MaxResults one past the maximum",
            "ListRegions",
            json!({ "MaxResults": 26 }),
            "26".to_string(),
            one_error(&clause(
                "maxResults",
                "Member must have value less than or equal to 25",
            )),
        ),
        (
            "ListRegions MaxResults far above the maximum",
            "ListRegions",
            json!({ "MaxResults": 100 }),
            "100".to_string(),
            one_error(&clause(
                "maxResults",
                "Member must have value less than or equal to 25",
            )),
        ),
        (
            "ListRegions MaxResults one below the minimum",
            "ListRegions",
            json!({ "MaxResults": 0 }),
            "0".to_string(),
            one_error(&clause(
                "maxResults",
                "Member must have value greater than or equal to 1",
            )),
        ),
        // An operation the issue cited neither of: the same member, the same rule.
        (
            "ListWorkspaceInstances MaxResults one past the maximum",
            "ListWorkspaceInstances",
            json!({ "MaxResults": 26 }),
            "26".to_string(),
            one_error(&clause(
                "maxResults",
                "Member must have value less than or equal to 25",
            )),
        ),
        // A `@length` member, and a `@sensitive` one at that: its value must not be
        // reflected into an error body at all.
        (
            "ListRegions NextToken one past the maximum length",
            "ListRegions",
            json!({ "NextToken": long_token.clone() }),
            long_token.clone(),
            one_error(&clause(
                "nextToken",
                "Member must have length less than or equal to 2048",
            )),
        ),
        (
            "ListWorkspaceInstances NextToken below the minimum length",
            "ListWorkspaceInstances",
            json!({ "NextToken": "" }),
            String::new(),
            one_error(&clause(
                "nextToken",
                "Member must have length greater than or equal to 1",
            )),
        ),
        // A `@pattern` member: the length is legal, the character class is not.
        (
            "GetWorkspaceInstance WorkspaceInstanceId outside the pattern",
            "GetWorkspaceInstance",
            json!({ "WorkspaceInstanceId": "wsinst-abc_defgh" }),
            "wsinst-abc_defgh".to_string(),
            one_error(&clause(
                "workspaceInstanceId",
                &format!(
                    "Member must satisfy regular expression pattern: {WORKSPACE_INSTANCE_ID_PATTERN}"
                ),
            )),
        ),
        (
            "DeleteWorkspaceInstance WorkspaceInstanceId outside the pattern",
            "DeleteWorkspaceInstance",
            json!({ "WorkspaceInstanceId": "WSINST-12345678" }),
            "WSINST-12345678".to_string(),
            one_error(&clause(
                "workspaceInstanceId",
                &format!(
                    "Member must satisfy regular expression pattern: {WORKSPACE_INSTANCE_ID_PATTERN}"
                ),
            )),
        ),
        // A `@required` member the request left out.
        (
            "GetWorkspaceInstance without WorkspaceInstanceId",
            "GetWorkspaceInstance",
            json!({}),
            String::new(),
            one_error(&null_clause("workspaceInstanceId")),
        ),
        (
            "TagResource without Tags",
            "TagResource",
            json!({ "WorkspaceInstanceId": "wsinst-0123456789abc" }),
            String::new(),
            one_error(&null_clause("tags")),
        ),
    ];

    for (case, operation, body, submitted, expected) in rows {
        assert_refused(&router, case, operation, body, &submitted, &expected).await;
    }
}

#[tokio::test]
async fn a_refusal_with_several_violations_counts_its_clauses_and_echoes_nothing() {
    let router = router();

    // Two members of one request, each outside its own bound. The envelope states
    // how many clauses it carries, so the count and the clauses have to agree —
    // whatever order the validating walk finds them in.
    let too_long = "b".repeat(2049);
    let rows: Vec<(&str, &str, Value, Vec<String>)> = vec![
        (
            "ListRegions with MaxResults and NextToken both outside their bounds",
            "ListRegions",
            json!({ "MaxResults": 0, "NextToken": too_long.clone() }),
            vec![too_long.clone(), "0".to_string()],
        ),
        (
            "ListInstanceTypes with MaxResults and NextToken both outside their bounds",
            "ListInstanceTypes",
            json!({ "MaxResults": 601, "NextToken": "" }),
            vec!["601".to_string()],
        ),
        (
            "DeleteWorkspaceInstance with an id too short for both its length and its pattern",
            "DeleteWorkspaceInstance",
            json!({ "WorkspaceInstanceId": "wsinst-abc" }),
            vec!["wsinst-abc".to_string()],
        ),
    ];

    for (case, operation, body, submitted) in rows {
        let (status, payload) = call(&router, operation, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            VALIDATION_EXCEPTION,
            "case {case:?}: expected {VALIDATION_EXCEPTION}, got {payload}"
        );
        let actual = message(case, &payload);
        let (count, clauses) = match actual.split_once(" validation error detected: ") {
            Some((count, rest)) => (count.to_string(), rest.to_string()),
            None => {
                let (count, rest) = actual
                    .split_once(" validation errors detected: ")
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: the message carries no envelope: {actual:?}")
                    });
                (count.to_string(), rest.to_string())
            }
        };
        let stated: usize = count.parse().unwrap_or_else(|e| {
            panic!("case {case:?}: {count:?} is not a violation count ({e}): {actual:?}")
        });
        let found = clauses.split("; ").count();
        assert_eq!(
            stated, found,
            "case {case:?}: the envelope states {stated} violations but carries {found} \
             clauses: {actual:?}"
        );
        for one in clauses.split("; ") {
            assert!(
                one.starts_with("Value at '") || one.starts_with("Value null at '"),
                "case {case:?}: a clause must open with the member path, got {one:?}"
            );
            assert!(
                one.contains("failed to satisfy constraint: Member must "),
                "case {case:?}: a clause must state the constraint, got {one:?}"
            );
        }
        for value in submitted {
            let echoed = format!("Value '{value}'");
            assert!(
                !actual.contains(&echoed),
                "case {case:?}: the message must not reproduce the submitted value \
                 {value:?}, got {actual:?}"
            );
            // A longer value cannot collide with a bound the clause states, so it is
            // also checked raw; a one- or two-character one is covered by the quoted
            // echo form above.
            if value.len() >= 3 {
                assert!(
                    !actual.contains(&value),
                    "case {case:?}: the message must not reproduce the submitted value \
                     {value:?}, got {actual:?}"
                );
            }
        }
    }
}

/// Assert a not-found answer: the status, code, `ResourceType` token and
/// `ResourceId` of rule 2. Returns the `Message`, so a caller can check it does not
/// vary with the request.
async fn assert_not_found(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    id: &str,
    body: Value,
) -> String {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, NOT_FOUND_STATUS,
        "case {case:?}: expected HTTP {NOT_FOUND_STATUS} from {operation}, got {status} \
         with {payload}"
    );
    assert_eq!(
        error_code(&payload),
        RESOURCE_NOT_FOUND,
        "case {case:?}: expected {RESOURCE_NOT_FOUND}, got {payload}"
    );
    let resource_type = payload
        .get("ResourceType")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: the error carried no ResourceType: {payload}"));
    assert_eq!(
        resource_type, WORKSPACE_INSTANCE_RESOURCE_TYPE,
        "case {case:?}: expected the ResourceType token \
         {WORKSPACE_INSTANCE_RESOURCE_TYPE:?}, got {resource_type:?}"
    );
    // The token form itself: upper case and underscore separated, so a client
    // switching on it never has to case-fold or re-spell.
    assert!(
        resource_type
            .chars()
            .all(|c| c.is_ascii_uppercase() || c == '_')
            && !resource_type.starts_with('_'),
        "case {case:?}: {resource_type:?} is not an upper-case underscored token"
    );
    assert_eq!(
        payload.get("ResourceId").and_then(Value::as_str),
        Some(id),
        "case {case:?}: the error must name the id that was looked up, got {payload}"
    );
    let stated = message(case, &payload);
    assert!(
        !stated.is_empty(),
        "case {case:?}: ResourceNotFoundException$Message is @required and must say \
         something, got {stated:?}"
    );
    assert!(
        !stated.contains(id),
        "case {case:?}: the Message is request-independent and must not reproduce the \
         id; the modelled ResourceId carries it, got {stated:?}"
    );
    stated
}

#[tokio::test]
async fn an_unknown_instance_is_not_found_with_this_services_status_and_resource_type() {
    let router = router();

    // Well-formed per the model, stored by nobody: the shortest legal id, the longest
    // legal id, a mixed-case one, and one whose spelling the issue cited.
    let ids = [
        "wsinst-00000000".to_string(),
        format!("wsinst-{}", "z".repeat(63)),
        "wsinst-AbCdEf01234".to_string(),
        "wsinst-00000000notreal".to_string(),
    ];

    // Every operation that looks an instance up by id, not only the two the issue
    // cited. The volume operations are still stubs (HTTP 501) and belong to their own
    // issues.
    let mut messages: Vec<String> = Vec::new();
    for id in &ids {
        for (operation, body) in [
            ("GetWorkspaceInstance", json!({ "WorkspaceInstanceId": id })),
            ("DeleteWorkspaceInstance", json!({ "WorkspaceInstanceId": id })),
            ("ListTagsForResource", json!({ "WorkspaceInstanceId": id })),
            (
                "TagResource",
                json!({ "WorkspaceInstanceId": id, "Tags": [{ "Key": "env", "Value": "dev" }] }),
            ),
            (
                "UntagResource",
                json!({ "WorkspaceInstanceId": id, "TagKeys": ["env"] }),
            ),
        ] {
            let case = format!("{operation} on the unknown {id}");
            messages.push(assert_not_found(&router, &case, operation, id, body).await);
        }
    }

    // One wording for one condition: the Message does not vary with the operation or
    // with the id that was looked up.
    let first = messages.first().expect("the table is not empty").clone();
    for (index, stated) in messages.iter().enumerate() {
        assert_eq!(
            stated, &first,
            "case \"row {index}\": the not-found Message is request-independent, got \
             {stated:?} beside {first:?}"
        );
    }
}

#[tokio::test]
async fn a_torn_down_instance_is_answered_the_same_way_as_one_that_never_existed() {
    let router = router();

    let created = call(
        &router,
        "CreateWorkspaceInstance",
        json!({
            "ManagedInstance": {
                "ImageId": "ami-0123456789abcdef0",
                "InstanceType": "t3.medium",
            },
        }),
    )
    .await;
    assert_eq!(
        created.0, 200,
        "case \"create\": expected HTTP 200, got {} with {}",
        created.0, created.1
    );
    let id = created
        .1
        .get("WorkspaceInstanceId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case \"create\": no WorkspaceInstanceId: {}", created.1))
        .to_string();

    // Round trip: while it is stored, the reader answers 200 — so the not-found below
    // is the store's answer and not a canned one.
    let (status, payload) = call(
        &router,
        "GetWorkspaceInstance",
        json!({ "WorkspaceInstanceId": id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"read back a stored instance\": expected HTTP 200, got {status} with {payload}"
    );
    assert_eq!(
        payload.get("WorkspaceInstanceId").and_then(Value::as_str),
        Some(id.as_str()),
        "case \"read back a stored instance\": expected {id:?}, got {payload}"
    );

    let (status, payload) = call(
        &router,
        "DeleteWorkspaceInstance",
        json!({ "WorkspaceInstanceId": id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"delete\": expected HTTP 200, got {status} with {payload}"
    );

    // The delete starts an asynchronous teardown, so the instance is still reported
    // for a while (the `instance_deletion_lifecycle` suite spans that window). Only
    // once the teardown and its retention have both fallen due does the id name no
    // stored record, and then it is answered exactly like one that never existed.
    tokio::time::sleep(
        wsinst::state::DEALLOCATION_SPAN
            + wsinst::state::DEALLOCATED_RETENTION
            + std::time::Duration::from_millis(250),
    )
    .await;

    for operation in [
        "GetWorkspaceInstance",
        "DeleteWorkspaceInstance",
        "ListTagsForResource",
    ] {
        let case = format!("{operation} after the teardown completed");
        assert_not_found(
            &router,
            &case,
            operation,
            &id,
            json!({ "WorkspaceInstanceId": id }),
        )
        .await;
    }
}
