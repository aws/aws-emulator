// SPDX-License-Identifier: CC0-1.0

//! The AWS Marketplace Catalog assessment reports, and the not-found class of every
//! operation that reads a record, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path
//! a client takes.
//!
//! The rules under test:
//!
//! * `DescribeAssessment` classifies its `AssessmentIdentifier` BEFORE it reads the
//!   store: an identifier of neither modelled form names no assessment, so it is the
//!   modelled `ValidationException`, while one that is well formed but names nothing
//!   stored is the modelled `ResourceNotFoundException`.
//! * `ListAssessments` reports the store as a list — present and empty when nothing
//!   matches, never an absent member — selecting with every filter the model declares
//!   and reporting newest first, and `DescribeAssessment` reports the record the store
//!   holds through either form of identifier.
//! * An id that names no record is a CLIENT refusal naming the resource asked for,
//!   and it leaves the store exactly as it was.

use std::sync::Arc;
use std::time::UNIX_EPOCH;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::ops::types::{
    AMISecurityFilters, AssessmentTargetFilter, ContainerSecurityFilters, ControlAssessment,
    FrameworkFilters, FrameworkSummary,
};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::state::{
    assessment_arn, assessment_id_of, service_state, AssessmentRecord, ASSESSMENT_FRAMEWORKS,
    DESCRIBE_ASSESSMENT_PAGE_LIMIT, LIST_ASSESSMENTS_PAGE_LIMIT,
};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The catalog every operation's `Catalog` member documents as its fixed value.
const CATALOG: &str = "AWSMarketplace";

