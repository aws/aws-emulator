// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for this service's resource-tagging surface: `TagResource`,
//! `ListTagsForResource` and `UntagResource`.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes. The HTTP harness is the one
//! `private_workflow_runs` owns; only the tagging helpers live here.
//!
//! The rule under test: the three tagging operations address ONE per-resource tag map,
//! held on the resource the request's `resourceArn` names, whatever KIND of resource that
//! is — `TagResource` merges the request's tags into it (a key already there takes the new
//! value, every other key stays), `ListTagsForResource` reports it, and `UntagResource`
//! removes exactly the keys the request lists. Each row below is therefore a resource
//! kind, which is what the rule generalises over:
//!
//! * every kind's tags read back through `ListTagsForResource` and through the resource's
//!   own `Get`, which answers from the same record;
//! * one resource's tags are its own: tagging or untagging another leaves them alone;
//! * an ARN naming no resource of this service at all is refused as invalid, and so is one
//!   naming a resource this emulator does not hold — an id nothing holds, or a held id in
//!   another Region or account — which this ARN-addressed path refuses on the request with
//!   its own fixed sentence, from each of the three operations;
//! * the documented tags-per-resource ceiling is a quota: a merge past it is refused and
//!   leaves the resource's tags as they were, while giving a key already there a new value
//!   needs no room.

use std::collections::BTreeMap;

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Map, Value};
use service_com_amazonaws_omics_omics::state::MAX_TAGS;
use smithy_server_runtime::MultiServiceRouter;

use crate::private_workflow_runs::{
    call, created_id, emulator, error_code, start_run, start_run_body, tag_map,
    workflow_for_runs, ACCOUNT, REGION,
};

/// The status a successful `TagResource` / `UntagResource` answers, and the empty body
/// that goes with it: `TagResourceResponse` and `UntagResourceResponse` model no member
/// and both operations declare `@http(code: 204)`.
/// Source: `smithy_query(query="operation",operation="TagResource,UntagResource")`.
const TAG_MUTATION_STATUS: u16 = 204;

/// The sentence the ARN-addressed tagging path states when the `resourceArn` it was given
/// resolves to a resource this service does not hold. Entirely FIXED text: it carries no
/// ARN, name or id, so the three operations state it identically whichever kind of
/// resource the ARN named.
/// Source: the sentence this service's tagging path answers that condition with,
/// `state::TAGGING_RESOURCE_NOT_FOUND_MESSAGE`.
const TAGGING_NOT_FOUND_SENTENCE: &str = "Resource was not found";

