// SPDX-License-Identifier: CC0-1.0

//! `ListAssessments` filtered by `AssessmentTargetFilter$EntityIds`, end to end over
//! HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization. Nothing seeds the
//! store: the assessments read back are the ones a change set produced and the ones
//! `StartAssessment` started over the same entities.
//!
//! The rule under test is the member's own contract: `EntityIds` is "a list of up to 50
//! entity IDs whose assessments you want to list. The response includes only the latest
//! published assessment of each entity. You must also specify `FrameworkId` in the
//! request, and you can't combine `EntityIds` with `EntityId` or `ChangeSetId`" — so a
//! request naming it beside those members, or without a framework, or carrying a value
//! that is not an entity id, is refused and selects nothing, while one inside the
//! contract answers at most one assessment per entity it named.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The catalog every operation's `Catalog` member documents as its fixed value.
const CATALOG: &str = "AWSMarketplace";

/// A SigV4 credential scope naming this service's signing name (`aws-marketplace`), which
/// is what routes a request here.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The framework an AMI product is assessed against.
const FRAMEWORK: &str = "AMISecurity";

/// An entity id of the published form that names no entity this catalog holds.
const ABSENT_ENTITY: &str = "ent-000000000000";

/// How long a poll waits for a condition the clock owes before giving up: far longer than
/// any span this service schedules, so a loaded host waits rather than failing.
const POLL_DEADLINE: Duration = Duration::from_secs(60);

