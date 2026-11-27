// SPDX-License-Identifier: CC0-1.0

//! The assessments `StartAssessment` STARTS, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. Nothing seeds the store: the entities assessed are the ones the change
//! sets started below created, and the assessments read back are the ones
//! `StartAssessment` started over them.
//!
//! The rules under test:
//!
//! * an assessment of an entity runs every control of the framework asked for — over the
//!   input the request supplied for that control, which is the caller's own attestation,
//!   or over the delivery option the entity delivers, which is the catalog's own finding —
//!   and the result of each control is what evaluating those members found;
//! * the assessment runs asynchronously: the start answers an id immediately, and the
//!   result and the control results are reported once the run has finished, which is also
//!   when the entity it assessed stops being in use;
//! * a start outside the operation's contract is refused and starts nothing.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::state::OPEN_ASSESSMENT_LIMIT;
use service_com_amazonaws_marketplacecatalog_awsmpseymour::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The catalog every operation's `Catalog` member documents as its fixed value.
const CATALOG: &str = "AWSMarketplace";

/// A SigV4 credential scope naming this service's signing name (`aws-marketplace`), which
/// is what routes a request here.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The controls of the AMI Security framework, in the order an assessment reports them.
/// Spelled out here rather than imported from the crate, so a renamed or reordered
/// control fails this test instead of following it.
const AMI_CONTROLS: &[&str] = &[
    "AmiSourceImageDeclared",
    "AmiSourceAccessRoleDeclared",
    "AmiSourceLoginUserIsNotSuperuser",
    "AmiSecurityGroupIngressIsScoped",
    "AmiUsageInstructionsDeclared",
];

/// The controls of the Container Security framework, in the same way.
const CONTAINER_CONTROLS: &[&str] = &[
    "ContainerImagesDeclared",
    "ContainerImagesArePinned",
    "ContainerUsageInstructionsDeclared",
];

/// A `VerificationSource` value: the catalog evaluated the control itself
/// (`smithy_query(query="shape", shape="VerificationSource")`).
const AWS_VERIFIED: &str = "AWS_VERIFIED";

