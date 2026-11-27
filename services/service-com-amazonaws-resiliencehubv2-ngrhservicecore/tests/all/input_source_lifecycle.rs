// SPDX-License-Identifier: CC0-1.0

//! The `InputSource` lifecycle, end to end over HTTP: `CreateInputSource` ->
//! `ListInputSources` -> `DeleteInputSource`.
//!
//! The rule under test: an input source is a child of exactly one service, identified
//! within it by a minted `inputSourceId` and by nothing else, so each create stores a new
//! source under that service carrying the one `ResourceConfiguration` member the call
//! supplied — two creates carrying IDENTICAL configuration are two distinct sources — a
//! listing projects each stored source back as that same configuration member plus the
//! `InputSourceType` that member names, a delete removes exactly the source its
//! `inputSourceId` names and leaves every other one, and a `serviceArn` or `inputSourceId`
//! naming nothing stored is refused `ResourceNotFoundException`.
//!
//! Every suite drives the real wire path (`MultiServiceRouter::dispatch`), so the modelled
//! constraints, the restJson1 (de)serializers and the HTTP status codes are exercised
//! exactly as a client would exercise them.
//!
//! The suites are TABLES: each row is one input of the class the rule covers (which
//! configuration member, its shape and size, the page size, the type filter), not one
//! recorded request.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_resiliencehubv2_ngrhservicecore::state::{
    MAX_RESULTS_MAX, MAX_RESULTS_MIN,
};
use service_com_amazonaws_resiliencehubv2_ngrhservicecore::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The modelled routes this suite drives.
const CREATE_SERVICE: &str = "/v2/create-service";
const DELETE_SERVICE: &str = "/v2/delete-service";
const CREATE_INPUT_SOURCE: &str = "/v2/create-input-source";
const DELETE_INPUT_SOURCE: &str = "/v2/delete-input-source";
const LIST_INPUT_SOURCES: &str = "/v2/list-input-sources";
/// The routes of the children that DO go with a deleted service, and of the service read
/// itself, so one suite can assert what the delete takes and what it leaves.
const CREATE_SERVICE_FUNCTION: &str = "/v2/create-service-function";
const CREATE_ASSERTION: &str = "/v2/create-assertion";
const GET_SERVICE: &str = "/v2/get-service";
const LIST_SERVICE_FUNCTIONS: &str = "/v2/list-functions";
const LIST_ASSERTIONS: &str = "/v2/list-assertions";

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (`resiliencehub`) and the Region ARNs are minted in. Signature verification is off
/// for these tests, so the value only has to parse.
const SIGNED: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTONLY00000000/20260101/us-east-1/resiliencehub/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// `InputSourceId`'s modelled bounds (`smithy_query(query="shape",
/// shape="InputSourceId")` reports `length {min: 1, max: 255}` with pattern
/// `^\S{1,255}$`), which every minted `inputSourceId` must satisfy.
const INPUT_SOURCE_ID_MIN_LENGTH: usize = 1;
const INPUT_SOURCE_ID_MAX_LENGTH: usize = 255;

/// The values `InputSourceType` accepts on the wire.
///
/// SOURCE: `smithy_query(query="shape", shape="InputSourceType")` — an enum of exactly
/// these six values. `MONITORING` is the one no `ResourceConfiguration` member names, so
/// no create can produce a source of that type.
const INPUT_SOURCE_TYPE_VALUES: [&str; 6] = [
    "CFN_STACK",
    "DESIGN_FILE",
    "EKS",
    "MONITORING",
    "TAGS",
    "TERRAFORM",
];

/// An emulator holding only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body
        .map(|value| serde_json::to_vec(&value).expect("a JSON body serializes"))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", SIGNED)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Create a service named `name` and answer its ARN.
async fn create_service(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        CREATE_SERVICE,
        Some(json!({
            "name": name,
            "regions": ["us-east-1"],
            "permissionModel": {"invokerRoleName": "ResilienceHubRole"},
        })),
    )
    .await;
    assert_eq!(status, 200, "creating service {name}: got {body:?}");
    body["service"]["serviceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no service.serviceArn in {body:?}"))
        .to_string()
}

async fn create_source(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", CREATE_INPUT_SOURCE, Some(body)).await
}

async fn delete_source(
    router: &MultiServiceRouter,
    service_arn: &str,
    input_source_id: &str,
) -> (u16, Value) {
    call(
        router,
        "POST",
        DELETE_INPUT_SOURCE,
        Some(json!({"serviceArn": service_arn, "inputSourceId": input_source_id})),
    )
    .await
}

