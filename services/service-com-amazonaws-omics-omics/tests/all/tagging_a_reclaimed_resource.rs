// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the refusal the ARN-addressed tagging path answers when the
//! resource its `resourceArn` resolves to is not one this service holds any more.
//!
//! The rule under test: the three operations bound to `/tags/{resourceArn}` refuse an ARN
//! that names a resource of this service which no record here holds — above all one a
//! delete has already reclaimed — on the REQUEST, with HTTP 400 `ValidationException`
//! whose message is one sentence of entirely fixed text, while the resource's OWN reader
//! answers the very same resource with its resource-level HTTP 404
//! `ResourceNotFoundException`. The two paths deliberately do not answer alike, so a
//! caller distinguishing "bad input" from "resource gone" sees what the real service says.
//!
//! The cases below vary along everything the rule generalises over — which KIND of
//! resource was reclaimed (and so which shape of ARN is presented, including a version
//! named inside its workflow's own resource path), which of the three tagging operations
//! the ARN is presented to, and how many tags the resource carried before it went away —
//! and assert the one status, code and sentence for all of them, against the production
//! router, restJson1 serialization, the modelled-constraint walk and the handlers.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::private_workflow_runs::{
    call, created_id, emulator, error_code, get_workflow, workflow_for_runs,
};

/// The sentence the tagging path states when the ARN it was given resolves to a resource
/// this service does not hold. Entirely fixed text: it carries no ARN, name or id, so one
/// sentence serves every operation and every kind of resource.
///
/// Source: the recorded `ValidationException` of a tagging call against the ARN of a
/// resource already deleted — written out here rather than read from the emulator's own
/// constant, so the test pins the wording independently.
const TAGGING_NOT_FOUND_SENTENCE: &str = "Resource was not found";

/// The status a successful `TagResource` / `UntagResource` answers: both model no output
/// member and declare `@http(code: 204)`.
/// Source: `smithy_query(query="operation",operation="TagResource,UntagResource")`.
const TAG_MUTATION_STATUS: u16 = 204;

/// Percent-encode a value the way an AWS SDK encodes a non-greedy `@httpLabel`, so an
/// ARN's `:` and `/` arrive escaped and `/tags/{resourceArn}` stays one path segment.
fn encode(value: &str) -> String {
    use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

/// `GET /tags/{resourceArn}` — `ListTagsForResource`.
async fn list_tags(router: &MultiServiceRouter, resource_arn: &str) -> (u16, Value) {
    call(router, "GET", &format!("/tags/{}", encode(resource_arn)), None).await
}

/// `POST /tags/{resourceArn}` — `TagResource`.
async fn tag_resource(
    router: &MultiServiceRouter,
    resource_arn: &str,
    tags: &Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/tags/{}", encode(resource_arn)),
        Some(&json!({ "tags": tags })),
    )
    .await
}

/// `DELETE /tags/{resourceArn}?tagKeys=…` — `UntagResource`; `tagKeys` is an `@httpQuery`
/// list, so each key travels as its own repeated parameter.
async fn untag_resource(
    router: &MultiServiceRouter,
    resource_arn: &str,
    tag_keys: &[&str],
) -> (u16, Value) {
    let query = tag_keys
        .iter()
        .map(|key| format!("tagKeys={}", encode(key)))
        .collect::<Vec<_>>()
        .join("&");
    call(
        router,
        "DELETE",
        &format!("/tags/{}?{query}", encode(resource_arn)),
        None,
    )
    .await
}

/// A tag map as a request body carries it.
fn tags_body(pairs: &[(&str, &str)]) -> Value {
    let mut map = serde_json::Map::new();
    for (key, value) in pairs {
        map.insert((*key).to_string(), json!(*value));
    }
    Value::Object(map)
}

/// The `arn` member of a create or read response.
fn arn_of(case: &str, body: &Value) -> String {
    body.get("arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: response carried no arn, got {body:?}"))
        .to_string()
}

/// One resource that was created, tagged and then reclaimed: the ARN a tagging call
/// addresses it by, and the path of its own reader.
struct Reclaimed {
    /// How the row is reported in a failure.
    case: &'static str,
    /// The ARN the resource's own create (or read) answered.
    arn: String,
    /// The resource's own reader, which answers the resource-level refusal.
    reader: String,
}

/// Assert `status`/`body` is the tagging path's missing-resource refusal.
fn assert_tagging_refusal(case: &str, operation: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?} / {operation}: expected 400, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?} / {operation}: expected ValidationException, got {body:?}"
    );
    assert_eq!(
        body.get("message").and_then(Value::as_str),
        Some(TAGGING_NOT_FOUND_SENTENCE),
        "case {case:?} / {operation}: expected {TAGGING_NOT_FOUND_SENTENCE:?}, got {body:?}"
    );
}

