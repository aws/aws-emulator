// SPDX-License-Identifier: CC0-1.0

//! `ListAssessments` judges a `FrameworkFilters` member against the framework the
//! request itself names, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path
//! a client takes.
//!
//! The rule under test: when a request carries BOTH a member naming the scope a page is
//! listed for and a union member whose own documentation ties it to exactly one such
//! scope, the member named must be the one belonging to that scope — a member of a
//! different scope asks for two scopes at once, which nothing listed can satisfy, so it
//! is refused with the modelled `ValidationException` (422) naming the member and the
//! scope the request asked for, rather than answered with the empty page the two
//! predicates select together.
//!
//! `ListAssessments`' `FrameworkId` and `FrameworkFilters` are such a pair
//! (`FrameworkFilters` is "framework-specific filters … Set exactly one member,
//! corresponding to the framework you want to filter by"), and the sibling pairs of the
//! same kind — `ListEntities`' `EntityType` with `EntityTypeFilters` and with
//! `EntityTypeSort` — are judged the same way in `entity_type_projections.rs`.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::ops::types::ControlAssessment;
use service_com_amazonaws_marketplacecatalog_awsmpseymour::state::{
    assessment_arn, service_state, AssessmentRecord,
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

/// The framework version the seeded records were evaluated at.
///
/// `ListAssessmentsRequest$FrameworkId`'s `@pattern` (`^[\w\-@]+$`) admits the version
/// separator but no dot, so a version a request can name through that member is a plain
/// run of alphanumerics.
const SEEDED_VERSION: &str = "1";

/// The frameworks this catalog assesses against, each paired with the `FrameworkFilters`
/// union member that filters by it and with a delivery option of the seeded record.
///
/// Written out here rather than imported from the crate, so a pairing the service
/// invents fails this test instead of following it. The pairs are the model's own
/// (`smithy_query(query="shape", shape="FrameworkFilters")`: `AMISecurityFilters` —
/// "filters that apply to assessments performed against the AMI Security framework" —
/// and `ContainerSecurityFilters`, whose frameworks `FrameworkSummary`'s members give
/// as `AMISecurity` and `ContainerSecurity`).
const FRAMEWORK_MEMBERS: &[(&str, &str)] = &[
    ("AMISecurity", "AMISecurityFilters"),
    ("ContainerSecurity", "ContainerSecurityFilters"),
];

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

    /// The fault this answer carries: `ValidationException$Message`.
    fn message(&self) -> String {
        self.body["Message"].as_str().unwrap_or_default().to_string()
    }

    /// The `AssessmentId`s this answer reported, in the order it reported them.
    fn listed_ids(&self) -> Vec<String> {
        self.body
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
}

async fn list_assessments(router: &MultiServiceRouter, request: Value) -> Answer {
    let http_request = ::http::Request::builder()
        .method("POST")
        .uri("/ListAssessments")
        .header("host", "catalog.marketplace.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_EU_WEST_1)
        .header("content-type", "application/json")
        .body(Bytes::from(request.to_string()))
        .expect("the test request is well formed");

    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "POST /ListAssessments answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// One assessment record of `framework`, created at `created_at` and carrying
/// `delivery_option_id`.
fn record(
    id: &str,
    framework: &str,
    delivery_option_id: &str,
    created_at: &str,
) -> AssessmentRecord {
    AssessmentRecord {
        id: id.to_string(),
        arn: assessment_arn("eu-west-1", id),
        framework_id: format!("{framework}@{SEEDED_VERSION}"),
        framework: framework.to_string(),
        delivery_option_id: Some(delivery_option_id.to_string()),
        entity_id: Some("ent-1".to_string()),
        change_set_id: None,
        result: "PASS".to_string(),
        created_at: created_at.to_string(),
        expires_at: "2026-04-01T00:00:00Z".to_string(),
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

/// The delivery option of the one seeded assessment of `framework`.
fn seeded_delivery_option(framework: &str) -> String {
    format!("do-{}", framework.to_lowercase())
}

/// The id of the one seeded assessment of `framework`.
fn seeded_id(framework: &str) -> String {
    format!("assessment-{}", framework.to_lowercase())
}

/// An emulator holding exactly one assessment of each published framework.
///
/// An assessment is the RESULT of a framework evaluation, which a change set submits a
/// product's delivery options to (`assessment_production.rs` drives that path end to
/// end). The rule under test is about which REQUESTS are answered at all, so the store
/// is seeded directly and everything asserted after that goes through the router.
fn emulator_holding_one_assessment_per_framework() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let state = service_state(&*engine).expect("the marketplace catalog store");
        let mut store = state.lock();
        for (index, (framework, _)) in FRAMEWORK_MEMBERS.iter().enumerate() {
            let held = record(
                &seeded_id(framework),
                framework,
                &seeded_delivery_option(framework),
                &format!("2026-01-0{}T00:00:00Z", index + 1),
            );
            store.assessments.insert(held.id.clone(), held);
        }
    }
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// The two forms a request can name a framework in through `FrameworkId`: the
/// unversioned id the member documents, and that framework at a version.
fn framework_id_forms(framework: &str) -> [String; 2] {
    [
        framework.to_string(),
        format!("{framework}@{SEEDED_VERSION}"),
    ]
}

/// A `FrameworkFilters` member of a framework OTHER than the one the request's
/// `FrameworkId` names is refused with the modelled `ValidationException` and its 422,
/// naming the member and the framework asked for — never a 200 with the empty page the
/// two predicates would have selected together.
///
/// The rows differ along what the rule generalises over: which framework is named, which
/// member is set, whether the framework is named unversioned or at a version, whether
/// the member carries a delivery option, and whether another filter accompanies them.
#[tokio::test]
async fn framework_filters_of_another_framework_is_refused() {
    let router = emulator_holding_one_assessment_per_framework();

    // The store HOLDS one assessment per framework, so an empty page is something this
    // catalog could otherwise have answered.
    let every = list_assessments(&router, json!({"Catalog": CATALOG, "MaxResults": 100})).await;
    assert_eq!(
        every.listed_ids().len(),
        FRAMEWORK_MEMBERS.len(),
        "the seed must hold one assessment per published framework, got {:?}",
        every.body
    );

    for &(named, _) in FRAMEWORK_MEMBERS {
        for &(filtered, member) in FRAMEWORK_MEMBERS {
            if named == filtered {
                continue;
            }
            for framework_id in framework_id_forms(named) {
                // Both the bare member and one carrying a delivery option that the
                // assessment of ITS framework really has, so the refusal is about the
                // frameworks contradicting each other and nothing else.
                let payloads = [
                    json!({}),
                    json!({"DeliveryOptionId": seeded_delivery_option(filtered)}),
                ];
                for payload in payloads {
                    let requests = [
                        json!({
                            "Catalog": CATALOG,
                            "MaxResults": 100,
                            "FrameworkId": framework_id,
                            "FrameworkFilters": {member: payload},
                        }),
                        json!({
                            "Catalog": CATALOG,
                            "MaxResults": 100,
                            "FrameworkId": framework_id,
                            "FrameworkFilters": {member: payload},
                            "AssessmentTargetFilter": {"EntityId": "ent-1"},
                        }),
                    ];
                    for request in requests {
                        let case =
                            format!("FrameworkId {framework_id:?} with {member} in {request}");
                        let answer = list_assessments(&router, request.clone()).await;
                        assert_eq!(
                            (answer.status, answer.error_code()),
                            (422, "ValidationException"),
                            "case {case:?}: expected 422 ValidationException, got {} {:?} \
                             with {:?}",
                            answer.status,
                            answer.error_code(),
                            answer.body
                        );
                        let message = answer.message();
                        assert!(
                            message.contains(member),
                            "case {case:?}: the fault must name the member {member} that was \
                             set, got {message:?}"
                        );
                        assert!(
                            message.contains(framework_id.as_str()),
                            "case {case:?}: the fault must name the framework asked for \
                             {framework_id:?}, got {message:?}"
                        );
                        let served = answer.listed_ids();
                        assert!(
                            served.is_empty(),
                            "case {case:?}: a refused call serves no page, got {served:?} \
                             in {:?}",
                            answer.body
                        );
                    }
                }
            }
        }
    }

    // The refusals changed nothing: the seeded assessments are still all there.
    let after = list_assessments(&router, json!({"Catalog": CATALOG, "MaxResults": 100})).await;
    assert_eq!(
        after.listed_ids().len(),
        FRAMEWORK_MEMBERS.len(),
        "the refusals must leave the catalog as it was, got {:?}",
        after.body
    );
}

/// A `FrameworkFilters` member of the framework the request names agrees with it, so it
/// is served: the page is read from state, scoped to that framework and narrowed by the
/// delivery option the member carries.
///
/// The rows differ along what the rule generalises over: which framework is named, in
/// which of the two forms, whether a delivery option is carried, and whether the
/// framework is named at all — a member set on its own constrains the page alone.
#[tokio::test]
async fn framework_filters_of_the_framework_named_is_served() {
    let router = emulator_holding_one_assessment_per_framework();

    for &(framework, member) in FRAMEWORK_MEMBERS {
        let held = seeded_id(framework);
        let option = seeded_delivery_option(framework);
        let other = FRAMEWORK_MEMBERS
            .iter()
            .map(|(framework, _)| seeded_delivery_option(framework))
            .find(|candidate| candidate != &option)
            .expect("another framework's delivery option");

        // (case, the FrameworkId to send or none, the member's payload, the ids expected)
        let mut rows: Vec<(String, Option<String>, Value, Vec<String>)> = vec![
            (
                format!("{member} on its own"),
                None,
                json!({}),
                vec![held.clone()],
            ),
            (
                format!("{member} on its own, narrowed to its delivery option"),
                None,
                json!({"DeliveryOptionId": option}),
                vec![held.clone()],
            ),
            (
                format!("{member} on its own, narrowed to another framework's option"),
                None,
                json!({"DeliveryOptionId": other}),
                Vec::new(),
            ),
        ];
        for framework_id in framework_id_forms(framework) {
            rows.push((
                format!("{member} beside FrameworkId {framework_id:?}"),
                Some(framework_id.clone()),
                json!({}),
                vec![held.clone()],
            ));
            rows.push((
                format!(
                    "{member} beside FrameworkId {framework_id:?}, narrowed to its \
                     delivery option"
                ),
                Some(framework_id.clone()),
                json!({"DeliveryOptionId": option}),
                vec![held.clone()],
            ));
            rows.push((
                format!(
                    "{member} beside FrameworkId {framework_id:?}, narrowed to a delivery \
                     option this catalog assessed for no one"
                ),
                Some(framework_id.clone()),
                json!({"DeliveryOptionId": "do-never-assessed"}),
                Vec::new(),
            ));
        }

        for (case, framework_id, payload, expected) in rows {
            let mut request = json!({
                "Catalog": CATALOG,
                "MaxResults": 100,
                "FrameworkFilters": {member: payload},
            });
            if let Some(framework_id) = framework_id {
                request["FrameworkId"] = json!(framework_id);
            }
            let answer = list_assessments(&router, request.clone()).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a member agreeing with the framework named must be served, \
                 got {} with {:?}",
                answer.status, answer.body
            );
            let ids = answer.listed_ids();
            assert_eq!(
                ids, expected,
                "case {case:?}: expected {expected:?} for {request}, got {ids:?} from {:?}",
                answer.body
            );
        }
    }
}