async fn list_sources(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(router, "GET", &format!("{LIST_INPUT_SOURCES}?{query}"), None).await
}

/// The modelled error code of a refusal, from the restJson1 `__type` discriminator.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `inputSourceSummaries` array of a successful listing.
fn listed(case: &str, status: u16, body: &Value) -> Vec<Value> {
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListInputSources, got {status} {body:?}"
    );
    body["inputSourceSummaries"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the @required inputSourceSummaries member is absent from {body:?}"
            )
        })
        .clone()
}

/// The summary of `input_source_id` within a listing.
fn summary_of(case: &str, summaries: &[Value], input_source_id: &str) -> Value {
    summaries
        .iter()
        .find(|summary| summary["inputSourceId"] == json!(input_source_id))
        .unwrap_or_else(|| {
            panic!("case {case:?}: no summary for {input_source_id} in {summaries:?}")
        })
        .clone()
}

/// The `inputSourceId` a successful create minted, with the response checked against the
/// modelled output contract first.
fn minted_id(case: &str, service_arn: &str, status: u16, body: &Value) -> String {
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateInputSource, got {status} {body:?}"
    );
    assert_eq!(
        body["serviceArn"],
        json!(service_arn),
        "case {case:?}: CreateInputSource must echo the service it created the source for, \
         got {body:?}"
    );
    let id = body["inputSourceId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no inputSourceId in {body:?}"))
        .to_string();
    assert!(
        (INPUT_SOURCE_ID_MIN_LENGTH..=INPUT_SOURCE_ID_MAX_LENGTH).contains(&id.len())
            && !id.chars().any(char::is_whitespace),
        "case {case:?}: inputSourceId must satisfy InputSourceId's len \
         {INPUT_SOURCE_ID_MIN_LENGTH}..{INPUT_SOURCE_ID_MAX_LENGTH} and ^\\S{{1,255}}$, got \
         {id:?}"
    );
    id
}

/// Every configuration member `InputSourceSummary` can report, so a row can assert that
/// only the one it supplied comes back.
///
/// SOURCE: `smithy_query(query="shape", shape="InputSourceSummary")` — the summary's
/// configuration members beside `inputSourceId`, `type` and `createdAt`.
const SUMMARY_CONFIGURATION_MEMBERS: [&str; 5] = [
    "resourceTags",
    "cfnStackArn",
    "tfStateFileUrl",
    "eks",
    "designFileS3Url",
];

/// One row of the configuration table: a `resourceConfiguration` to create an input source
/// from, the `InputSourceType` it names, and the summary member it must read back as.
#[derive(Debug, Clone)]
struct Row {
    case: &'static str,
    configuration: Value,
    expected_type: &'static str,
    member: &'static str,
    expected_member: Value,
}

impl Row {
    fn body(&self, service_arn: &str) -> Value {
        json!({
            "serviceArn": service_arn,
            "resourceConfiguration": self.configuration,
        })
    }

    /// Assert `summary` is this row's stored source, projected back.
    fn assert_summary(&self, summary: &Value) {
        assert_eq!(
            summary["type"],
            json!(self.expected_type),
            "case {:?}: the reported type must be the one the configuration member names, \
             got {summary:?}",
            self.case
        );
        assert_eq!(
            summary[self.member], self.expected_member,
            "case {:?}: the summary must report the configuration as supplied, got {summary:?}",
            self.case
        );
        for other in SUMMARY_CONFIGURATION_MEMBERS
            .iter()
            .filter(|name| **name != self.member)
        {
            assert!(
                summary.get(*other).is_none(),
                "case {:?}: member {other:?} was never stored, so it must stay absent, got \
                 {summary:?}",
                self.case
            );
        }
        assert!(
            summary["createdAt"].is_number() || summary["createdAt"].is_string(),
            "case {:?}: a stored source reports when it was created, got {summary:?}",
            self.case
        );
    }
}