/// The other `VerificationSource` value: the caller attested to the control's result.
const SELF_ATTESTED: &str = "SELF_ATTESTED";

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

    /// The fault message, however the envelope spells the member.
    fn message(&self) -> String {
        let message = self.text("/Message");
        if message.is_empty() {
            return self.text("/message");
        }
        message
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

/// One delivery option's details, in the form a change submits them: the delivery form's
/// own members wrapped in the member naming that form.
fn delivery_option(form: &str, details: Value) -> Value {
    json!({"Details": {form: details}})
}

/// An AMI delivery option whose login user is `user_name` and whose single ingress rule
/// admits `ip_range`, with every other member a satisfied control reads.
fn ami_option(user_name: &str, ip_range: &str) -> Value {
    delivery_option(
        "AmiDeliveryOptionDetails",
        json!({
            "AmiSource": {
                "AmiId": "ami-0abcdef1234567890",
                "AccessRoleArn": "arn:aws:iam::123456789012:role/MarketplaceScan",
                "UserName": user_name,
            },
            "SecurityGroups": [{
                "IpProtocol": "tcp",
                "IpRanges": [ip_range],
                "FromPort": 22,
                "ToPort": 22,
            }],
            "UsageInstructions": "Connect over SSH and run /opt/product/start.",
        }),
    )
}

/// A container delivery option delivering `images`.
fn container_option(images: Value) -> Value {
    delivery_option(
        "EcrDeliveryOptionDetails",
        json!({
            "ContainerImages": images,
            "UsageInstructions": "helm install product ./chart",
        }),
    )
}

/// The details of a change that submits `options` as a product version.
fn version_with(options: Vec<Value>) -> Value {
    json!({
        "Version": {"VersionTitle": "1.0", "ReleaseNotes": "first release"},
        "DeliveryOptions": options,
    })
}

/// Create one entity per `(entity_type, details)` through ONE change set, wait for that
/// change set to succeed, and answer the entity ids in the order asked for.
async fn created_entities(router: &MultiServiceRouter, specs: &[(&str, Value)]) -> Vec<String> {
    let changes: Vec<Value> = specs
        .iter()
        .map(|(entity_type, details)| {
            json!({
                "ChangeType": "AddDeliveryOptions",
                "Entity": {"Type": format!("{entity_type}@1.0")},
                "DetailsDocument": details,
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

    // The change set is asynchronous, so the fixture waits for the CONDITION it needs —
    // the change set having succeeded, which is what makes its entities exist — and reads
    // the entities it created off its own summary.
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
                specs.len(),
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

/// Start an assessment of `entity_id` against `framework`, without judging the answer.
async fn try_start(
    router: &MultiServiceRouter,
    entity_id: &str,
    framework: &str,
    extra: Value,
) -> Answer {
    let mut request = json!({
        "Catalog": CATALOG,
        "EntityId": entity_id,
        "FrameworkId": framework,
    });
    if let (Value::Object(into), Value::Object(members)) = (&mut request, extra) {
        for (key, value) in members {
            into.insert(key, value);
        }
    }
    call(router, "POST", "/StartAssessment", Some(request)).await
}

/// The same, asserting the start was accepted, and answer the assessment id.
async fn start_assessment(
    router: &MultiServiceRouter,
    entity_id: &str,
    framework: &str,
    extra: Value,
) -> String {
    let answer = try_start(router, entity_id, framework, extra).await;
    assert_eq!(
        answer.status, 200,
        "starting an assessment of {entity_id} against {framework} must be accepted, \
         got {} with {:?}",
        answer.status, answer.body
    );
    let id = answer.text("/AssessmentId");
    assert!(
        id.starts_with("assessment-"),
        "the start must answer the id of the assessment it started, got {:?}",
        answer.body
    );
    id
}

/// `DescribeAssessment` of `assessment_id`.
async fn describe_assessment(router: &MultiServiceRouter, assessment_id: &str) -> Answer {
    let answer = call(
        router,
        "POST",
        "/DescribeAssessment",
        Some(json!({"Catalog": CATALOG, "AssessmentIdentifier": assessment_id})),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "describing the started assessment {assessment_id} must answer 200, got {} \
         with {:?}",
        answer.status, answer.body
    );
    answer
}

/// `DescribeAssessment` of `assessment_id` once its run has finished: the report that
/// carries an `AssessmentResult`.
async fn finished_assessment(router: &MultiServiceRouter, assessment_id: &str) -> Answer {
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let answer = describe_assessment(router, assessment_id).await;
        if !answer.text("/AssessmentResult").is_empty() {
            return answer;
        }
        assert!(
            Instant::now() < deadline,
            "the assessment {assessment_id} must finish and report a result, got {:?}",
            answer.body
        );
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// The `(ControlId, ControlAssessmentResult, VerificationSource)` triples a report
/// carried.
fn control_results(answer: &Answer) -> Vec<(String, String, String)> {
    answer
        .list("/ControlAssessments")
        .iter()
        .map(|control| {
            (
                control["ControlId"].as_str().unwrap_or_default().to_string(),
                control["ControlAssessmentResult"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                control["VerificationSource"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// How many assessments of `entity_id` this catalog holds.
async fn assessments_of(router: &MultiServiceRouter, entity_id: &str) -> Vec<Value> {
    let listed = call(
        router,
        "POST",
        "/ListAssessments",
        Some(json!({
            "Catalog": CATALOG,
            "MaxResults": 100,
            "AssessmentTargetFilter": {"EntityId": entity_id},
        })),
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "listing the assessments of {entity_id} must answer 200, got {} with {:?}",
        listed.status, listed.body
    );
    listed.list("/AssessmentSummaryList")
}

/// A started assessment runs every control of the framework over the delivery option the
/// entity delivers, reports no result while it is running, and reports what the controls
/// found once it has finished.
///
/// The rows differ along what the rule generalises over: which product type and so which
/// framework, which member of the delivery option is at fault, whether the fault is a
/// refused value or a missing member, and whether the entity delivers an option at all.
#[tokio::test]
async fn a_started_assessment_runs_the_framework_over_the_entity_it_names() {
    let router = emulator();

    // (case, entity type, framework, the details the entity is created with, the control
    // results the finished report must carry, in order)
    let rows: Vec<(&str, &str, &str, Value, Vec<&str>)> = vec![
        (
            "an AMI option that satisfies every control",
            "AmiProduct",
            "AMISecurity",
            version_with(vec![ami_option("ec2-user", "10.0.0.0/16")]),
            vec!["PASS", "PASS", "PASS", "PASS", "PASS"],
        ),
        (
            "an AMI option offering the superuser as its login user",
            "AmiProduct",
            "AMISecurity",
            version_with(vec![ami_option("root", "10.0.0.0/16")]),
            vec!["PASS", "PASS", "FAIL", "PASS", "PASS"],
        ),
        (
            "an AMI option whose ingress admits every address",
            "AmiProduct",
            "AMISecurity",
            version_with(vec![ami_option("ec2-user", "0.0.0.0/0")]),
            vec!["PASS", "PASS", "PASS", "FAIL", "PASS"],
        ),
        (
            "an AMI product delivering no option at all",
            "AmiProduct",
            "AMISecurity",
            json!({"ProductTitle": "an unreleased product"}),
            vec!["FAIL", "FAIL", "NOT_EXECUTED", "NOT_EXECUTED", "FAIL"],
        ),
        (
            "a container option pinned by digest",
            "ContainerProduct",
            "ContainerSecurity",
            version_with(vec![container_option(json!([
                "123456789012.dkr.ecr.eu-west-1.amazonaws.com/product@sha256:0123456789abcdef"
            ]))]),
            vec!["PASS", "PASS", "PASS"],
        ),
        (
            "a container option on a floating tag",
            "ContainerProduct",
            "ContainerSecurity",
            version_with(vec![container_option(json!([
                "123456789012.dkr.ecr.eu-west-1.amazonaws.com/product:latest"
            ]))]),
            vec!["PASS", "FAIL", "PASS"],
        ),
    ];

    let specs: Vec<(&str, Value)> = rows
        .iter()
        .map(|(_, entity_type, _, details, _)| (*entity_type, details.clone()))
        .collect();
    let entities = created_entities(&router, &specs).await;

    // Every assessment is started first, so each one is observed while it is still
    // running before any of them is waited for.
    let mut started: Vec<String> = Vec::with_capacity(rows.len());
    for ((case, _, framework, _, _), entity_id) in rows.iter().zip(entities.iter()) {
        let assessment_id = start_assessment(&router, entity_id, framework, json!({})).await;
        let running = describe_assessment(&router, &assessment_id).await;
        assert_eq!(
            (
                running.text("/AssessmentResult"),
                running.list("/ControlAssessments").len(),
                running.text("/AssessmentTargetSummary/EntityId"),
                running.text("/FrameworkId"),
            ),
            (
                String::new(),
                0,
                entity_id.clone(),
                format!("{framework}@1.0"),
            ),
            "case {case:?}: a running assessment must report its target and framework and \
             neither a result nor a control result, got {:?}",
            running.body
        );
        assert_eq!(
            running.body.pointer("/AssessmentTargetSummary/ChangeSetId"),
            None,
            "case {case:?}: an assessment of an entity names no change set, got {:?}",
            running.body
        );
        started.push(assessment_id);
    }

    for ((case, _, framework, _, expected), (entity_id, assessment_id)) in
        rows.iter().zip(entities.iter().zip(started.iter()))
    {
        let finished = finished_assessment(&router, assessment_id).await;
        let controls = if *framework == "AMISecurity" {
            AMI_CONTROLS
        } else {
            CONTAINER_CONTROLS
        };
        let expected_controls: Vec<(String, String, String)> = controls
            .iter()
            .zip(expected.iter())
            .map(|(control, result)| {
                (
                    (*control).to_string(),
                    (*result).to_string(),
                    AWS_VERIFIED.to_string(),
                )
            })
            .collect();
        assert_eq!(
            control_results(&finished),
            expected_controls,
            "case {case:?}: the finished assessment must report every control of the \
             framework with what evaluating the entity's own delivery option found, got \
             {:?}",
            finished.body
        );
        let expected_result = if expected.contains(&"FAIL") {
            "FAIL"
        } else {
            "PASS"
        };
        assert_eq!(
            finished.text("/AssessmentResult"),
            expected_result,
            "case {case:?}: the assessment result must be FAIL exactly when a control \
             failed, got {:?}",
            finished.body
        );

        // The started assessment is one of the entity's own: `ListAssessments` reports it
        // under the target it names.
        let listed = assessments_of(&router, entity_id).await;
        let found = listed
            .iter()
            .find(|summary| summary["AssessmentId"].as_str() == Some(assessment_id.as_str()))
            .unwrap_or_else(|| {
                panic!("case {case:?}: the started assessment must be listed, got {listed:?}")
            });
        assert_eq!(
            (
                found["AssessmentResult"].as_str().unwrap_or_default(),
                found["FrameworkId"].as_str().unwrap_or_default(),
            ),
            (expected_result, format!("{framework}@1.0").as_str()),
            "case {case:?}: the summary must report what the record holds, got {found:?}"
        );
    }
}

/// A control the request supplies input for is assessed from THAT input and reported as
/// the caller's own attestation, while every other control of the framework is assessed
/// from the delivery option the entity delivers and reported as the catalog's finding.
///
/// The rows differ along which control is given input, what that input says, and whether
/// the attested result is a pass or a failure.
#[tokio::test]
async fn a_control_given_input_is_assessed_from_it_and_reported_self_attested() {
    let router = emulator();

    // Every row assesses an AMI product whose stored option offers the superuser and
    // opens its ingress to every address, so each attested control is visibly judged
    // against the INPUT and not against what the entity holds.
    let stored = version_with(vec![ami_option("root", "0.0.0.0/0")]);
    // (case, the control given input, that input, the result that control must report)
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "a login user the caller attests is not the superuser",
            "AmiSourceLoginUserIsNotSuperuser",
            json!({"AmiSource": {"UserName": "ec2-user"}}),
            "PASS",
        ),
        (
            "a login user the caller attests IS the superuser",
            "AmiSourceLoginUserIsNotSuperuser",
            json!({"AmiSource": {"UserName": "root"}}),
            "FAIL",
        ),
        (
            "an ingress range the caller attests is scoped",
            "AmiSecurityGroupIngressIsScoped",
            json!({"SecurityGroups": [{"IpRanges": ["192.168.0.0/24"]}]}),
            "PASS",
        ),
        (
            "an input that declares nothing the control reads",
            "AmiSourceImageDeclared",
            json!({}),
            "FAIL",
        ),
    ];

    let specs: Vec<(&str, Value)> = rows
        .iter()
        .map(|_| ("AmiProduct", stored.clone()))
        .collect();
    let entities = created_entities(&router, &specs).await;

    for ((case, control, input, expected), entity_id) in rows.iter().zip(entities.iter()) {
        let assessment_id = start_assessment(
            &router,
            entity_id,
            "AMISecurity",
            json!({"Controls": [{"ControlId": control, "Input": input}]}),
        )
        .await;
        let finished = finished_assessment(&router, &assessment_id).await;
        let results = control_results(&finished);
        assert_eq!(
            results.len(),
            AMI_CONTROLS.len(),
            "case {case:?}: every control of the framework is included in the assessment, \
             got {results:?}"
        );
        for (control_id, result, source) in &results {
            let attested = control_id == control;
            let expected_source = if attested { SELF_ATTESTED } else { AWS_VERIFIED };
            assert_eq!(
                source, expected_source,
                "case {case:?}: control {control_id} must report the source its result came \
                 from, got {results:?}"
            );
            if attested {
                assert_eq!(
                    result, expected,
                    "case {case:?}: the attested control must report what evaluating the \
                     supplied input found, got {results:?}"
                );
            }
        }
        // The controls the request said nothing about are still the catalog's own
        // findings about the stored option, which offers the superuser.
        let login = results
            .iter()
            .find(|(control_id, _, _)| control_id == "AmiSourceLoginUserIsNotSuperuser")
            .expect("the framework runs its login-user control");
        if *control != "AmiSourceLoginUserIsNotSuperuser" {
            assert_eq!(
                (login.1.as_str(), login.2.as_str()),
                ("FAIL", AWS_VERIFIED),
                "case {case:?}: a control the request left unspecified must report what the \
                 entity's own option holds, got {results:?}"
            );
        }
    }
}

/// An entity being assessed is in use: a second start against it is refused while the
/// first run lasts, and accepted once that run has finished.
#[tokio::test]
async fn an_entity_already_being_assessed_is_in_use() {
    let router = emulator();
    let entities = created_entities(
        &router,
        &[(
            "AmiProduct",
            version_with(vec![ami_option("ec2-user", "10.0.0.0/16")]),
        )],
    )
    .await;
    let entity_id = &entities[0];

    let first = start_assessment(&router, entity_id, "AMISecurity", json!({})).await;
    let refused = try_start(&router, entity_id, "AMISecurity", json!({})).await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (423, "ResourceInUseException"),
        "a second start while the first assessment runs must be refused 423 \
         ResourceInUseException, got {} with {:?}",
        refused.status,
        refused.body
    );
    let message = refused.message();
    assert!(
        message.contains(entity_id) && message.contains(&first),
        "the refusal must name the entity in use and the assessment holding it, got \
         {message:?}"
    );

    // The refused start left nothing behind: the entity carries the one started
    // assessment and the one the change set produced.
    let while_running: Vec<String> = assessments_of(&router, entity_id)
        .await
        .iter()
        .map(|summary| {
            summary["AssessmentId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        while_running.len(),
        2,
        "the refused start must have started nothing, got {while_running:?}"
    );

    // Once the run has finished the entity is free again, so the next start is served.
    let _ = finished_assessment(&router, &first).await;
    let second = start_assessment(&router, entity_id, "AMISecurity", json!({})).await;
    assert_ne!(
        second, first,
        "a start after the first run finished must start a new assessment"
    );
}

/// The same `ClientToken` names the same assessment, so a retried start reports the first
/// one rather than starting a second.
#[tokio::test]
async fn one_client_token_starts_one_assessment() {
    let router = emulator();
    let entities = created_entities(
        &router,
        &[(
            "AmiProduct",
            version_with(vec![ami_option("ec2-user", "10.0.0.0/16")]),
        )],
    )
    .await;
    let entity_id = &entities[0];

    let token = json!({"ClientToken": "token-abc-123"});
    let first = start_assessment(&router, entity_id, "AMISecurity", token.clone()).await;
    let retried = start_assessment(&router, entity_id, "AMISecurity", token.clone()).await;
    assert_eq!(
        retried, first,
        "a retried start must report the assessment the token already started"
    );

    // Still only the one started assessment beside the one the change set produced — and
    // the retry is served even though the entity is in use, because it names that same
    // assessment.
    let held = assessments_of(&router, entity_id).await;
    assert_eq!(
        held.len(),
        2,
        "the retried start must have started nothing, got {held:?}"
    );

    // A DIFFERENT token after the run has finished starts a second assessment.
    let _ = finished_assessment(&router, &first).await;
    let other = start_assessment(
        &router,
        entity_id,
        "AMISecurity",
        json!({"ClientToken": "token-def-456"}),
    )
    .await;
    assert_ne!(
        other, first,
        "another token must start an assessment of its own"
    );
}

/// The tags a start applies are the tags the assessment carries, reported and updated
/// through the assessment's own ARN.
#[tokio::test]
async fn a_started_assessment_carries_the_tags_the_start_applied() {
    let router = emulator();
    let entities = created_entities(
        &router,
        &[(
            "AmiProduct",
            version_with(vec![ami_option("ec2-user", "10.0.0.0/16")]),
        )],
    )
    .await;

    let assessment_id = start_assessment(
        &router,
        &entities[0],
        "AMISecurity",
        json!({"Tags": {"Team": "catalog", "Stage": "test"}}),
    )
    .await;
    let arn = describe_assessment(&router, &assessment_id)
        .await
        .text("/AssessmentArn");
    assert!(
        arn.contains(&format!("/Assessment/{assessment_id}")),
        "the report must carry the assessment's own ARN, got {arn:?}"
    );

    let listed = call(
        &router,
        "POST",
        "/ListTagsForResource",
        Some(json!({"ResourceArn": arn})),
    )
    .await;
    assert_eq!(
        (listed.status, tag_pairs(&listed)),
        (
            200,
            vec![
                ("Stage".to_string(), "test".to_string()),
                ("Team".to_string(), "catalog".to_string()),
            ]
        ),
        "the tags the start applied must be reported for the assessment, got {} with {:?}",
        listed.status,
        listed.body
    );

    // The tagging surface acts on the same resource: a further tag joins the set, and one
    // removed leaves it.
    let tagged = call(
        &router,
        "POST",
        "/TagResource",
        Some(json!({"ResourceArn": arn, "Tags": [{"Key": "Owner", "Value": "sre"}]})),
    )
    .await;
    assert_eq!(
        tagged.status, 200,
        "tagging the assessment must answer 200, got {} with {:?}",
        tagged.status, tagged.body
    );
    let untagged = call(
        &router,
        "POST",
        "/UntagResource",
        Some(json!({"ResourceArn": arn, "TagKeys": ["Stage"]})),
    )
    .await;
    assert_eq!(
        untagged.status, 200,
        "untagging the assessment must answer 200, got {} with {:?}",
        untagged.status, untagged.body
    );
    let after = call(
        &router,
        "POST",
        "/ListTagsForResource",
        Some(json!({"ResourceArn": arn})),
    )
    .await;
    assert_eq!(
        tag_pairs(&after),
        vec![
            ("Owner".to_string(), "sre".to_string()),
            ("Team".to_string(), "catalog".to_string()),
        ],
        "the assessment must report the set the tag writes left, got {:?}",
        after.body
    );
}

/// The `(Key, Value)` pairs a `ListTagsForResource` answer carried, sorted by key so the
/// assertion is about the SET and not about the order a map was written in.
fn tag_pairs(answer: &Answer) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = answer
        .list("/Tags")
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    pairs.sort();
    pairs
}

/// A start outside the operation's contract is refused, and starts nothing.
///
/// The rows span the conditions the operation states: the fixed catalog, the frameworks
/// this catalog assesses against, the entity the request names, the controls the framework
/// defines, the input a control is given, the tag namespace AWS keeps, and the published
/// token syntax.
#[tokio::test]
async fn a_start_outside_the_contract_is_refused_and_starts_nothing() {
    let router = emulator();
    let entities = created_entities(
        &router,
        &[
            (
                "AmiProduct",
                version_with(vec![ami_option("ec2-user", "10.0.0.0/16")]),
            ),
            ("Offer", json!({"Name": "an offer", "ProductId": "prod-1"})),
        ],
    )
    .await;
    let (product, offer) = (entities[0].clone(), entities[1].clone());

    // (case, the request members beside Catalog, the status and the error code expected)
    let rows: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a catalog other than the fixed value",
            json!({"Catalog": "OtherCatalog", "EntityId": product, "FrameworkId": "AMISecurity"}),
            422,
            "ValidationException",
        ),
        (
            "a framework this catalog does not assess against",
            json!({"EntityId": product, "FrameworkId": "FooSecurity"}),
            422,
            "ValidationException",
        ),
        (
            "an entity id that is not an entity id",
            json!({"EntityId": "not an entity id", "FrameworkId": "AMISecurity"}),
            422,
            "ValidationException",
        ),
        (
            "a client token outside the published syntax",
            json!({
                "EntityId": product,
                "FrameworkId": "AMISecurity",
                "ClientToken": "token with spaces",
            }),
            422,
            "ValidationException",
        ),
        (
            "a control the framework does not define",
            json!({
                "EntityId": product,
                "FrameworkId": "AMISecurity",
                "Controls": [{"ControlId": "ContainerImagesDeclared"}],
            }),
            422,
            "ValidationException",
        ),
        (
            "one control given input twice",
            json!({
                "EntityId": product,
                "FrameworkId": "AMISecurity",
                "Controls": [
                    {"ControlId": "AmiSourceImageDeclared", "Input": {"AmiSource": {"AmiId": "ami-1"}}},
                    {"ControlId": "AmiSourceImageDeclared", "Input": {"AmiSource": {"AmiId": "ami-2"}}},
                ],
            }),
            422,
            "ValidationException",
        ),
        (
            "a control input that is not an object",
            json!({
                "EntityId": product,
                "FrameworkId": "AMISecurity",
                "Controls": [{"ControlId": "AmiSourceImageDeclared", "Input": "ami-1"}],
            }),
            422,
            "ValidationException",
        ),
        (
            "a tag key inside the namespace AWS keeps",
            json!({
                "EntityId": product,
                "FrameworkId": "AMISecurity",
                "Tags": {"aws:created-by": "me"},
            }),
            422,
            "ValidationException",
        ),
        (
            "an entity this catalog does not hold",
            json!({"EntityId": "ent-000000000000", "FrameworkId": "AMISecurity"}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a framework that does not assess the entity's type",
            json!({"EntityId": product, "FrameworkId": "ContainerSecurity"}),
            422,
            "ValidationException",
        ),
        (
            "an entity type no framework of this catalog assesses",
            json!({"EntityId": offer, "FrameworkId": "AMISecurity"}),
            422,
            "ValidationException",
        ),
    ];

    let before = assessments_of(&router, &product).await.len();
    for (case, members, status, code) in rows {
        let mut request = json!({"Catalog": CATALOG});
        if let (Value::Object(into), Value::Object(members)) = (&mut request, members) {
            for (key, value) in members {
                into.insert(key, value);
            }
        }
        let refused = call(&router, "POST", "/StartAssessment", Some(request.clone())).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (status, code),
            "case {case:?}: {request} must be refused {status} {code}, got {} with {:?}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.body.pointer("/AssessmentId"),
            None,
            "case {case:?}: a refused start names no assessment, got {:?}",
            refused.body
        );
        let after = assessments_of(&router, &product).await.len();
        assert_eq!(
            after, before,
            "case {case:?}: a refused start must leave the assessments the catalog holds \
             alone, got {after} where {before} were held"
        );
    }
}

/// The assessments one account may have running at once are bounded: at the bound a
/// further start is refused with the quota fault this operation models.
#[tokio::test]
async fn the_open_assessments_of_an_account_are_bounded() {
    let router = emulator();
    // One entity per start, since an entity being assessed is in use — and one more than
    // the bound, so the last start is the one past it. The entities deliver no option, so
    // creating them produces no assessment of their own.
    let specs: Vec<(&str, Value)> = (0..=OPEN_ASSESSMENT_LIMIT)
        .map(|index| {
            (
                "AmiProduct",
                json!({"ProductTitle": format!("product {index}")}),
            )
        })
        .collect();
    let entities = created_entities(&router, &specs).await;

    for (index, entity_id) in entities.iter().enumerate().take(OPEN_ASSESSMENT_LIMIT) {
        let started = try_start(&router, entity_id, "AMISecurity", json!({})).await;
        assert_eq!(
            started.status,
            200,
            "start {index} is inside the bound of {OPEN_ASSESSMENT_LIMIT} and must be \
             accepted, got {} with {:?}",
            started.status,
            started.body
        );
    }

    let refused = try_start(
        &router,
        &entities[OPEN_ASSESSMENT_LIMIT],
        "AMISecurity",
        json!({}),
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (402, "ServiceQuotaExceededException"),
        "a start past the bound of {OPEN_ASSESSMENT_LIMIT} running assessments must be \
         refused 402 ServiceQuotaExceededException, got {} with {:?}",
        refused.status,
        refused.body
    );
    assert!(
        refused.message().contains(&OPEN_ASSESSMENT_LIMIT.to_string()),
        "the refusal must name the bound it reports, got {:?}",
        refused.message()
    );
    assert!(
        assessments_of(&router, &entities[OPEN_ASSESSMENT_LIMIT])
            .await
            .is_empty(),
        "the refused start must have started nothing for the entity it named"
    );
}