/// Percent-encode `value` the way an AWS SDK encodes a non-greedy `@httpLabel` or a query
/// value: everything outside RFC 3986's unreserved set is escaped, so an ARN's `:` and `/`
/// arrive as `%XX` and `/tags/{resourceArn}` stays one path segment. `NON_ALPHANUMERIC`
/// also escapes `-._~`, which a server decodes to the same bytes.
fn encode(value: &str) -> String {
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

/// `DELETE /tags/{resourceArn}?tagKeys=…` — `UntagResource`. `tagKeys` is an
/// `@httpQuery` list, so each key travels as its own repeated parameter.
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

/// A tag map as the request body carries it.
fn tags_body(pairs: &[(&str, &str)]) -> Value {
    let mut map = Map::new();
    for (key, value) in pairs {
        map.insert((*key).to_string(), json!(*value));
    }
    Value::Object(map)
}

/// The same pairs as the order-independent map an assertion compares against.
fn tags(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// A JSON tag object as an order-independent map.
fn as_map(case: &str, tags: &Value) -> BTreeMap<String, String> {
    tags.as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected a tag object, got {tags:?}"))
        .iter()
        .map(|(key, value)| {
            let value = value
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: tag {key} is not a string: {value:?}"));
            (key.clone(), value.to_string())
        })
        .collect()
}

/// The tags `ListTagsForResource` reports for `resource_arn`, after asserting the 200.
/// `tags` is `@required`, so a resource carrying none answers an empty map rather than
/// omitting the member.
async fn tags_listed(
    router: &MultiServiceRouter,
    case: &str,
    resource_arn: &str,
) -> BTreeMap<String, String> {
    let (status, body) = list_tags(router, resource_arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from ListTagsForResource, got {status} with {body:?}"
    );
    let tags = body
        .get("tags")
        .unwrap_or_else(|| panic!("case {case:?}: response carried no tags member: {body:?}"));
    as_map(case, tags)
}

/// One taggable resource kind: the ARN the service answered for it, and its own reader.
struct Row {
    /// How the row is reported in a failure.
    case: &'static str,
    /// The ARN this resource's create (or read) answered: what a tagging call addresses.
    arn: String,
    /// The resource's own reader, when that reader's output shape models a `tags` member
    /// — the second reader a round trip is proved through. `GetSequenceStoreResponse`
    /// models none, so a store's tags are read through `ListTagsForResource` alone.
    reader: Option<String>,
}

/// The `arn` member of a create or read response.
fn arn_of(case: &str, body: &Value) -> String {
    body.get("arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: response carried no arn, got {body:?}"))
        .to_string()
}

/// One resource of every kind this emulator mints and this surface can address, each
/// created through its own operation and reported by the ARN that operation answered.
async fn taggable_resources(router: &MultiServiceRouter) -> Vec<Row> {
    let mut rows = Vec::new();

    let workflow_id = workflow_for_runs(router, "tagged-workflow").await;
    let (status, workflow) = call(router, "GET", &format!("/workflow/{workflow_id}"), None).await;
    assert_eq!(status, 200, "GetWorkflow must answer 200, got {workflow:?}");
    rows.push(Row {
        case: "a private workflow",
        arn: arn_of("a private workflow", &workflow),
        reader: Some(format!("/workflow/{workflow_id}")),
    });

    // A version is named INSIDE its workflow's resource path, so its ARN is the other
    // shape of ARN this surface resolves.
    let (status, version) = call(
        router,
        "POST",
        &format!("/workflow/{workflow_id}/version"),
        Some(&json!({ "versionName": "1.0.0", "requestId": "tagged-version" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateWorkflowVersion must answer 201, got {version:?}"
    );
    rows.push(Row {
        case: "a version of that workflow",
        arn: arn_of("a version of that workflow", &version),
        reader: Some(format!("/workflow/{workflow_id}/version/1.0.0")),
    });

    let (status, run) = start_run(
        router,
        &start_run_body(&workflow_id, "tagged-run", &json!({})),
    )
    .await;
    let run_id = created_id("a run", status, &run);
    rows.push(Row {
        case: "a run",
        arn: arn_of("a run", &run),
        reader: Some(format!("/run/{run_id}")),
    });

    let (status, group) = call(
        router,
        "POST",
        "/runGroup",
        Some(&json!({ "requestId": "tagged-group", "name": "tagged" })),
    )
    .await;
    let group_id = created_id("a run group", status, &group);
    rows.push(Row {
        case: "a run group",
        arn: arn_of("a run group", &group),
        reader: Some(format!("/runGroup/{group_id}")),
    });

    let (status, cache) = call(
        router,
        "POST",
        "/runCache",
        Some(&json!({
            "requestId": "tagged-cache",
            "cacheS3Location": "s3://omics-emulator-cache/tagged/",
        })),
    )
    .await;
    let cache_id = created_id("a run cache", status, &cache);
    rows.push(Row {
        case: "a run cache",
        arn: arn_of("a run cache", &cache),
        reader: Some(format!("/runCache/{cache_id}")),
    });

    let (status, store) = call(
        router,
        "POST",
        "/sequencestore",
        Some(&json!({ "name": "the tagged destination" })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateSequenceStore must answer 200, got {store:?}"
    );
    rows.push(Row {
        case: "a sequence store",
        arn: arn_of("a sequence store", &store),
        reader: None,
    });

    rows
}

/// Assert the resource's OWN reader reports exactly `expected` — and, when `expected` is
/// empty, that it reports no `tags` member at all, which is how every reader of this
/// service omits a map nothing was ever put in.
async fn assert_reader_agrees(
    router: &MultiServiceRouter,
    row: &Row,
    stage: &str,
    expected: &BTreeMap<String, String>,
) {
    let Some(reader) = row.reader.as_deref() else {
        return;
    };
    let case = format!("{} / {stage}", row.case);
    let (status, body) = call(router, "GET", reader, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {reader}, got {status} with {body:?}"
    );
    match body.get("tags") {
        Some(reported) => assert_eq!(
            &as_map(&case, reported),
            expected,
            "case {case:?}: expected {reader} to report {expected:?}, got {reported:?}"
        ),
        None => assert!(
            expected.is_empty(),
            "case {case:?}: expected {reader} to report {expected:?}, got no tags member \
             in {body:?}"
        ),
    }
}

#[tokio::test]
async fn tags_round_trip_through_every_taggable_resource_kind() {
    let router = emulator();
    let rows = taggable_resources(&router).await;

    for (index, row) in rows.iter().enumerate() {
        // Nothing is tagged to begin with, and the required member says so with an empty
        // map rather than by being absent.
        assert_eq!(
            tags_listed(&router, row.case, &row.arn).await,
            BTreeMap::new(),
            "case {:?}: expected no tags before anything tagged it",
            row.case
        );

        // A first write: exactly what the request carried, through both readers.
        let owner = format!("lab{index}");
        let first = tags(&[("env", "test"), ("owner", &owner)]);
        let (status, body) =
            tag_resource(&router, &row.arn, &tags_body(&[("env", "test"), ("owner", &owner)]))
                .await;
        assert_eq!(
            status, TAG_MUTATION_STATUS,
            "case {:?}: expected {TAG_MUTATION_STATUS} from TagResource, got {status} \
             with {body:?}",
            row.case
        );
        assert_eq!(
            tags_listed(&router, row.case, &row.arn).await,
            first,
            "case {:?}: expected the tags just written",
            row.case
        );
        assert_reader_agrees(&router, row, "first write", &first).await;

        // A second write MERGES: `owner` takes the new value, `env` stays, `stage` joins.
        let (status, body) = tag_resource(
            &router,
            &row.arn,
            &tags_body(&[("owner", "core"), ("stage", "beta")]),
        )
        .await;
        assert_eq!(
            status, TAG_MUTATION_STATUS,
            "case {:?}: expected {TAG_MUTATION_STATUS} from the merging TagResource, got \
             {status} with {body:?}",
            row.case
        );
        let merged = tags(&[("env", "test"), ("owner", "core"), ("stage", "beta")]);
        assert_eq!(
            tags_listed(&router, row.case, &row.arn).await,
            merged,
            "case {:?}: expected the second write to merge, not to replace the map",
            row.case
        );
        assert_reader_agrees(&router, row, "merged", &merged).await;

        // An untag removes exactly the keys it lists; a key the resource never carried is
        // nothing to remove, so the same call repeated answers the same way and leaves
        // the same tags behind.
        let remaining = tags(&[("env", "test"), ("stage", "beta")]);
        for attempt in ["untag", "the same untag again"] {
            let (status, body) =
                untag_resource(&router, &row.arn, &["owner", "never-set"]).await;
            assert_eq!(
                status, TAG_MUTATION_STATUS,
                "case {:?} / {attempt}: expected {TAG_MUTATION_STATUS} from \
                 UntagResource, got {status} with {body:?}",
                row.case
            );
            assert_eq!(
                tags_listed(&router, row.case, &row.arn).await,
                remaining,
                "case {:?} / {attempt}: expected only the listed key removed",
                row.case
            );
        }
        assert_reader_agrees(&router, row, "untagged", &remaining).await;

        // Removing the rest empties the map, and every reader says so.
        let (status, body) = untag_resource(&router, &row.arn, &["env", "stage"]).await;
        assert_eq!(
            status, TAG_MUTATION_STATUS,
            "case {:?}: expected {TAG_MUTATION_STATUS} from the final UntagResource, got \
             {status} with {body:?}",
            row.case
        );
        assert_eq!(
            tags_listed(&router, row.case, &row.arn).await,
            BTreeMap::new(),
            "case {:?}: expected no tags once every key was removed",
            row.case
        );
        assert_reader_agrees(&router, row, "emptied", &BTreeMap::new()).await;
    }
}

#[tokio::test]
async fn each_resources_tags_are_its_own() {
    let router = emulator();
    let rows = taggable_resources(&router).await;

    // One distinct tag per resource, so a map reaching the wrong record is visible.
    let mut expected = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let value = format!("value{index}");
        let (status, body) =
            tag_resource(&router, &row.arn, &tags_body(&[("kind", &value)])).await;
        assert_eq!(
            status, TAG_MUTATION_STATUS,
            "case {:?}: expected {TAG_MUTATION_STATUS} from TagResource, got {status} \
             with {body:?}",
            row.case
        );
        expected.push(tags(&[("kind", &value)]));
    }
    for (row, want) in rows.iter().zip(&expected) {
        assert_eq!(
            &tags_listed(&router, row.case, &row.arn).await,
            want,
            "case {:?}: expected each resource to report only its own tags",
            row.case
        );
    }

    // Untagging the first resource leaves every other resource's tags where they were.
    let (status, body) = untag_resource(&router, &rows[0].arn, &["kind"]).await;
    assert_eq!(
        status, TAG_MUTATION_STATUS,
        "case {:?}: expected {TAG_MUTATION_STATUS} from UntagResource, got {status} \
         with {body:?}",
        rows[0].case
    );
    assert_eq!(
        tags_listed(&router, rows[0].case, &rows[0].arn).await,
        BTreeMap::new(),
        "case {:?}: expected its own tag removed",
        rows[0].case
    );
    for (row, want) in rows.iter().zip(&expected).skip(1) {
        assert_eq!(
            &tags_listed(&router, row.case, &row.arn).await,
            want,
            "case {:?}: expected another resource's untag to leave this one alone",
            row.case
        );
    }
}

#[tokio::test]
async fn an_arn_naming_no_resource_of_this_service_is_refused() {
    let router = emulator();
    let rows = taggable_resources(&router).await;
    let held = &rows[0].arn;

    // A value satisfying the modelled `^arn:.+$` but naming no resource of this service
    // addresses nothing this API could resolve: it is refused on the input, not reported
    // as a resource that happens to be missing.
    let invalid = [
        (
            "another service's resource",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:omics-emulator-bucket"),
        ),
        (
            "an omics ARN with no resource path",
            format!("arn:aws:omics:{REGION}:{ACCOUNT}:"),
        ),
        (
            "an omics ARN naming a resource type with no id",
            format!("arn:aws:omics:{REGION}:{ACCOUNT}:workflow/"),
        ),
        (
            "an omics ARN naming a type and nothing else",
            format!("arn:aws:omics:{REGION}:{ACCOUNT}:workflow"),
        ),
        ("too few fields to be an ARN", "arn:aws:omics".to_string()),
    ];
    for (case, resource_arn) in &invalid {
        for (operation, (status, body)) in [
            ("TagResource", tag_resource(&router, resource_arn, &tags_body(&[("env", "test")])).await),
            ("ListTagsForResource", list_tags(&router, resource_arn).await),
            ("UntagResource", untag_resource(&router, resource_arn, &["env"]).await),
        ] {
            assert_eq!(
                status, 400,
                "case {case:?} / {operation}: expected 400, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?} / {operation}: expected ValidationException, got {body:?}"
            );
        }
    }

    // The resource that does exist is untouched by every refusal above.
    assert_eq!(
        tags_listed(&router, "the held workflow", held).await,
        BTreeMap::new(),
        "expected the held resource's tags to be untouched by the refusals"
    );
}

#[tokio::test]
async fn an_arn_naming_a_resource_this_emulator_does_not_hold_is_refused_on_the_request() {
    let router = emulator();
    let rows = taggable_resources(&router).await;

    let mut absent: Vec<(String, String)> = Vec::new();
    for row in &rows {
        // The same ARN in another Region, and in another account: a resource is held in
        // one Region of one account, so neither names it.
        absent.push((
            format!("{} in another Region", row.case),
            row.arn.replacen(REGION, "eu-west-1", 1),
        ));
        absent.push((
            format!("{} in another account", row.case),
            row.arn.replacen(ACCOUNT, "123456789012", 1),
        ));
    }
    // An id nothing holds, for each kind of resource ARN this surface resolves, plus a
    // resource type of this service that nothing here can create yet.
    // Source: the `arn` members of `CreateWorkflowResponse`, `CreateWorkflowVersionResponse`,
    // `StartRunResponse`, `CreateRunGroupResponse`, `CreateRunCacheResponse` and
    // `CreateSequenceStoreResponse`.
    for resource in [
        "workflow/999999999999",
        "workflow/999999999999/version/1.0.0",
        "run/999999999999",
        "runGroup/999999999999",
        "runCache/999999999999",
        "sequenceStore/9999999999",
        "variantStore/cohort_variants",
    ] {
        absent.push((
            format!("an unheld {resource}"),
            format!("arn:aws:omics:{REGION}:{ACCOUNT}:{resource}"),
        ));
    }
    // A version name its own workflow does not hold is the pair not being found too.
    absent.push((
        "a version name the held workflow does not carry".to_string(),
        format!("{}/version/9.9.9", rows[0].arn),
    ));

    for (case, resource_arn) in &absent {
        for (operation, (status, body)) in [
            ("TagResource", tag_resource(&router, resource_arn, &tags_body(&[("env", "test")])).await),
            ("ListTagsForResource", list_tags(&router, resource_arn).await),
            ("UntagResource", untag_resource(&router, resource_arn, &["env"]).await),
        ] {
            assert_eq!(
                status, 400,
                "case {case:?} / {operation}: expected 400, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?} / {operation}: expected ValidationException, got {body:?}"
            );
            assert_eq!(
                body.get("message").and_then(Value::as_str),
                Some(TAGGING_NOT_FOUND_SENTENCE),
                "case {case:?} / {operation}: expected the tagging path's own \
                 missing-resource sentence, got {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_merge_past_the_tag_quota_is_refused_and_changes_nothing() {
    let router = emulator();
    let rows = taggable_resources(&router).await;

    for row in &rows {
        // Up to the documented ceiling is accepted.
        // Source: the `tags` member of `CreateWorkflowRequest` ("You can define up to 50
        // tags for the workflow"), the ceiling `state::MAX_TAGS` states.
        let full = tag_map(MAX_TAGS);
        let (status, body) = tag_resource(&router, &row.arn, &full).await;
        assert_eq!(
            status, TAG_MUTATION_STATUS,
            "case {:?}: expected {MAX_TAGS} tags to be accepted, got {status} with {body:?}",
            row.case
        );
        let at_ceiling = as_map(row.case, &full);
        assert_eq!(
            tags_listed(&router, row.case, &row.arn).await,
            at_ceiling,
            "case {:?}: expected every one of the {MAX_TAGS} tags to be held",
            row.case
        );

        // One more FRESH key would take the resource past it, so nothing is written.
        let (status, body) = tag_resource(&router, &row.arn, &tags_body(&[("one-too-many", "x")])).await;
        assert_eq!(
            status, 402,
            "case {:?}: expected 402 past the ceiling, got {status} with {body:?}",
            row.case
        );
        assert_eq!(
            error_code(&body),
            "ServiceQuotaExceededException",
            "case {:?}: expected ServiceQuotaExceededException, got {body:?}",
            row.case
        );
        assert_eq!(
            tags_listed(&router, row.case, &row.arn).await,
            at_ceiling,
            "case {:?}: expected the refused merge to leave the tags as they were",
            row.case
        );

        // A key the resource already carries is a replacement, so it needs no room even
        // at the ceiling.
        let (status, body) = tag_resource(&router, &row.arn, &tags_body(&[("key0", "replaced")])).await;
        assert_eq!(
            status, TAG_MUTATION_STATUS,
            "case {:?}: expected a replacement at the ceiling to be accepted, got \
             {status} with {body:?}",
            row.case
        );
        let mut replaced = at_ceiling.clone();
        replaced.insert("key0".to_string(), "replaced".to_string());
        assert_eq!(
            tags_listed(&router, row.case, &row.arn).await,
            replaced,
            "case {:?}: expected the replacement to take effect and add no entry",
            row.case
        );

        // Freeing a key makes room for a fresh one again.
        let (status, body) = untag_resource(&router, &row.arn, &["key1"]).await;
        assert_eq!(
            status, TAG_MUTATION_STATUS,
            "case {:?}: expected {TAG_MUTATION_STATUS} from UntagResource, got {status} \
             with {body:?}",
            row.case
        );
        let (status, body) = tag_resource(&router, &row.arn, &tags_body(&[("one-too-many", "x")])).await;
        assert_eq!(
            status, TAG_MUTATION_STATUS,
            "case {:?}: expected the freed room to admit a fresh key, got {status} with \
             {body:?}",
            row.case
        );
        let mut after = replaced;
        after.remove("key1");
        after.insert("one-too-many".to_string(), "x".to_string());
        assert_eq!(
            tags_listed(&router, row.case, &row.arn).await,
            after,
            "case {:?}: expected the resource to hold the freed-and-refilled map",
            row.case
        );
    }
}

#[tokio::test]
async fn more_tags_than_the_modelled_map_admits_are_refused_by_the_walk() {
    let router = emulator();
    let rows = taggable_resources(&router).await;

    // The `tags` member of `TagResourceRequest` is bounded at 50 entries, so a single
    // request carrying more is refused before the handler runs — a modelled constraint,
    // not this emulator's own ceiling.
    // Source: `smithy_query(query="shape",shape="TagResourceRequest")` -> tags, length
    // 0..50.
    let (status, body) = tag_resource(&router, &rows[0].arn, &tag_map(MAX_TAGS + 1)).await;
    assert_eq!(
        status, 400,
        "expected 400 for {} tags in one request, got {status} with {body:?}",
        MAX_TAGS + 1
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "expected ValidationException for an oversized tag map, got {body:?}"
    );
    assert_eq!(
        tags_listed(&router, rows[0].case, &rows[0].arn).await,
        BTreeMap::new(),
        "expected the refused request to write nothing"
    );
}