/// The configuration table: one row per `ResourceConfiguration` member, and rows differing
/// in the shape and size of that member (one tag versus the longest `ResourceTagList`, a
/// tag with no values, both `S3Url` forms, an EKS source with and without a label
/// selector).
///
/// SOURCE of the bounds exercised: `ResourceConfiguration`'s `resourceTags` is
/// `len 1..10`, `ResourceTag$values` is `len 0..10`, `EksSource$namespaces` is `len 1..10`
/// and `S3Url` is `^((https://([^/]+)\.s3[^/]*\.[^/]+)|(s3://([^/]+)))/\S{1,2000}$`
/// (`smithy_query(query="shape", shape="ResourceConfiguration,ResourceTag,EksSource")`).
fn configuration_rows() -> Vec<Row> {
    let longest_tag_list: Vec<Value> = (0..10)
        .map(|index| json!({"key": format!("tier-{index}"), "values": [format!("value-{index}")]}))
        .collect();
    vec![
        Row {
            case: "one resource tag with one value",
            configuration: json!({"resourceTags": [{"key": "Env", "values": ["prod"]}]}),
            expected_type: "TAGS",
            member: "resourceTags",
            expected_member: json!([{"key": "Env", "values": ["prod"]}]),
        },
        Row {
            case: "a tag with several values and a tag with none",
            configuration: json!({
                "resourceTags": [
                    {"key": "Env", "values": ["prod", "staging", "dev"]},
                    {"key": "AnyValue", "values": []},
                ]
            }),
            expected_type: "TAGS",
            member: "resourceTags",
            expected_member: json!([
                {"key": "Env", "values": ["prod", "staging", "dev"]},
                {"key": "AnyValue", "values": []},
            ]),
        },
        Row {
            case: "the longest resource tag list",
            configuration: json!({"resourceTags": longest_tag_list.clone()}),
            expected_type: "TAGS",
            member: "resourceTags",
            expected_member: json!(longest_tag_list),
        },
        Row {
            case: "a CloudFormation stack",
            configuration: json!({
                "cfnStackArn": "arn:aws:cloudformation:us-east-1:123456789012:stack/checkout/abc123"
            }),
            expected_type: "CFN_STACK",
            member: "cfnStackArn",
            expected_member: json!(
                "arn:aws:cloudformation:us-east-1:123456789012:stack/checkout/abc123"
            ),
        },
        Row {
            case: "a Terraform state file in the s3:// form",
            configuration: json!({"tfStateFileUrl": "s3://resilience-state/prod/terraform.tfstate"}),
            expected_type: "TERRAFORM",
            member: "tfStateFileUrl",
            expected_member: json!("s3://resilience-state/prod/terraform.tfstate"),
        },
        Row {
            case: "a design file in the https:// form",
            configuration: json!({
                "designFileS3Url":
                    "https://resilience-designs.s3.us-east-1.amazonaws.com/checkout/design.json"
            }),
            expected_type: "DESIGN_FILE",
            member: "designFileS3Url",
            expected_member: json!(
                "https://resilience-designs.s3.us-east-1.amazonaws.com/checkout/design.json"
            ),
        },
        Row {
            case: "an EKS cluster with one namespace and no label selector",
            configuration: json!({
                "eks": {
                    "clusterArn": "arn:aws:eks:us-east-1:123456789012:cluster/checkout",
                    "namespaces": ["default"],
                }
            }),
            expected_type: "EKS",
            member: "eks",
            expected_member: json!({
                "clusterArn": "arn:aws:eks:us-east-1:123456789012:cluster/checkout",
                "namespaces": ["default"],
            }),
        },
        Row {
            case: "an EKS cluster with several namespaces and a label selector",
            configuration: json!({
                "eks": {
                    "clusterArn": "arn:aws:eks:us-west-2:123456789012:cluster/orders",
                    "namespaces": ["orders", "payments", "shipping"],
                    "labelSelector": {
                        "matchLabels": {"app": "orders"},
                        "matchExpressions": [
                            {"key": "tier", "operator": "IN", "values": ["web", "api"]}
                        ],
                    },
                }
            }),
            expected_type: "EKS",
            member: "eks",
            expected_member: json!({
                "clusterArn": "arn:aws:eks:us-west-2:123456789012:cluster/orders",
                "namespaces": ["orders", "payments", "shipping"],
                "labelSelector": {
                    "matchLabels": {"app": "orders"},
                    "matchExpressions": [
                        {"key": "tier", "operator": "IN", "values": ["web", "api"]}
                    ],
                },
            }),
        },
    ]
}