/// Create one resource of each kind this emulator both mints and reclaims, tag each while
/// it is still held (so the record the refusals are about really carried tags), and then
/// delete it. Each row carries the ARN its create answered and its own reader's path.
async fn reclaimed_resources(router: &MultiServiceRouter) -> Vec<Reclaimed> {
    let mut rows = Vec::new();

    // A run group: the plain `runGroup/<id>` ARN shape.
    let (status, group) = call(
        router,
        "POST",
        "/runGroup",
        Some(&json!({ "requestId": "reclaimed-group", "name": "reclaimed" })),
    )
    .await;
    let group_id = created_id("a run group", status, &group);
    rows.push(Reclaimed {
        case: "a deleted run group",
        arn: arn_of("a run group", &group),
        reader: format!("/runGroup/{group_id}"),
    });

    // A sequence store: a store ARN, whose id is drawn from a wider modelled width.
    let (status, store) = call(
        router,
        "POST",
        "/sequencestore",
        Some(&json!({ "name": "the reclaimed destination" })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateSequenceStore must answer 200, got {store:?}"
    );
    let store_id = store
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("CreateSequenceStore carried no id, got {store:?}"))
        .to_string();
    rows.push(Reclaimed {
        case: "a deleted sequence store",
        arn: arn_of("a sequence store", &store),
        reader: format!("/sequencestore/{store_id}"),
    });

    // A workflow version: named INSIDE its workflow's resource path, which is the other
    // shape of ARN this surface resolves. Its workflow stays held, so the version's ARN is
    // the only thing that went away.
    let versioned_id = workflow_for_runs(router, "reclaimed-version-owner").await;
    let (status, version) = call(
        router,
        "POST",
        &format!("/workflow/{versioned_id}/version"),
        Some(&json!({ "versionName": "1.0.0", "requestId": "reclaimed-version" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateWorkflowVersion must answer 201, got {version:?}"
    );
    rows.push(Reclaimed {
        case: "a deleted workflow version",
        arn: arn_of("a workflow version", &version),
        reader: format!("/workflow/{versioned_id}/version/1.0.0"),
    });

    // A whole workflow, holding no version and no run, so its delete is not blocked.
    let workflow_id = workflow_for_runs(router, "reclaimed-workflow").await;
    let (status, workflow) = get_workflow(router, &workflow_id).await;
    assert_eq!(status, 200, "GetWorkflow must answer 200, got {workflow:?}");
    rows.push(Reclaimed {
        case: "a deleted workflow",
        arn: arn_of("a workflow", &workflow),
        reader: format!("/workflow/{workflow_id}"),
    });

    // Every row carries tags before it goes away, in differing numbers, so the refusals
    // below are about records that really held a tag map rather than about empty ones.
    for (index, row) in rows.iter().enumerate() {
        let mut pairs = vec![("env", "test")];
        if index % 2 == 1 {
            pairs.push(("owner", "lab"));
        }
        let (status, body) = tag_resource(router, &row.arn, &tags_body(&pairs)).await;
        assert_eq!(
            status, TAG_MUTATION_STATUS,
            "case {:?}: expected {TAG_MUTATION_STATUS} tagging the held resource, got \
             {status} with {body:?}",
            row.case
        );
    }

    // And each is reclaimed through its own delete.
    for row in &rows {
        let (status, body) = call(router, "DELETE", &row.reader, None).await;
        assert!(
            (200..300).contains(&status),
            "case {:?}: expected the delete of {} to succeed, got {status} with {body:?}",
            row.case,
            row.reader
        );
    }

    rows
}

#[tokio::test]
async fn every_tagging_operation_refuses_a_reclaimed_resources_arn_on_the_request() {
    let router = emulator();
    let rows = reclaimed_resources(&router).await;

    for row in &rows {
        for (operation, (status, body)) in [
            (
                "TagResource",
                tag_resource(&router, &row.arn, &tags_body(&[("any", "value")])).await,
            ),
            ("ListTagsForResource", list_tags(&router, &row.arn).await),
            (
                "UntagResource",
                untag_resource(&router, &row.arn, &["env"]).await,
            ),
        ] {
            assert_tagging_refusal(row.case, operation, status, &body);
        }
    }
}

#[tokio::test]
async fn a_reclaimed_resources_own_reader_still_answers_the_resource_level_refusal() {
    let router = emulator();
    let rows = reclaimed_resources(&router).await;

    // The same resource, addressed by its own id instead of by its ARN: the reader states
    // the resource-level 404, which is what makes the tagging path's 400 a different
    // answer rather than the same one worded twice.
    for row in &rows {
        let (status, body) = call(&router, "GET", &row.reader, None).await;
        assert_eq!(
            status, 404,
            "case {:?}: expected 404 from {}, got {status} with {body:?}",
            row.case, row.reader
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {:?}: expected ResourceNotFoundException from {}, got {body:?}",
            row.case,
            row.reader
        );

        let (tag_status, tag_body) = list_tags(&router, &row.arn).await;
        assert_ne!(
            tag_status, status,
            "case {:?}: expected the tagging path and {} to answer different statuses, \
             both gave {status} ({tag_body:?})",
            row.case, row.reader
        );
    }
}

#[tokio::test]
async fn a_resource_still_held_keeps_tagging_after_another_is_reclaimed() {
    let router = emulator();

    // A control resource created BEFORE the reclaimed ones and left alone: the refusals
    // above are about the resource the ARN names, not about the tagging path as a whole.
    let (status, control) = call(
        &router,
        "POST",
        "/runGroup",
        Some(&json!({ "requestId": "held-group", "name": "held" })),
    )
    .await;
    let control_arn = arn_of("the held run group", &control);
    assert_eq!(status, 201, "CreateRunGroup must answer 201, got {control:?}");

    let rows = reclaimed_resources(&router).await;
    for row in &rows {
        let (status, body) = tag_resource(&router, &row.arn, &tags_body(&[("k", "v")])).await;
        assert_tagging_refusal(row.case, "TagResource", status, &body);
    }

    let (status, body) = tag_resource(&router, &control_arn, &tags_body(&[("k", "v")])).await;
    assert_eq!(
        status, TAG_MUTATION_STATUS,
        "the held run group: expected {TAG_MUTATION_STATUS} from TagResource, got {status} \
         with {body:?}"
    );
    let (status, body) = list_tags(&router, &control_arn).await;
    assert_eq!(
        status, 200,
        "the held run group: expected 200 from ListTagsForResource, got {status} with {body:?}"
    );
    assert_eq!(
        body.get("tags").and_then(|tags| tags.get("k")),
        Some(&json!("v")),
        "the held run group: expected its own tags to be reported, got {body:?}"
    );
}