/// How long a poll sleeps between looks.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

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

    /// The array at `pointer`, or an empty one when the member is absent.
    fn list(&self, pointer: &str) -> Vec<Value> {
        self.body
            .pointer(pointer)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
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

/// The details of a change that submits one AMI delivery option as a product version.
fn ami_version() -> Value {
    json!({
        "Version": {"VersionTitle": "1.0"},
        "DeliveryOptions": [{
            "Details": {"AmiDeliveryOptionDetails": {
                "AmiSource": {
                    "AmiId": "ami-0abcdef1234567890",
                    "AccessRoleArn": "arn:aws:iam::123456789012:role/MarketplaceScan",
                    "UserName": "ec2-user",
                },
                "SecurityGroups": [{"IpProtocol": "tcp", "IpRanges": ["10.0.0.0/16"]}],
                "UsageInstructions": "Connect over SSH.",
            }},
        }],
    })
}

/// Create `count` AMI products, each submitting one delivery option, through ONE change
/// set, and answer their entity ids once that change set has succeeded.
async fn created_products(router: &MultiServiceRouter, count: usize) -> Vec<String> {
    let changes: Vec<Value> = (0..count)
        .map(|_| {
            json!({
                "ChangeType": "AddDeliveryOptions",
                "Entity": {"Type": "AmiProduct@1.0"},
                "DetailsDocument": ami_version(),
            })
        })
        .collect();
    let started = call(
        router,
        "POST",
        "/StartChangeSet",
        Some(json!({"Catalog": CATALOG, "ChangeSet": changes})),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "the fixture's change set must be accepted, got {} with {:?}",
        started.status, started.body
    );
    let change_set_id = started.text("/ChangeSetId");

    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let listed = call(
            router,
            "POST",
            "/ListChangeSets",
            Some(json!({"Catalog": CATALOG, "MaxResults": 20})),
        )
        .await;
        let summary = listed
            .list("/ChangeSetSummaryList")
            .into_iter()
            .find(|item| item["ChangeSetId"].as_str() == Some(change_set_id.as_str()))
            .unwrap_or_else(|| {
                panic!(
                    "the fixture's change set {change_set_id} must be listed, got {:?}",
                    listed.body
                )
            });
        let status = summary["Status"].as_str().unwrap_or_default().to_string();
        if status == "SUCCEEDED" {
            let ids: Vec<String> = summary["EntityIdList"]
                .as_array()
                .map(|ids| {
                    ids.iter()
                        .map(|id| id.as_str().unwrap_or_default().to_string())
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                ids.len(),
                count,
                "the fixture's change set must report one entity per change, got {summary:?}"
            );
            return ids;
        }
        assert!(
            status != "FAILED" && Instant::now() < deadline,
            "the fixture's change set must succeed, got {status:?} with {summary:?}"
        );
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Start an assessment of `entity_id` and answer its id once its run has finished.
async fn finished_assessment_of(router: &MultiServiceRouter, entity_id: &str) -> String {
    let started = call(
        router,
        "POST",
        "/StartAssessment",
        Some(json!({
            "Catalog": CATALOG,
            "EntityId": entity_id,
            "FrameworkId": FRAMEWORK,
        })),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "starting an assessment of {entity_id} must be accepted, got {} with {:?}",
        started.status, started.body
    );
    let assessment_id = started.text("/AssessmentId");

    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let described = call(
            router,
            "POST",
            "/DescribeAssessment",
            Some(json!({"Catalog": CATALOG, "AssessmentIdentifier": assessment_id})),
        )
        .await;
        if !described.text("/AssessmentResult").is_empty() {
            return assessment_id;
        }
        assert!(
            Instant::now() < deadline,
            "the assessment {assessment_id} must finish and report a result, got {:?}",
            described.body
        );
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// `ListAssessments` with `request` beside the fixed catalog.
async fn list_assessments(router: &MultiServiceRouter, mut request: Value) -> Answer {
    if let Value::Object(members) = &mut request {
        members.insert("Catalog".to_string(), json!(CATALOG));
        members.insert("MaxResults".to_string(), json!(100));
    }
    call(router, "POST", "/ListAssessments", Some(request)).await
}

/// The `AssessmentId`s an answer reported, sorted, so the assertion is about the SET of
/// assessments selected.
fn selected_ids(answer: &Answer) -> Vec<String> {
    let mut ids: Vec<String> = answer
        .list("/AssessmentSummaryList")
        .iter()
        .map(|summary| {
            summary["AssessmentId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    ids.sort();
    ids
}

/// A request filtering by `EntityIds` answers the latest assessment of each entity it
/// names, and nothing for an entity with none — while the singular `EntityId` answers
/// every assessment of the one entity it names.
///
/// The rows differ along how many entities the list names, whether one of them has no
/// assessment at all, and whether the list names an entity twice.
#[tokio::test]
async fn entity_ids_answers_the_latest_assessment_of_each_entity_it_names() {
    let router = emulator();
    let entities = created_products(&router, 2).await;
    let (first, second) = (entities[0].clone(), entities[1].clone());

    // Each product already carries the assessment its change set produced; a start adds a
    // later one, which is then the latest of that entity.
    let latest_of_first = finished_assessment_of(&router, &first).await;
    let latest_of_second = finished_assessment_of(&router, &second).await;

    let produced = list_assessments(&router, json!({"AssessmentTargetFilter": {"EntityId": first}}))
        .await;
    let all_of_first = selected_ids(&produced);
    assert_eq!(
        all_of_first.len(),
        2,
        "the singular EntityId answers every assessment of that entity, got {:?}",
        produced.body
    );

    // (case, the entity ids asked for, the assessments the page must carry)
    let rows: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        (
            "one entity",
            vec![first.clone()],
            vec![latest_of_first.clone()],
        ),
        (
            "both entities",
            vec![first.clone(), second.clone()],
            vec![latest_of_first.clone(), latest_of_second.clone()],
        ),
        (
            "an entity named twice",
            vec![first.clone(), first.clone()],
            vec![latest_of_first.clone()],
        ),
        (
            "one entity beside one with no assessment",
            vec![second.clone(), ABSENT_ENTITY.to_string()],
            vec![latest_of_second.clone()],
        ),
        (
            "an entity with no assessment alone",
            vec![ABSENT_ENTITY.to_string()],
            Vec::new(),
        ),
    ];

    for (case, asked, expected) in rows {
        let answer = list_assessments(
            &router,
            json!({
                "FrameworkId": FRAMEWORK,
                "AssessmentTargetFilter": {"EntityIds": asked},
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a request inside the contract must be served, got {} with {:?}",
            answer.status, answer.body
        );
        let mut expected = expected;
        expected.sort();
        assert_eq!(
            selected_ids(&answer),
            expected,
            "case {case:?}: the page must carry the latest assessment of each entity named, \
             got {:?}",
            answer.body
        );
    }
}

/// A request naming `EntityIds` outside the member's contract is refused and selects
/// nothing.
#[tokio::test]
async fn entity_ids_outside_its_contract_is_refused() {
    let router = emulator();
    let entities = created_products(&router, 1).await;
    let entity_id = entities[0].clone();

    // (case, the request members beside the catalog and the page size)
    let rows: Vec<(&str, Value)> = vec![
        (
            "no framework beside it",
            json!({"AssessmentTargetFilter": {"EntityIds": [entity_id.clone()]}}),
        ),
        (
            "combined with the singular entity id",
            json!({
                "FrameworkId": FRAMEWORK,
                "AssessmentTargetFilter": {
                    "EntityIds": [entity_id.clone()],
                    "EntityId": entity_id.clone(),
                },
            }),
        ),
        (
            "combined with a change set id",
            json!({
                "FrameworkId": FRAMEWORK,
                "AssessmentTargetFilter": {
                    "EntityIds": [entity_id.clone()],
                    "ChangeSetId": "cs-000000000001",
                },
            }),
        ),
        (
            "a value that is not an entity id",
            json!({
                "FrameworkId": FRAMEWORK,
                // Inside the wider `ResourceId` the member targets, outside the form this
                // service states for an entity id.
                "AssessmentTargetFilter": {"EntityIds": [entity_id.clone(), "ent_1"]},
            }),
        ),
    ];

    for (case, request) in rows {
        let refused = list_assessments(&router, request.clone()).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (422, "ValidationException"),
            "case {case:?}: {request} must be refused 422 ValidationException, got {} with {:?}",
            refused.status,
            refused.body
        );
        assert!(
            refused.list("/AssessmentSummaryList").is_empty(),
            "case {case:?}: a refused request serves no page, got {:?}",
            refused.body
        );
    }

    // The refusals changed nothing: the entity's own assessment is still reported.
    let served = list_assessments(
        &router,
        json!({
            "FrameworkId": FRAMEWORK,
            "AssessmentTargetFilter": {"EntityIds": [entity_id.clone()]},
        }),
    )
    .await;
    assert_eq!(
        selected_ids(&served).len(),
        1,
        "the entity's latest assessment must still be served, got {:?}",
        served.body
    );
}