/// A create stores the configuration it was given, and the listing projects it back with
/// the type that configuration member names.
#[tokio::test]
async fn create_stores_every_configuration_member_and_the_listing_projects_it() {
    let router = emulator();
    let service_arn = create_service(&router, "InputSourceProjection").await;

    let rows = configuration_rows();
    let mut created: Vec<(Row, String)> = Vec::new();
    for row in rows {
        let (status, body) = create_source(&router, row.body(&service_arn)).await;
        let id = minted_id(row.case, &service_arn, status, &body);
        created.push((row, id));
    }

    // Every minted id is distinct, whatever the configuration.
    let ids: BTreeSet<&String> = created.iter().map(|(_, id)| id).collect();
    assert_eq!(
        ids.len(),
        created.len(),
        "expected one distinct inputSourceId per create, got {created:?}"
    );

    // The ROUND TRIP: the listing projects the same stored records, in creation order.
    let (status, body) = list_sources(&router, &format!("serviceArn={service_arn}")).await;
    let summaries = listed("round trip", status, &body);
    assert!(
        body.get("nextToken").is_none(),
        "every source fits one default-sized page, so no nextToken is due: {body:?}"
    );
    assert_eq!(
        summaries.len(),
        created.len(),
        "the listing must enumerate every stored source, got {summaries:?}"
    );
    let listed_order: Vec<&str> = summaries
        .iter()
        .map(|summary| summary["inputSourceId"].as_str().unwrap_or_default())
        .collect();
    let created_order: Vec<&str> = created.iter().map(|(_, id)| id.as_str()).collect();
    assert_eq!(
        listed_order, created_order,
        "the listing must enumerate sources in creation order"
    );
    for (row, id) in &created {
        row.assert_summary(&summary_of(row.case, &summaries, id));
    }

    // A read leaves the store untouched, so two listings agree.
    let (status, again) = list_sources(&router, &format!("serviceArn={service_arn}")).await;
    assert_eq!(
        listed("repeated read", status, &again),
        summaries,
        "two reads of the same service's input sources must answer identically"
    );
}

/// Nothing in the configuration identifies an input source: two creates carrying identical
/// configuration are two distinct sources, and deleting one leaves the other.
#[tokio::test]
async fn identical_configuration_yields_distinct_sources_deleted_individually() {
    let router = emulator();
    let service_arn = create_service(&router, "IdenticalConfigurations").await;

    for row in configuration_rows() {
        let (status, body) = create_source(&router, row.body(&service_arn)).await;
        let first = minted_id(row.case, &service_arn, status, &body);
        let (status, body) = create_source(&router, row.body(&service_arn)).await;
        let second = minted_id(row.case, &service_arn, status, &body);
        assert_ne!(
            first, second,
            "case {:?}: identical configuration must mint two distinct input sources",
            row.case
        );

        let (status, body) = list_sources(&router, &format!("serviceArn={service_arn}")).await;
        let summaries = listed(row.case, status, &body);
        for id in [&first, &second] {
            row.assert_summary(&summary_of(row.case, &summaries, id));
        }

        // Deleting one names exactly that source, and the other is untouched.
        let kept = summary_of(row.case, &summaries, &second);
        let (status, body) = delete_source(&router, &service_arn, &first).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from DeleteInputSource, got {status} {body:?}",
            row.case
        );
        assert_eq!(
            body,
            json!({"serviceArn": service_arn, "inputSourceId": first}),
            "case {:?}: DeleteInputSource must answer the source it deleted, got {body:?}",
            row.case
        );

        let (status, body) = list_sources(&router, &format!("serviceArn={service_arn}")).await;
        let summaries = listed(row.case, status, &body);
        assert!(
            !summaries
                .iter()
                .any(|summary| summary["inputSourceId"] == json!(first)),
            "case {:?}: the deleted source must be gone from the listing, got {summaries:?}",
            row.case
        );
        assert_eq!(
            summary_of(row.case, &summaries, &second),
            kept,
            "case {:?}: the source that was not deleted must be unchanged",
            row.case
        );

        // Repeating the delete does not report a second success.
        let (status, body) = delete_source(&router, &service_arn, &first).await;
        assert_eq!(
            status, 404,
            "case {:?}: expected HTTP 404 deleting an input source twice, got {status} {body:?}",
            row.case
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {:?}: expected ResourceNotFoundException, got {body:?}",
            row.case
        );

        // Clear the service for the next row.
        let (status, body) = delete_source(&router, &service_arn, &second).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 deleting the second source, got {status} {body:?}",
            row.case
        );
    }

    let (status, body) = list_sources(&router, &format!("serviceArn={service_arn}")).await;
    assert!(
        listed("after every delete", status, &body).is_empty(),
        "every source was deleted, so the listing is empty: {body:?}"
    );
}