/// A SigV4 credential scope naming this service's signing name (`aws-marketplace`),
/// which is what routes a request here.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        let raw = self.body["__type"].as_str().unwrap_or_default();
        raw.rsplit('#').next().unwrap_or(raw)
    }

    /// The string at `pointer`, or the empty string.
    fn text(&self, pointer: &str) -> String {
        self.body
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "catalog.marketplace.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_EU_WEST_1);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

async fn describe_assessment(router: &MultiServiceRouter, identifier: &str) -> Answer {
    call(
        router,
        "POST",
        "/DescribeAssessment",
        Some(json!({"Catalog": CATALOG, "AssessmentIdentifier": identifier})),
    )
    .await
}

async fn list_assessments(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/ListAssessments", Some(request)).await
}

/// An assessment ARN of this catalog, in the form
/// `DescribeAssessmentRequest$AssessmentIdentifier` documents
/// (`arn:aws:aws-marketplace:<region>:<account>:AWSMarketplace/Assessment/assessment-…`).
/// Spelled out here rather than taken from the crate's own builder so a wrong
/// implementation cannot make this test pass.
fn spelled_out_assessment_arn(region: &str, account: &str, id: &str) -> String {
    format!("arn:aws:aws-marketplace:{region}:{account}:{CATALOG}/Assessment/{id}")
}

/// `DescribeAssessment` tells a malformed identifier from an unknown one, and answers
/// the modelled error for each — never a 501, and never a success for a record that
/// does not exist.
///
/// The rows differ along what the rule generalises over: which of the two modelled
/// forms the identifier takes, how long its id is, and — for the ARN form — which
/// partition, region, account, service namespace, catalog and resource type it names.
#[tokio::test]
async fn an_assessment_identifier_is_classified_before_the_store_is_read() {
    let router = emulator();

    // (case, identifier, expected status, expected error code)
    //
    // 404: a well-formed identifier of either form, naming an assessment this
    // catalog does not hold. 422: an identifier that is not one of the two forms the
    // service publishes — whether or not it satisfies the model's own, narrower
    // `AssessmentIdentifier` pattern, since the handler is what judges the member
    // (`skip_validation` on `DescribeAssessmentInput`) and it answers the modelled
    // `ValidationException` and its 422 for every such value.
    let rows: [(&str, String, u16, &str); 14] = [
        (
            "the shortest id",
            "assessment-1".to_string(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an alphanumeric id",
            "assessment-sim2realconv0000".to_string(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a long id",
            format!("assessment-{}", "a".repeat(200)),
            404,
            "ResourceNotFoundException",
        ),
        (
            "the arn form",
            spelled_out_assessment_arn("eu-west-1", "000000000000", "assessment-1"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "the arn form with no account",
            spelled_out_assessment_arn("us-east-1", "", "assessment-abc123"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "the arn form with no region",
            spelled_out_assessment_arn("", "000000000000", "assessment-abc123"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "the arn form in another partition",
            "arn:aws-cn:aws-marketplace:cn-north-1::AWSMarketplace/Assessment/assessment-9"
                .to_string(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an arn naming another service's namespace",
            "arn:aws:ec2:eu-west-1::AWSMarketplace/Assessment/assessment-1".to_string(),
            422,
            "ValidationException",
        ),
        (
            "an arn naming another catalog",
            "arn:aws:aws-marketplace:eu-west-1::OtherCatalog/Assessment/assessment-1".to_string(),
            422,
            "ValidationException",
        ),
        (
            "an arn naming another resource type",
            "arn:aws:aws-marketplace:eu-west-1::AWSMarketplace/ChangeSet/assessment-1".to_string(),
            422,
            "ValidationException",
        ),
        (
            "an arn naming a change set, not an assessment",
            "arn:aws:aws-marketplace:eu-west-1::AWSMarketplace/Assessment/cs-000000000001"
                .to_string(),
            422,
            "ValidationException",
        ),
        (
            "an identifier of neither form",
            "not-a-valid-identifier".to_string(),
            422,
            "ValidationException",
        ),
        (
            "an id with no suffix at all",
            "assessment-".to_string(),
            422,
            "ValidationException",
        ),
        (
            "an id in the wrong case",
            "ASSESSMENT-1".to_string(),
            422,
            "ValidationException",
        ),
    ];

    for (case, identifier, expected_status, expected_code) in rows {
        let answer = describe_assessment(&router, &identifier).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (expected_status, expected_code),
            "case {case:?}: expected HTTP {expected_status} {expected_code}, got {} {:?} \
             with {:?}",
            answer.status,
            answer.error_code(),
            answer.body
        );
        assert_ne!(
            answer.status, 501,
            "case {case:?}: a bad or unknown identifier is not an unimplemented operation, \
             got {:?}",
            answer.body
        );
        // Each refusal is built from the request: the 404 names the assessment asked
        // for, the 422 the identifier it rejected.
        let message = answer.text("/Message");
        let named = if expected_status == 404 {
            "assessment-"
        } else {
            identifier.as_str()
        };
        assert!(
            message.contains(named),
            "case {case:?}: the refusal must name {named:?}, got {message:?}"
        );
    }
}

/// `DescribeAssessment` refuses a catalog other than the fixed value its model
/// documents, and a page request outside what the model allows — before and
/// independently of the identifier.
#[tokio::test]
async fn describe_assessment_refuses_a_request_outside_its_contract() {
    let router = emulator();

    // `Catalog` is modelled `^[a-zA-Z]+$`, so each of these reaches the handler.
    for catalog in ["Marketplace", "awsmarketplace", "AWSMarketplaceX"] {
        let answer = call(
            &router,
            "POST",
            "/DescribeAssessment",
            Some(json!({"Catalog": catalog, "AssessmentIdentifier": "assessment-1"})),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (422, "ValidationException"),
            "case {catalog:?}: a catalog other than the fixed value must answer 422 \
             ValidationException, got {} {:?} with {:?}",
            answer.status,
            answer.error_code(),
            answer.body
        );
    }

    // A `NextToken` this service never issued names no page of control results.
    let answer = call(
        &router,
        "POST",
        "/DescribeAssessment",
        Some(json!({
            "Catalog": CATALOG,
            "AssessmentIdentifier": "assessment-1",
            "NextToken": "not-a-token",
        })),
    )
    .await;
    assert_eq!(
        (answer.status, answer.error_code()),
        (422, "ValidationException"),
        "a token this service never issued must answer 422 ValidationException, got {} {:?} \
         with {:?}",
        answer.status,
        answer.error_code(),
        answer.body
    );

    // `DescribeAssessmentMaxResultInteger` is `@range(min: 1, max: 100)`: each bound is
    // accepted and one past it is refused at the boundary as a client error.
    let page_sizes: [(i32, bool); 4] = [
        (1, true),
        (DESCRIBE_ASSESSMENT_PAGE_LIMIT, true),
        (0, false),
        (DESCRIBE_ASSESSMENT_PAGE_LIMIT + 1, false),
    ];
    for (max_results, inside) in page_sizes {
        let answer = call(
            &router,
            "POST",
            "/DescribeAssessment",
            Some(json!({
                "Catalog": CATALOG,
                "AssessmentIdentifier": "assessment-1",
                "MaxResults": max_results,
            })),
        )
        .await;
        if inside {
            // Inside the range the request is served: the assessment is simply not
            // there.
            assert_eq!(
                (answer.status, answer.error_code()),
                (404, "ResourceNotFoundException"),
                "case MaxResults={max_results}: a page size the model allows must reach the \
                 store, got {} {:?} with {:?}",
                answer.status,
                answer.error_code(),
                answer.body
            );
        } else {
            assert!(
                (400..500).contains(&answer.status) && answer.status != 404,
                "case MaxResults={max_results}: a page size outside the modelled range must \
                 be refused as a client error, got {} with {:?}",
                answer.status,
                answer.body
            );
        }
    }
}

/// `ListAssessments` answers 200 with the list member PRESENT — empty when this
/// catalog holds no assessment the filters select — and no `NextToken` when there are
/// no more results, through every filter the model declares.
///
/// The rows differ along what the rule generalises over: which filters are set, which
/// framework and target they name, and how large a page is asked for.
#[tokio::test]
async fn list_assessments_reports_the_store_through_every_modelled_filter() {
    let router = emulator();

    // The frameworks come from the model's own `FrameworkSummary` / `FrameworkFilters`
    // union members, so a framework added to the model is covered without editing this
    // row list.
    let mut rows: Vec<(String, Value)> = vec![
        ("no filter at all".to_string(), json!({"Catalog": CATALOG})),
        (
            "an entity target".to_string(),
            json!({"Catalog": CATALOG, "AssessmentTargetFilter": {"EntityId": "ent-000000000001"}}),
        ),
        (
            "a change-set target".to_string(),
            json!({
                "Catalog": CATALOG,
                "AssessmentTargetFilter": {"ChangeSetId": "cs-000000000001"},
            }),
        ),
        (
            "both targets".to_string(),
            json!({
                "Catalog": CATALOG,
                "AssessmentTargetFilter": {
                    "EntityId": "ent-000000000001",
                    "ChangeSetId": "cs-000000000001",
                },
            }),
        ),
        (
            // Both members are optional, so a filter that names neither constrains
            // nothing rather than being refused.
            "an empty target filter".to_string(),
            json!({"Catalog": CATALOG, "AssessmentTargetFilter": {}}),
        ),
        (
            "the smallest page".to_string(),
            json!({"Catalog": CATALOG, "MaxResults": 1}),
        ),
        (
            "the largest page the model allows".to_string(),
            json!({"Catalog": CATALOG, "MaxResults": LIST_ASSESSMENTS_PAGE_LIMIT}),
        ),
    ];
    for framework in ASSESSMENT_FRAMEWORKS {
        rows.push((
            format!("the {framework} framework, unversioned"),
            json!({"Catalog": CATALOG, "FrameworkId": framework}),
        ));
        rows.push((
            format!("the {framework} framework, versioned"),
            // `ListAssessmentsRequest$FrameworkId` is `^[\w\-@]+$` — the version
            // separator is in it, a dot is not.
            json!({"Catalog": CATALOG, "FrameworkId": format!("{framework}@1")}),
        ));
        rows.push((
            format!("the {framework} framework's own filters"),
            json!({
                "Catalog": CATALOG,
                "FrameworkFilters": {format!("{framework}Filters"): {}},
            }),
        ));
        rows.push((
            format!("a {framework} delivery option"),
            json!({
                "Catalog": CATALOG,
                "FrameworkFilters": {
                    format!("{framework}Filters"): {"DeliveryOptionId": "do-000000000001"},
                },
            }),
        ));
    }

    for (case, request) in rows {
        let answer = list_assessments(&router, request.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200 for {request}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body.pointer("/AssessmentSummaryList"),
            Some(&json!([])),
            "case {case:?}: AssessmentSummaryList must be present and empty, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.body.pointer("/NextToken"),
            None,
            "case {case:?}: a last page carries no NextToken, got {:?}",
            answer.body
        );
    }
}

/// `ListAssessments` refuses what it cannot answer: a catalog other than the fixed
/// value, a target filter that names neither target, a token it never issued, and a
/// page size outside the modelled range.
#[tokio::test]
async fn list_assessments_refuses_a_request_outside_its_contract() {
    let router = emulator();

    let refusals: [(&str, Value); 2] = [
        (
            "a catalog other than the fixed value",
            json!({"Catalog": "Marketplace"}),
        ),
        (
            "a token this service never issued",
            json!({"Catalog": CATALOG, "NextToken": "not-a-token"}),
        ),
    ];
    for (case, request) in refusals {
        let answer = list_assessments(&router, request).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (422, "ValidationException"),
            "case {case:?}: expected 422 ValidationException, got {} {:?} with {:?}",
            answer.status,
            answer.error_code(),
            answer.body
        );
    }

    // `ListAssessmentsMaxResultInteger` is `@range(min: 1, max: 100)`: each bound is
    // accepted and one past it is refused at the boundary as a client error.
    let page_sizes: [(i32, bool); 4] = [
        (1, true),
        (LIST_ASSESSMENTS_PAGE_LIMIT, true),
        (0, false),
        (LIST_ASSESSMENTS_PAGE_LIMIT + 1, false),
    ];
    for (max_results, inside) in page_sizes {
        let answer = list_assessments(&router, json!({"Catalog": CATALOG, "MaxResults": max_results}))
            .await;
        if inside {
            assert_eq!(
                answer.status, 200,
                "case MaxResults={max_results}: a page size the model allows must be served, \
                 got {} with {:?}",
                answer.status, answer.body
            );
        } else {
            assert!(
                (400..500).contains(&answer.status),
                "case MaxResults={max_results}: a page size outside the modelled range must be \
                 refused as a client error, got {} with {:?}",
                answer.status,
                answer.body
            );
            assert_ne!(
                answer.status, 501,
                "case MaxResults={max_results}: a bad request is not an unimplemented \
                 operation, got {:?}",
                answer.body
            );
        }
    }
}

/// Every operation that reads a record by id refuses an id that names none as a CLIENT
/// fault naming the resource asked for — and changes nothing: the lists report exactly
/// what was there before, and what was stored before is still readable after.
#[tokio::test]
async fn an_unknown_id_is_a_client_refusal_with_no_side_effect() {
    let router = emulator();

    // One stored change set, so "the store is unchanged" is a claim about something.
    let started = call(
        &router,
        "POST",
        "/StartChangeSet",
        Some(json!({
            "Catalog": CATALOG,
            "ChangeSet": [{
                "ChangeType": "CreateProduct",
                "Entity": {"Type": "AmiProduct@1.0"},
                "Details": "{\"ProductTitle\":\"before\"}",
            }],
        })),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "the fixture change set must be accepted, got {} with {:?}",
        started.status, started.body
    );
    let stored = started.text("/ChangeSetId");

    // (case, method, uri, body, the id the refusal must name)
    #[allow(clippy::type_complexity)]
    let rows: [(&str, &str, &str, Option<Value>, &str); 4] = [
        (
            "CancelChangeSet",
            "PATCH",
            "/CancelChangeSet?catalog=AWSMarketplace&changeSetId=cs-does-not-exist",
            None,
            "cs-does-not-exist",
        ),
        (
            "DescribeChangeSet",
            "GET",
            "/DescribeChangeSet?catalog=AWSMarketplace&changeSetId=cs-does-not-exist",
            None,
            "cs-does-not-exist",
        ),
        (
            "DescribeAssessment",
            "POST",
            "/DescribeAssessment",
            Some(json!({"Catalog": CATALOG, "AssessmentIdentifier": "assessment-nope"})),
            "assessment-nope",
        ),
        (
            "StartChangeSet against an unknown entity",
            "POST",
            "/StartChangeSet",
            Some(json!({
                "Catalog": CATALOG,
                "ChangeSet": [{
                    "ChangeType": "UpdateInformation",
                    "Entity": {"Type": "AmiProduct@1.0", "Identifier": "ent-does-not-exist"},
                    "Details": "{}",
                }],
            })),
            "ent-does-not-exist",
        ),
    ];

    for (case, method, uri, body, named) in rows {
        let answer = call(&router, method, uri, body).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: an id that names no record must answer 404 \
             ResourceNotFoundException, got {} {:?} with {:?}",
            answer.status,
            answer.error_code(),
            answer.body
        );
        assert!(
            answer.text("/Message").contains(named),
            "case {case:?}: the refusal must name {named:?}, got {:?}",
            answer.body
        );
    }

    // Nothing above created, cancelled or minted anything: the one stored change set is
    // still the only one, still readable, and the catalog still holds no assessment —
    // the fixture change set submitted no delivery option for a framework to evaluate.
    let listed = call(
        &router,
        "POST",
        "/ListChangeSets",
        Some(json!({"Catalog": CATALOG})),
    )
    .await;
    let ids: Vec<String> = listed
        .body
        .pointer("/ChangeSetSummaryList")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| item["ChangeSetId"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        ids,
        vec![stored.clone()],
        "the refusals must have changed nothing: expected only {stored:?}, got {:?}",
        listed.body
    );

    let described = call(
        &router,
        "GET",
        &format!("/DescribeChangeSet?catalog={CATALOG}&changeSetId={stored}"),
        None,
    )
    .await;
    assert_eq!(
        described.status, 200,
        "the stored change set must still be readable, got {} with {:?}",
        described.status, described.body
    );

    let assessments = list_assessments(&router, json!({"Catalog": CATALOG})).await;
    assert_eq!(
        assessments.body.pointer("/AssessmentSummaryList"),
        Some(&json!([])),
        "no call above submits a delivery option to a framework, so the list stays \
         empty, got {:?}",
        assessments.body
    );
}

/// An emulator whose store already holds `records`.
///
/// An assessment is the RESULT of a framework evaluation, which a change set submits a
/// product's delivery options to (`assessment_production.rs` drives that path end to
/// end). The record-level rules below are about reporting a record whatever it holds —
/// a framework, a target or a delivery option this catalog's own evaluation would never
/// pair — so they seed the store directly. Everything asserted after that goes through
/// the router.
fn emulator_holding(records: Vec<AssessmentRecord>) -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let state = service_state(&*engine).expect("the marketplace catalog store");
        let mut store = state.lock();
        for held in records {
            store.assessments.insert(held.id.clone(), held);
        }
    }
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// One assessment record, for the record-level rules below. `region` is what its ARN
/// names; `framework` is which `FrameworkSummary` member the report must carry.
fn record(
    id: &str,
    region: &str,
    framework: &str,
    entity_id: Option<&str>,
    change_set_id: Option<&str>,
    delivery_option_id: Option<&str>,
) -> AssessmentRecord {
    AssessmentRecord {
        id: id.to_string(),
        arn: assessment_arn(region, id),
        framework_id: format!("{framework}@1"),
        framework: framework.to_string(),
        delivery_option_id: delivery_option_id.map(str::to_string),
        entity_id: entity_id.map(str::to_string),
        change_set_id: change_set_id.map(str::to_string),
        result: "PASS".to_string(),
        created_at: "2026-01-01T00:00:00Z".to_string(),
        expires_at: "2026-02-01T00:00:00Z".to_string(),
        controls: vec![ControlAssessment {
            control_id: Some("control-1".to_string()),
            control_assessment_result: Some("PASS".to_string()),
            verification_source: Some("AWS_VERIFIED".to_string()),
            errors: None,
            control_summary: None,
        }],
        // A finished assessment: nothing of it is still running.
        runs_until: None,
        tags: Vec::new(),
    }
}

/// The three assessments the seeded suites read: two against one framework (one of an
/// entity, one of a change set, each with its own delivery option) and one against the
/// other, with distinct creation instants so the documented order is observable.
fn seeded() -> Vec<AssessmentRecord> {
    let mut newest = record(
        "assessment-newest",
        "eu-west-1",
        "AMISecurity",
        Some("ent-1"),
        None,
        Some("do-1"),
    );
    newest.created_at = "2026-01-03T00:00:00Z".to_string();
    newest.controls.push(ControlAssessment {
        control_id: Some("control-2".to_string()),
        control_assessment_result: Some("FAIL".to_string()),
        verification_source: Some("AWS_VERIFIED".to_string()),
        errors: None,
        control_summary: None,
    });
    let mut middle = record(
        "assessment-middle",
        "eu-west-1",
        "AMISecurity",
        None,
        Some("cs-1"),
        Some("do-2"),
    );
    middle.created_at = "2026-01-02T00:00:00Z".to_string();
    let mut oldest = record(
        "assessment-oldest",
        "us-east-1",
        "ContainerSecurity",
        Some("ent-2"),
        None,
        Some("do-1"),
    );
    oldest.created_at = "2026-01-01T00:00:00Z".to_string();
    oldest.result = "FAIL".to_string();
    vec![newest, middle, oldest]
}

/// The `AssessmentId`s of a `ListAssessments` answer, in the order it reported them.
fn listed_ids(answer: &Answer) -> Vec<String> {
    answer
        .body
        .pointer("/AssessmentSummaryList")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| item["AssessmentId"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// `ListAssessments` selects the assessments the store holds with every filter the
/// model declares, and reports them newest first.
///
/// The rows differ along what the rule generalises over: which filter is set, the
/// framework (versioned and not), which target, and which delivery option.
#[tokio::test]
async fn list_assessments_selects_and_orders_the_assessments_the_store_holds() {
    let router = emulator_holding(seeded());

    // (case, request, the ids it must report, in order)
    let rows: [(&str, Value, &[&str]); 8] = [
        (
            "no filter: newest first",
            json!({"Catalog": CATALOG}),
            &[
                "assessment-newest",
                "assessment-middle",
                "assessment-oldest",
            ],
        ),
        (
            "one framework, unversioned",
            json!({"Catalog": CATALOG, "FrameworkId": "AMISecurity"}),
            &["assessment-newest", "assessment-middle"],
        ),
        (
            "one framework, versioned",
            json!({"Catalog": CATALOG, "FrameworkId": "AMISecurity@1"}),
            &["assessment-newest", "assessment-middle"],
        ),
        (
            "the other framework",
            json!({"Catalog": CATALOG, "FrameworkId": "ContainerSecurity"}),
            &["assessment-oldest"],
        ),
        (
            "an entity target",
            json!({"Catalog": CATALOG, "AssessmentTargetFilter": {"EntityId": "ent-1"}}),
            &["assessment-newest"],
        ),
        (
            "a change-set target",
            json!({"Catalog": CATALOG, "AssessmentTargetFilter": {"ChangeSetId": "cs-1"}}),
            &["assessment-middle"],
        ),
        (
            "a framework's own filters",
            json!({"Catalog": CATALOG, "FrameworkFilters": {"ContainerSecurityFilters": {}}}),
            &["assessment-oldest"],
        ),
        (
            "a framework and a delivery option",
            json!({
                "Catalog": CATALOG,
                "FrameworkFilters": {"AMISecurityFilters": {"DeliveryOptionId": "do-2"}},
            }),
            &["assessment-middle"],
        ),
    ];

    for (case, request, expected) in rows {
        let answer = list_assessments(&router, request.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200 for {request}, got {} with {:?}",
            answer.status, answer.body
        );
        let ids = listed_ids(&answer);
        assert_eq!(
            ids,
            expected.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
            "case {case:?}: expected {expected:?} in that order, got {ids:?} from {:?}",
            answer.body
        );
    }

    // And the pages are the model's: `MaxResults` bounds one page, the `NextToken`
    // resumes exactly where it ended, and the last page carries none.
    let first = list_assessments(&router, json!({"Catalog": CATALOG, "MaxResults": 2})).await;
    assert_eq!(
        listed_ids(&first),
        vec![
            "assessment-newest".to_string(),
            "assessment-middle".to_string()
        ],
        "the first page of two must be the two newest, got {:?}",
        first.body
    );
    let token = first.text("/NextToken");
    assert!(
        !token.is_empty(),
        "a page with results left over must carry a NextToken, got {:?}",
        first.body
    );
    let second = list_assessments(
        &router,
        json!({"Catalog": CATALOG, "MaxResults": 2, "NextToken": token}),
    )
    .await;
    assert_eq!(
        listed_ids(&second),
        vec!["assessment-oldest".to_string()],
        "the token must resume at the third assessment, got {:?}",
        second.body
    );
    assert_eq!(
        second.body.pointer("/NextToken"),
        None,
        "the last page carries no NextToken, got {:?}",
        second.body
    );
}

/// `DescribeAssessment` reports the record the store holds — through either form of
/// identifier — and pages its control results.
#[tokio::test]
async fn describe_assessment_reports_the_record_the_store_holds() {
    let held = seeded()
        .into_iter()
        .next()
        .expect("the seeded assessments are not empty");
    let router = emulator_holding(vec![held.clone()]);

    // Either form of `AssessmentIdentifier` names the same record.
    for identifier in [held.id.clone(), held.arn.clone()] {
        let answer = describe_assessment(&router, &identifier).await;
        assert_eq!(
            answer.status, 200,
            "case {identifier:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            (
                answer.text("/AssessmentId"),
                answer.text("/AssessmentArn"),
                answer.text("/FrameworkId"),
                answer.text("/AssessmentResult"),
                answer.text("/CreatedAt"),
                answer.text("/ExpiresAt"),
                answer.text("/AssessmentTargetSummary/EntityId"),
                answer.text("/FrameworkSummary/AMISecuritySummary/DeliveryOptionId"),
            ),
            (
                held.id.clone(),
                held.arn.clone(),
                held.framework_id.clone(),
                held.result.clone(),
                held.created_at.clone(),
                held.expires_at.clone(),
                held.entity_id.clone().unwrap_or_default(),
                held.delivery_option_id.clone().unwrap_or_default(),
            ),
            "case {identifier:?}: the report must be the record the store holds, got {:?}",
            answer.body
        );
        let controls = answer
            .body
            .pointer("/ControlAssessments")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            controls.len(),
            held.controls.len(),
            "case {identifier:?}: every control result must be reported, got {:?}",
            answer.body
        );
    }

    // `ControlAssessments` is `@paginated`: one page at a time, resumed by the token.
    let first = call(
        &router,
        "POST",
        "/DescribeAssessment",
        Some(json!({
            "Catalog": CATALOG,
            "AssessmentIdentifier": held.id,
            "MaxResults": 1,
        })),
    )
    .await;
    assert_eq!(
        (
            first.status,
            first.text("/ControlAssessments/0/ControlId").as_str()
        ),
        (200, "control-1"),
        "the first page of one must carry the first control, got {} with {:?}",
        first.status,
        first.body
    );
    let token = first.text("/NextToken");
    assert!(
        !token.is_empty(),
        "a page with controls left over must carry a NextToken, got {:?}",
        first.body
    );
    let second = call(
        &router,
        "POST",
        "/DescribeAssessment",
        Some(json!({
            "Catalog": CATALOG,
            "AssessmentIdentifier": held.arn,
            "MaxResults": 1,
            "NextToken": token,
        })),
    )
    .await;
    assert_eq!(
        (
            second.status,
            second.text("/ControlAssessments/0/ControlId").as_str(),
            second.text("/ControlAssessments/0/ControlAssessmentResult").as_str(),
        ),
        (200, "control-2", "FAIL"),
        "the token must resume at the second control, got {} with {:?}",
        second.status,
        second.body
    );
    assert_eq!(
        second.body.pointer("/NextToken"),
        None,
        "the last page carries no NextToken, got {:?}",
        second.body
    );
}

/// An assessment record projects what it holds, and the `FrameworkSummary` member it
/// reports is the one its `FrameworkId` names — one row per framework the model
/// declares, plus a framework the model has no member for, which reports none.
#[test]
fn an_assessment_record_projects_the_framework_its_id_names() {
    for framework in ASSESSMENT_FRAMEWORKS {
        let held = record(
            "assessment-1",
            "eu-west-1",
            framework,
            Some("ent-1"),
            None,
            Some("do-1"),
        );
        // The record owes no run, so the projection reports what it holds at any instant;
        // this one is a constant rather than the host's clock.
        let summary = held.to_summary(UNIX_EPOCH);
        assert_eq!(
            (
                summary.assessment_id.as_deref(),
                summary.assessment_arn.as_deref(),
                summary.framework_id.as_deref(),
                summary.assessment_result.as_deref(),
                summary.created_at.as_deref(),
                summary.expires_at.as_deref(),
            ),
            (
                Some(held.id.as_str()),
                Some(held.arn.as_str()),
                Some(held.framework_id.as_str()),
                Some(held.result.as_str()),
                Some(held.created_at.as_str()),
                Some(held.expires_at.as_str()),
            ),
            "case {framework:?}: the summary must carry what the record holds, got \
             {summary:?}"
        );
        assert_eq!(
            summary
                .assessment_target_summary
                .as_ref()
                .and_then(|target| target.entity_id.as_deref()),
            Some("ent-1"),
            "case {framework:?}: the summary must name the entity assessed, got {summary:?}"
        );
        let reported = match summary.framework_summary.as_ref() {
            Some(FrameworkSummary::AMISecuritySummary(summary)) => {
                ("AMISecurity", summary.delivery_option_id.as_deref())
            }
            Some(FrameworkSummary::ContainerSecuritySummary(summary)) => {
                ("ContainerSecurity", summary.delivery_option_id.as_deref())
            }
            // The two framework summaries of the LISTING frameworks carry no
            // delivery option: this catalog assesses neither, so no record
            // reports one of them.
            Some(FrameworkSummary::ListingQualitySummary(_)) => ("ListingQuality", None),
            Some(FrameworkSummary::ListingRecommendationsSummary(_)) => {
                ("ListingRecommendations", None)
            }
            None => ("none", None),
        };
        assert_eq!(
            reported,
            (*framework, Some("do-1")),
            "case {framework:?}: the summary must carry that framework's own member with \
             its delivery option, got {reported:?}"
        );
    }

    // A framework the model declares no `FrameworkSummary` member for reports none
    // rather than another framework's.
    let unknown = record("assessment-2", "us-east-1", "NoSuchSecurity", None, None, None);
    assert!(
        unknown.to_summary(UNIX_EPOCH).framework_summary.is_none(),
        "a framework with no modelled member must report none, got {:?}",
        unknown.to_summary(UNIX_EPOCH)
    );
}

/// The filters `ListAssessments` declares select a record by what it holds: a member a
/// filter leaves absent constrains nothing, and a framework filter selects the
/// framework its union member names.
#[test]
fn the_modelled_filters_select_a_record_by_what_it_holds() {
    let held = record(
        "assessment-1",
        "eu-west-1",
        "AMISecurity",
        Some("ent-1"),
        Some("cs-1"),
        Some("do-1"),
    );

    // (case, filter, expected)
    let targets: [(&str, AssessmentTargetFilter, bool); 5] = [
        ("nothing named", AssessmentTargetFilter::default(), true),
        (
            "its entity",
            AssessmentTargetFilter {
                entity_id: Some("ent-1".to_string()),
                change_set_id: None,
                entity_ids: None,
            },
            true,
        ),
        (
            "another entity",
            AssessmentTargetFilter {
                entity_id: Some("ent-2".to_string()),
                change_set_id: None,
                entity_ids: None,
            },
            false,
        ),
        (
            "its entity and its change set",
            AssessmentTargetFilter {
                entity_id: Some("ent-1".to_string()),
                change_set_id: Some("cs-1".to_string()),
                entity_ids: None,
            },
            true,
        ),
        (
            "its entity and another change set",
            AssessmentTargetFilter {
                entity_id: Some("ent-1".to_string()),
                change_set_id: Some("cs-2".to_string()),
                entity_ids: None,
            },
            false,
        ),
    ];
    for (case, filter, expected) in targets {
        let matched = held.matches_target(&filter);
        assert_eq!(
            matched, expected,
            "case {case:?}: expected matches_target == {expected}, got {matched} for \
             {filter:?}"
        );
    }

    // `FrameworkId` is the UNVERSIONED id, so it selects every version of a framework;
    // the versioned form selects that version alone.
    let framework_ids: [(&str, bool); 4] = [
        ("AMISecurity", true),
        ("AMISecurity@1", true),
        ("AMISecurity@2", false),
        ("ContainerSecurity", false),
    ];
    for (framework_id, expected) in framework_ids {
        let matched = held.matches_framework_id(framework_id);
        assert_eq!(
            matched, expected,
            "case {framework_id:?}: expected matches_framework_id == {expected}, got \
             {matched}"
        );
    }

    // The union member chosen selects the framework; the `DeliveryOptionId` it carries
    // narrows further.
    let framework_filters: [(&str, FrameworkFilters, bool); 4] = [
        (
            "its framework, no delivery option",
            FrameworkFilters::AMISecurityFilters(AMISecurityFilters::default()),
            true,
        ),
        (
            "its framework and its delivery option",
            FrameworkFilters::AMISecurityFilters(AMISecurityFilters {
                delivery_option_id: Some("do-1".to_string()),
            }),
            true,
        ),
        (
            "its framework and another delivery option",
            FrameworkFilters::AMISecurityFilters(AMISecurityFilters {
                delivery_option_id: Some("do-2".to_string()),
            }),
            false,
        ),
        (
            "another framework",
            FrameworkFilters::ContainerSecurityFilters(ContainerSecurityFilters::default()),
            false,
        ),
    ];
    for (case, filters, expected) in framework_filters {
        let matched = held.matches_framework_filters(&filters);
        assert_eq!(
            matched, expected,
            "case {case:?}: expected matches_framework_filters == {expected}, got {matched}"
        );
    }
}

/// The ARN this service mints for an assessment is one of the two forms
/// `AssessmentIdentifier` accepts back: it resolves to the same id, in every region, and
/// an ARN naming another service's namespace or another catalog resolves to none.
#[test]
fn an_assessment_arn_resolves_back_to_its_own_id() {
    for region in ["eu-west-1", "us-east-1", ""] {
        for id in ["assessment-1", "assessment-sim2realconv0000"] {
            let arn = assessment_arn(region, id);
            assert_eq!(
                assessment_id_of(&arn, CATALOG),
                Some(id),
                "case {arn:?}: the minted ARN must resolve to {id:?}"
            );
            assert_eq!(
                assessment_id_of(id, CATALOG),
                Some(id),
                "case {id:?}: the bare id must resolve to itself"
            );
        }
    }

    // (case, identifier) — each names no assessment resource of this catalog.
    let rejected: [(&str, &str); 4] = [
        (
            "another service's namespace",
            "arn:aws:ec2:eu-west-1::AWSMarketplace/Assessment/assessment-1",
        ),
        (
            "another catalog",
            "arn:aws:aws-marketplace:eu-west-1::OtherCatalog/Assessment/assessment-1",
        ),
        (
            "another resource type",
            "arn:aws:aws-marketplace:eu-west-1::AWSMarketplace/ChangeSet/assessment-1",
        ),
        ("neither form", "not-a-valid-identifier"),
    ];
    for (case, identifier) in rejected {
        assert_eq!(
            assessment_id_of(identifier, CATALOG),
            None,
            "case {case:?}: {identifier:?} must resolve to no assessment"
        );
    }
}