/// `clientToken` is an `@idempotencyToken`: a repeat of the same create answers the source
/// the first call created rather than storing a second one.
#[tokio::test]
async fn a_replayed_client_token_answers_the_first_created_source() {
    let router = emulator();
    let service_arn = create_service(&router, "InputSourceIdempotency").await;

    // The rows span token shapes `ClientToken` admits (`len 1..64`, `^[A-Za-z0-9_.-]{1,64}$`)
    // and the configuration members a token can be used with.
    let tokens = ["t", "retry_token.1-2", &"z".repeat(64)];
    let rows = configuration_rows();
    for (token, row) in tokens.iter().zip(rows.iter()) {
        let mut body = row.body(&service_arn);
        body["clientToken"] = json!(token);
        let (status, first) = create_source(&router, body.clone()).await;
        let first_id = minted_id(row.case, &service_arn, status, &first);

        for attempt in 1..=2 {
            let (status, replay) = create_source(&router, body.clone()).await;
            assert_eq!(
                status, 200,
                "case {:?}: replay {attempt} of token {token:?} must succeed, got {status} \
                 {replay:?}",
                row.case
            );
            assert_eq!(
                replay, first,
                "case {:?}: replay {attempt} must answer the stored source, got {replay:?}",
                row.case
            );
        }

        let (status, body) = list_sources(&router, &format!("serviceArn={service_arn}")).await;
        let summaries = listed(row.case, status, &body);
        assert_eq!(
            summaries
                .iter()
                .filter(|summary| summary["inputSourceId"] == json!(first_id))
                .count(),
            1,
            "case {:?}: a replay must not store a second source, got {summaries:?}",
            row.case
        );
    }

    let (status, body) = list_sources(&router, &format!("serviceArn={service_arn}")).await;
    assert_eq!(
        listed("one source per token", status, &body).len(),
        tokens.len(),
        "three tokens, each replayed twice, created three sources: {body:?}"
    );
}

/// `type` narrows a listing to the sources whose configuration names that type, and a
/// request without it asks for all of them.
#[tokio::test]
async fn the_type_filter_narrows_the_listing() {
    let router = emulator();
    let service_arn = create_service(&router, "InputSourceTypeFilter").await;

    let rows = configuration_rows();
    for row in &rows {
        let (status, body) = create_source(&router, row.body(&service_arn)).await;
        minted_id(row.case, &service_arn, status, &body);
    }

    for wanted in INPUT_SOURCE_TYPE_VALUES {
        let expected = rows
            .iter()
            .filter(|row| row.expected_type == wanted)
            .count();
        let (status, body) =
            list_sources(&router, &format!("serviceArn={service_arn}&type={wanted}")).await;
        let summaries = listed(wanted, status, &body);
        assert_eq!(
            summaries.len(),
            expected,
            "case {wanted:?}: expected {expected} source(s) of that type, got {summaries:?}"
        );
        for summary in &summaries {
            assert_eq!(
                summary["type"],
                json!(wanted),
                "case {wanted:?}: a filtered listing reports only that type, got {summary:?}"
            );
        }
    }

    // Unfiltered, every stored source is enumerated.
    let (status, body) = list_sources(&router, &format!("serviceArn={service_arn}")).await;
    assert_eq!(
        listed("unfiltered", status, &body).len(),
        rows.len(),
        "a listing without a type filter enumerates every source: {body:?}"
    );
}

/// Paging enumerates every source exactly once, in creation order, and a page size or a
/// token outside what the model admits is refused.
#[tokio::test]
async fn paging_enumerates_every_source_once() {
    let router = emulator();
    let service_arn = create_service(&router, "InputSourcePaging").await;

    let rows = configuration_rows();
    let mut created: Vec<String> = Vec::new();
    for row in &rows {
        let (status, body) = create_source(&router, row.body(&service_arn)).await;
        created.push(minted_id(row.case, &service_arn, status, &body));
    }

    // The rows are page sizes: the smallest the model admits, a size that divides the set,
    // one that does not, the largest, and the default a request that omits it is given.
    for (case, max_results) in [
        ("the minimum page size", Some(MAX_RESULTS_MIN)),
        ("a size that divides the set", Some(2)),
        ("a size that does not divide the set", Some(3)),
        ("the maximum page size", Some(MAX_RESULTS_MAX)),
        ("the default page size", None),
    ] {
        let mut seen: Vec<String> = Vec::new();
        let mut next: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut query = format!("serviceArn={service_arn}");
            if let Some(size) = max_results {
                query.push_str(&format!("&maxResults={size}"));
            }
            if let Some(token) = next.as_deref() {
                query.push_str(&format!("&nextToken={token}"));
            }
            let (status, body) = list_sources(&router, &query).await;
            let page = listed(case, status, &body);
            if let Some(size) = max_results {
                assert!(
                    page.len() <= size as usize,
                    "case {case:?}: a page must not exceed maxResults={size}, got {page:?}"
                );
            }
            seen.extend(
                page.iter()
                    .map(|summary| summary["inputSourceId"].as_str().unwrap_or_default().to_string()),
            );
            pages += 1;
            assert!(
                pages <= created.len() + 1,
                "case {case:?}: paging must terminate, got {pages} pages for {} sources",
                created.len()
            );
            match body["nextToken"].as_str() {
                Some(token) => next = Some(token.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, created,
            "case {case:?}: paging must enumerate every source exactly once in creation order"
        );
    }

    // A `nextToken` this service did not issue is a bad request, not an empty page: one
    // value is not this service's encoding at all, the other is well-formed but names a
    // position never handed out ("999999" hex-encoded).
    for (case, token) in [
        ("not this service's encoding", "not-a-token"),
        ("a position never handed out", "393939393939"),
    ] {
        let (status, body) =
            list_sources(&router, &format!("serviceArn={service_arn}&nextToken={token}")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a forged nextToken, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body:?}"
        );
    }

    // And a `maxResults` outside the modelled range is refused rather than served at some
    // other size — one past each published bound.
    for (case, size) in [
        ("one below the minimum", MAX_RESULTS_MIN - 1),
        ("one above the maximum", MAX_RESULTS_MAX + 1),
    ] {
        let (status, body) =
            list_sources(&router, &format!("serviceArn={service_arn}&maxResults={size}")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for maxResults={size}, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body:?}"
        );
    }
}

/// A `serviceArn` or an `inputSourceId` naming nothing stored is refused, and an input
/// source belongs to exactly one service.
#[tokio::test]
async fn unknown_identifiers_are_refused_and_sources_are_per_service() {
    let router = emulator();
    let service_arn = create_service(&router, "InputSourceScope").await;
    let elsewhere = create_service(&router, "InputSourceBystander").await;
    let row = configuration_rows()
        .into_iter()
        .next()
        .expect("the configuration table has rows");
    let (status, body) = create_source(&router, row.body(&service_arn)).await;
    let id = minted_id(row.case, &service_arn, status, &body);

    // The rows span a syntactically valid ARN of this service's own form that names no
    // stored service, and one of another service's resource type.
    for (case, unknown) in [
        (
            "this service's ARN form, never created",
            "arn:aws:resiliencehub:us-east-1:123456789012:service/00000000-0000-8000-8000-000000000000",
        ),
        (
            "an ARN of another resource type",
            "arn:aws:resiliencehub:us-east-1:123456789012:policy/00000000-0000-8000-8000-000000000000",
        ),
    ] {
        let (status, body) = create_source(&router, row.body(unknown)).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 from CreateInputSource, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );

        let (status, body) = list_sources(&router, &format!("serviceArn={unknown}")).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 from ListInputSources, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );

        let (status, body) = delete_source(&router, unknown, &id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 from DeleteInputSource, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );
    }

    // An id that names no source in this service is a 404, and so is one that names a
    // source of ANOTHER service: a source belongs to exactly one service.
    for (case, target, input_source_id) in [
        ("an id never minted", service_arn.as_str(), "no-such-input-source"),
        ("another service's source", elsewhere.as_str(), id.as_str()),
    ] {
        let (status, body) = delete_source(&router, target, input_source_id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 from DeleteInputSource, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );
    }

    // The bystander service holds no source of its own, and the first service kept its one.
    let (status, body) = list_sources(&router, &format!("serviceArn={elsewhere}")).await;
    assert!(
        listed("the bystander service", status, &body).is_empty(),
        "another service's listing must not reach this service's sources: {body:?}"
    );
    let (status, body) = list_sources(&router, &format!("serviceArn={service_arn}")).await;
    assert_eq!(
        listed("the owning service", status, &body).len(),
        1,
        "the refused deletes must leave the stored source in place: {body:?}"
    );

    // Deleting the service does NOT take its input sources: they outlive it and stay
    // addressable through the very same ARN (see `input_sources_outlive_their_service`).
    let (status, body) = call(
        &router,
        "POST",
        DELETE_SERVICE,
        Some(json!({"serviceArn": service_arn})),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from DeleteService, got {status} {body:?}"
    );
    let (status, body) = list_sources(&router, &format!("serviceArn={service_arn}")).await;
    assert_eq!(
        listed("the deleted service's surviving sources", status, &body).len(),
        1,
        "the source created before the delete must still be listed, got {body:?}"
    );

    // A service recreated under the deleted one's name starts with no input sources, so
    // nothing leaked out of the removed record.
    let reborn = create_service(&router, "InputSourceScope").await;
    let (status, body) = list_sources(&router, &format!("serviceArn={reborn}")).await;
    assert!(
        listed("a recreated service", status, &body).is_empty(),
        "a freshly created service holds no input sources, got {body:?}"
    );
}

/// The id segment of an ARN: everything after the fifth `:`, which is what a
/// `ResourceNotFoundException` naming that resource reports and words its message with.
fn arn_resource_id(arn: &str) -> &str {
    arn.splitn(6, ':').nth(5).unwrap_or(arn)
}

/// Assert `uri` is refused with the `ResourceNotFoundException` a `serviceArn` naming no
/// stored service answers with, naming `service_id` the way that refusal words it.
async fn assert_service_gone(
    router: &MultiServiceRouter,
    case: &str,
    reader: &str,
    uri: &str,
    service_id: &str,
) {
    let (status, body) = call(router, "GET", uri, None).await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected HTTP 404 from {reader} on the deleted service, got {status} \
         {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException from {reader}, got {body:?}"
    );
    assert_eq!(
        body["message"],
        json!(format!("Service not found: {service_id}")),
        "case {case:?}: {reader} must name the service it could not find, got {body:?}"
    );
}

/// `DeleteService` takes the children the service record owns, but NOT its input sources:
/// an input source outlives the service it was created for and stays listable and
/// deletable through that same `serviceArn`.
///
/// The rows differ in how many input sources the service holds when it is deleted, which
/// `ResourceConfiguration` members those sources carry, and which of the children that DO
/// go with the service (a service function, an assertion) it also holds.
#[tokio::test]
async fn input_sources_outlive_their_service() {
    let rows = configuration_rows();
    for (case, count, with_function, with_assertion) in [
        ("one source, beside a function and an assertion", 1usize, true, true),
        ("two sources of different configuration members", 2, true, false),
        ("every configuration member the table spans", rows.len(), false, true),
    ] {
        let router = emulator();
        let service_arn = create_service(&router, "InputSourceSurvival").await;
        let service_id = arn_resource_id(&service_arn).to_string();

        if with_function {
            let (status, body) = call(
                &router,
                "POST",
                CREATE_SERVICE_FUNCTION,
                Some(json!({
                    "serviceArn": service_arn,
                    "name": "checkout flow",
                    "criticality": "PRIMARY",
                })),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from CreateServiceFunction, got {status} \
                 {body:?}"
            );
        }
        if with_assertion {
            let (status, body) = call(
                &router,
                "POST",
                CREATE_ASSERTION,
                Some(json!({
                    "serviceArn": service_arn,
                    "text": "Service must survive an AZ outage.",
                })),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from CreateAssertion, got {status} {body:?}"
            );
        }

        let created: Vec<(Row, String)> = {
            let mut created = Vec::new();
            for row in rows.iter().take(count) {
                let (status, body) = create_source(&router, row.body(&service_arn)).await;
                let id = minted_id(row.case, &service_arn, status, &body);
                created.push((row.clone(), id));
            }
            created
        };

        let (status, body) = list_sources(&router, &format!("serviceArn={service_arn}")).await;
        assert_eq!(
            listed(case, status, &body).len(),
            count,
            "case {case:?}: the live service must list every source created for it, got {body:?}"
        );

        let (status, body) = call(
            &router,
            "POST",
            DELETE_SERVICE,
            Some(json!({"serviceArn": service_arn})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteService, got {status} {body:?}"
        );

        // The service and the children its record owns are gone with it, each refused by
        // the one wording a `serviceArn` naming no stored service answers with.
        for (reader, uri) in [
            ("GetService", format!("{GET_SERVICE}?serviceArn={service_arn}")),
            (
                "ListServiceFunctions",
                format!("{LIST_SERVICE_FUNCTIONS}?serviceArn={service_arn}"),
            ),
            (
                "ListAssertions",
                format!("{LIST_ASSERTIONS}?serviceArn={service_arn}"),
            ),
        ] {
            assert_service_gone(&router, case, reader, &uri, &service_id).await;
        }

        // The input sources did not go with it: every one is still listed through the dead
        // ARN, still projected from what the create stored.
        let (status, body) = list_sources(&router, &format!("serviceArn={service_arn}")).await;
        let summaries = listed(case, status, &body);
        assert_eq!(
            summaries.len(),
            count,
            "case {case:?}: a deleted service's input sources must still be listed, got {body:?}"
        );
        for (row, id) in &created {
            row.assert_summary(&summary_of(case, &summaries, id));
        }

        // The listing's `type` filter reads the same surviving sources.
        for (row, id) in &created {
            let (status, body) = list_sources(
                &router,
                &format!("serviceArn={service_arn}&type={}", row.expected_type),
            )
            .await;
            let filtered = listed(case, status, &body);
            assert!(
                filtered
                    .iter()
                    .any(|summary| summary["inputSourceId"] == json!(id)),
                "case {case:?}: the {} filter must still reach the surviving source {id}, got \
                 {body:?}",
                row.expected_type
            );
        }

        // No source can be CREATED for a service that is gone: a create needs a live parent.
        let (status, body) = create_source(&router, rows[0].body(&service_arn)).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 from CreateInputSource on the deleted service, \
             got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException from CreateInputSource, got \
             {body:?}"
        );

        // And each surviving source is still DELETABLE through the dead ARN: the delete
        // answers 200 and the listing it leaves no longer carries that id.
        let mut remaining = count;
        for (row, id) in &created {
            let (status, body) = delete_source(&router, &service_arn, id).await;
            assert_eq!(
                status, 200,
                "case {case:?}/{:?}: expected HTTP 200 deleting a surviving source, got \
                 {status} {body:?}",
                row.case
            );
            assert_eq!(
                body["inputSourceId"],
                json!(id),
                "case {case:?}/{:?}: the delete must answer the source it removed, got {body:?}",
                row.case
            );
            remaining -= 1;
            let (status, body) =
                list_sources(&router, &format!("serviceArn={service_arn}")).await;
            let left = listed(case, status, &body);
            assert_eq!(
                left.len(),
                remaining,
                "case {case:?}/{:?}: the listing must lose exactly the deleted source, got \
                 {body:?}",
                row.case
            );
            assert!(
                !left
                    .iter()
                    .any(|summary| summary["inputSourceId"] == json!(id)),
                "case {case:?}/{:?}: {id} was deleted, so it must not be listed, got {body:?}",
                row.case
            );

            // Repeating the delete does not report a second success.
            let (status, body) = delete_source(&router, &service_arn, id).await;
            assert_eq!(
                status, 404,
                "case {case:?}/{:?}: expected HTTP 404 repeating the delete, got {status} \
                 {body:?}",
                row.case
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}/{:?}: expected ResourceNotFoundException, got {body:?}",
                row.case
            );
        }

        // With the last one deleted the ARN still answers the listing — with nothing in it.
        let (status, body) = list_sources(&router, &format!("serviceArn={service_arn}")).await;
        assert!(
            listed(case, status, &body).is_empty(),
            "case {case:?}: every surviving source was deleted, so the listing is empty, got \
             {body:?}"
        );
    }
}

/// A `serviceArn` that never had an input source keeps the refusal: nothing survives a
/// service that held none, so the listing and the delete are both 404 once it is gone.
#[tokio::test]
async fn an_arn_that_never_had_input_sources_is_still_refused() {
    let router = emulator();
    for (case, name) in [
        ("a service with a function but no input source", "NoSourcesButAFunction"),
        ("a service with no children at all", "NoChildrenAtAll"),
    ] {
        let service_arn = create_service(&router, name).await;
        if case.contains("function") {
            let (status, body) = call(
                &router,
                "POST",
                CREATE_SERVICE_FUNCTION,
                Some(json!({
                    "serviceArn": service_arn,
                    "name": "checkout flow",
                    "criticality": "PRIMARY",
                })),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from CreateServiceFunction, got {status} \
                 {body:?}"
            );
        }

        // While the service lives, the listing is an empty page rather than a refusal.
        let (status, body) = list_sources(&router, &format!("serviceArn={service_arn}")).await;
        assert!(
            listed(case, status, &body).is_empty(),
            "case {case:?}: a live service with no input sources lists none, got {body:?}"
        );

        let (status, body) = call(
            &router,
            "POST",
            DELETE_SERVICE,
            Some(json!({"serviceArn": service_arn})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteService, got {status} {body:?}"
        );

        let (status, body) = list_sources(&router, &format!("serviceArn={service_arn}")).await;
        assert_eq!(
            status, 404,
            "case {case:?}: nothing survives a service that had no input sources, so the \
             listing is refused, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );
        assert_eq!(
            body["message"],
            json!(format!("Service not found: {}", arn_resource_id(&service_arn))),
            "case {case:?}: the refusal must name the service it could not find, got {body:?}"
        );

        let (status, body) = delete_source(&router, &service_arn, "no-such-input-source").await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 from DeleteInputSource, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );
    }
}
