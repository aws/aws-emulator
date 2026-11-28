// SPDX-License-Identifier: CC0-1.0

//! Clean Rooms ML's resolution of the resource a job start REFERENCES, and the wording of the
//! refusal it answers when that reference resolves to nothing.
//!
//! RULE: a Clean Rooms ML job start names the resource the job runs — the configured audience
//! model an audience generation job generates an audience from, the trained model an inference
//! job runs predictions with. That reference is resolved, and the policies on the resource it
//! names asked whether they allow the action, BEFORE anything is reported about the shape of
//! the request: a reference that resolves to nothing has no resource-based policy that could
//! allow the call, so it is refused `AccessDeniedException` (HTTP 403) stating the reference
//! and the IAM action the call asks for — even when the request also carries a member outside
//! its modelled constraints, which is what the emulator used to answer first with a 400
//! `ValidationException`. A reference that DOES resolve is left to the modelled walk, which
//! still refuses such a member with its own 400: the authorization decision moved ahead of
//! validation, it did not replace it.
//!
//! What resolving a reference comes to belongs to the reference, and each refusal states what
//! failed: a configured audience model is resolved from the value itself — whether it names a
//! configured audience model of this service at all — and is stated INVALID when it names no
//! such resource; a trained model is resolved through the configured model algorithm
//! association it runs through, which an endpoint either holds or does not, and that
//! association is stated NOT FOUND when it holds none.
//!
//! This suite lives here because the chain the inference job's reference is resolved through
//! spans two services: a trained model is reached through the configured model algorithm
//! association it was trained through, and an association exists only under a Clean Rooms
//! MEMBERSHIP — so the collaboration and the membership below are created through Clean Rooms'
//! own `CreateCollaboration`/`CreateMembership`, and the association through Clean Rooms ML's
//! own create, on the one shared engine. What the gate reads is what those services persisted.
//!
//! The rows differ along what the rule generalises over: which operation is called, which
//! reference the request names, the partition, region, account and resource id that reference
//! carries, whether it names a resource of the right kind at all, whether this endpoint's
//! region is the one that holds a held resource, which association the request names for a
//! model, and which constraint-violating member stands beside the reference.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_cleanroomsml_awsstarkcontrolservice::state::{
    trained_model_arn, ComAmazonawsCleanroomsmlAwsstarkcontrolserviceState, TrainedModel,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for, unless a case signs for another.
const REGION: &str = "us-west-2";

/// A second endpoint, for the cases that pin that a Clean Rooms ML endpoint resolves the
/// resources created under its own region and no others.
const OTHER_REGION: &str = "eu-west-1";

/// The account every emulated resource belongs to.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// An IAM role ARN matching `IamRoleArn`'s `^arn:aws[-a-z]*:iam::[0-9]{12}:role/.+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/CleanRoomsMLRole";

/// One value `InferenceInstanceType` admits
/// (`smithy_query(query="shape", shape="InferenceInstanceType")`).
const INSTANCE_TYPE: &str = "ml.m5.large";

/// The largest `InferenceResourceConfig$instanceCount` admits (`@range(min: 1, max: 10)`),
/// spelled out rather than imported so a wrong ceiling in the service fails here.
const MAX_INSTANCE_COUNT: i64 = 10;

/// The largest `NameString` admits (`@length(min: 1, max: 63)`), for the rows that stand a
/// constraint violation beside the reference.
const MAX_NAME: usize = 63;

/// The largest `ResourceDescription` admits (`@length(max: 255)`), for the same reason.
const MAX_DESCRIPTION: usize = 255;

/// One emulator carrying Clean Rooms ML and Clean Rooms over ONE shared engine, exactly as the
/// production router builds them, plus the handle to that engine — the store the service reads
/// is the store a case seeds.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cleanroomsml_awsstarkcontrolservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_cleanrooms_awsbastioncontrolplaneservicelambda::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

/// One restJson1 call to `signing_name` in `region`, answered with the HTTP status and the JSON
/// body. The SigV4 credential scope is both the routing key and the region the service resolves
/// its resources under.
async fn call(
    router: &MultiServiceRouter,
    signing_name: &str,
    region: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let host = format!("{signing_name}.{region}.amazonaws.com");
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=abc"
            ),
        );
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// `POST /audience-generation-job` — `StartAudienceGenerationJob`.
async fn start_audience_job(
    router: &MultiServiceRouter,
    region: &str,
    body: Value,
) -> (u16, Value) {
    call(
        router,
        "cleanrooms-ml",
        region,
        "POST",
        "/audience-generation-job",
        Some(body),
    )
    .await
}

/// `POST /memberships/{membershipIdentifier}/trained-model-inference-jobs` —
/// `StartTrainedModelInferenceJob`.
async fn start_inference_job(
    router: &MultiServiceRouter,
    region: &str,
    membership: &str,
    body: Value,
) -> (u16, Value) {
    call(
        router,
        "cleanrooms-ml",
        region,
        "POST",
        &format!("/memberships/{membership}/trained-model-inference-jobs"),
        Some(body),
    )
    .await
}

/// The wire error code of a failure body (`__type`, possibly namespace-qualified).
fn error_code(payload: &Value) -> String {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `message` of a failure body.
fn error_message(payload: &Value) -> String {
    payload
        .get("message")
        .or_else(|| payload.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// Assert `payload` is the modelled 403 `AccessDeniedException` carrying `expected`.
fn assert_access_denied(case: &str, status: u16, payload: &Value, expected: &str) {
    assert_eq!(
        status, 403,
        "case {case:?}: expected HTTP 403, got {status} {payload}"
    );
    assert_eq!(
        error_code(payload),
        "AccessDeniedException",
        "case {case:?}: expected the modelled authorization refusal, got {payload}"
    );
    assert_eq!(
        error_message(payload),
        expected,
        "case {case:?}: expected {expected:?}, got {:?}",
        error_message(payload)
    );
}

/// Assert `payload` is a 400 `ValidationException` whose clause names `member` — the modelled
/// refusal a member outside its constraints still produces once the reference resolves.
fn assert_validation_error(case: &str, status: u16, payload: &Value, member: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {payload}"
    );
    assert_eq!(
        error_code(payload),
        "ValidationException",
        "case {case:?}: expected the modelled input refusal, got {payload}"
    );
    let message = error_message(payload);
    assert!(
        message.contains(member),
        "case {case:?}: expected a clause naming {member:?}, got {message:?}"
    );
}

/// Assert the call got PAST the authorization gate: `StartAudienceGenerationJob` and
/// `StartTrainedModelInferenceJob` are still scaffold stubs with their own issues, so a request
/// whose reference resolves and whose members are within their constraints reaches the
/// operation itself and is answered the scaffold's 501 — never the 403, which is the whole
/// point of the case.
fn assert_reached_the_operation(case: &str, status: u16, payload: &Value) {
    assert_eq!(
        status, 501,
        "case {case:?}: expected the call to reach the operation itself, got {status} {payload}"
    );
    assert_eq!(
        error_code(payload),
        "NotImplemented",
        "case {case:?}: expected the scaffold's own answer, got {payload}"
    );
}

/// A `ConfiguredAudienceModelArn` in `region` held by `account`, matching that shape's
/// `^arn:aws[-a-z]*:cleanrooms-ml:[-a-z0-9]+:[0-9]{12}:configured-audience-model/[-a-zA-Z0-9_/.]+$`
/// pattern. Spelled out rather than imported, so a wrong reading of the reference in the
/// service fails this suite.
fn configured_audience_model_arn(partition: &str, region: &str, account: &str, id: &str) -> String {
    format!("arn:{partition}:cleanrooms-ml:{region}:{account}:configured-audience-model/{id}")
}

/// The same ARN in the emulator's own account and partition, which every row that varies
/// something else uses.
fn audience_model_reference(id: &str) -> String {
    configured_audience_model_arn("aws", REGION, ACCOUNT, id)
}

/// An `MLInputChannelArn` under `membership`, matching that shape's own pattern — the data
/// source an inference job reads, which every row below keeps well formed so the only thing a
/// row varies is the reference under test.
fn ml_input_channel_arn(membership: &str, id: &str, region: &str) -> String {
    format!(
        "arn:aws:cleanrooms-ml:{region}:{ACCOUNT}:membership/{membership}/ml-input-channel/{id}"
    )
}

/// A `UUID` (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`) built from
/// `index`, so a case can name as many distinct memberships as it needs.
fn uuid(index: u32) -> String {
    format!("{index:08x}-2222-4333-8444-{index:012x}")
}

/// An instant that is the same on every run, so a seeded record carries no clock reading.
fn instant(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

/// Write a trained model trained through `association_arn` into the store the service reads,
/// and answer the ARN it is keyed by.
///
/// `CreateTrainedModel` is still a scaffold stub with its own issue, so the record a case
/// resolves against is written into the SAME store the service reads through its own state
/// module, reached from the one shared engine the router was built over.
fn seed_trained_model(
    engine: &engine::InMemoryEngine,
    membership: &str,
    collaboration: &str,
    id: &str,
    region: &str,
    association_arn: &str,
) -> String {
    let arn = trained_model_arn(membership, id, region);
    engine
        .state::<ComAmazonawsCleanroomsmlAwsstarkcontrolserviceState>()
        .lock()
        .trained_models
        .insert(
            arn.clone(),
            TrainedModel {
                arn: arn.clone(),
                region: region.to_string(),
                membership_identifier: membership.to_string(),
                collaboration_identifier: collaboration.to_string(),
                name: format!("model-{id}"),
                version_identifier: None,
                incremental_training_data_channels: None,
                configured_model_algorithm_association_arn: association_arn.to_string(),
                description: None,
                creator_account_id: ACCOUNT.to_string(),
                ml_model_training_payer_account_id: None,
                tags: Default::default(),
                status: "ACTIVE".to_string(),
                pending: None,
                create_time: instant(1_760_000_000),
                update_time: instant(1_760_000_060),
            },
        );
    arn
}

/// A collaboration and a membership in it, both created through CLEAN ROOMS in `region`.
/// Answers `(collaboration identifier, membership identifier)`.
async fn collaboration_with_membership(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
) -> (String, String) {
    let (status, created) = call(
        router,
        "cleanrooms",
        region,
        "POST",
        "/collaborations",
        Some(json!({
            "name": name,
            "creatorDisplayName": "creator",
            "creatorMemberAbilities": ["CAN_QUERY"],
            "members": [],
            "queryLogStatus": "ENABLED",
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateCollaboration {name:?}: expected the collaboration to be persisted, got \
         {status} {created}"
    );
    let collaboration = created["collaboration"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (status, joined) = call(
        router,
        "cleanrooms",
        region,
        "POST",
        "/memberships",
        Some(json!({
            "collaborationIdentifier": collaboration,
            "queryLogStatus": "ENABLED",
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateMembership in {collaboration}: expected the membership to be persisted, got \
         {status} {joined}"
    );
    let membership = joined["membership"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    (collaboration, membership)
}

/// A configured model algorithm association created through Clean Rooms ML's OWN creates under
/// `membership`, so the association the gate resolves is one that service really holds.
/// Answers its ARN.
async fn create_association(
    router: &MultiServiceRouter,
    region: &str,
    membership: &str,
    name: &str,
) -> String {
    let (status, algorithm) = call(
        router,
        "cleanrooms-ml",
        region,
        "POST",
        "/configured-model-algorithms",
        Some(json!({ "name": name, "roleArn": ROLE_ARN })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateConfiguredModelAlgorithm {name:?}: expected it to be persisted, got {status} \
         {algorithm}"
    );
    let algorithm_arn = algorithm["configuredModelAlgorithmArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (status, association) = call(
        router,
        "cleanrooms-ml",
        region,
        "POST",
        &format!("/memberships/{membership}/configured-model-algorithm-associations"),
        Some(json!({ "name": name, "configuredModelAlgorithmArn": algorithm_arn })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateConfiguredModelAlgorithmAssociation {name:?}: expected it to be persisted, got \
         {status} {association}"
    );
    association["configuredModelAlgorithmAssociationArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A `StartAudienceGenerationJob` request for `name` generating from `arn`, with `extra`
/// merged over it so a row can stand a constraint-violating member beside the reference.
fn audience_job_body(name: &str, arn: &str, extra: Value) -> Value {
    let mut body = json!({
        "name": name,
        "configuredAudienceModelArn": arn,
        "seedAudience": {
            "roleArn": ROLE_ARN,
            "dataSource": { "s3Uri": "s3://seed-audiences/seed.jsonl" },
        },
    });
    if let (Some(target), Some(fields)) = (body.as_object_mut(), extra.as_object()) {
        for (member, value) in fields {
            target.insert(member.clone(), value.clone());
        }
    }
    body
}

/// A `StartTrainedModelInferenceJob` request for `name` running `trained_model_arn` on
/// `instance_count` instances, with `extra` merged over it for the same reason.
fn inference_job_body(
    name: &str,
    trained_model_arn: &str,
    membership: &str,
    instance_count: i64,
    extra: Value,
) -> Value {
    let mut body = json!({
        "name": name,
        "trainedModelArn": trained_model_arn,
        "resourceConfig": { "instanceType": INSTANCE_TYPE, "instanceCount": instance_count },
        "outputConfiguration": { "members": [{ "accountId": ACCOUNT }] },
        "dataSource": { "mlInputChannelArn": ml_input_channel_arn(membership, "channel", REGION) },
    });
    if let (Some(target), Some(fields)) = (body.as_object_mut(), extra.as_object()) {
        for (member, value) in fields {
            target.insert(member.clone(), value.clone());
        }
    }
    body
}

/// The refusal `StartAudienceGenerationJob` answers for a reference it resolves to nothing,
/// spelled out here rather than imported so a changed template fails this suite.
fn audience_denial(arn: &str) -> String {
    format!(
        "Provided ConfiguredAudienceModelArn: {arn} is invalid. Please provide a valid arn. \
         because no resource-based policy allows the cleanrooms-ml:StartAudienceGenerationJob \
         action"
    )
}

/// The refusal `StartTrainedModelInferenceJob` answers for a reference it resolves to nothing,
/// spelled out for the same reason.
fn inference_denial(arn: &str) -> String {
    format!(
        "Configured Model Algorithm Association not found for Trained Model {arn}. because no \
         resource-based policy allows the cleanrooms-ml:StartTrainedModelInferenceJob action"
    )
}

/// A value that names no configured audience model is an AUTHORIZATION failure stating the
/// reference and the action — not the 400 the framework's constraint walk answers for the
/// member that carried it, nor for the members standing beside it. The rows span the ways a
/// value can name no such resource (another kind of Clean Rooms ML resource, another service's
/// resource, no ARN at all, an account that is not twelve digits, an empty resource id, a value
/// shorter than the modelled minimum) and the constraint-violating member each one carries.
#[tokio::test]
async fn an_audience_generation_job_naming_no_configured_audience_model_is_denied() {
    let (router, _engine) = emulator();

    // (case, the reference, the constraint-violating members standing beside it)
    let rows: Vec<(&str, String, Value)> = vec![
        (
            "an ARN naming another kind of Clean Rooms ML resource",
            format!("arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:audience-model/absent-model"),
            json!({}),
        ),
        (
            "an ARN naming a training dataset instead",
            format!("arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:training-dataset/some-dataset"),
            json!({}),
        ),
        (
            "an ARN of another service's resource",
            format!("arn:aws:cleanrooms:{REGION}:{ACCOUNT}:configured-audience-model/elsewhere"),
            json!({}),
        ),
        (
            "a resource path that is no ARN at all",
            "configured-audience-model/not-an-arn".to_string(),
            json!({}),
        ),
        (
            "an ARN whose account field is not twelve digits",
            format!("arn:aws:cleanrooms-ml:{REGION}:12345:configured-audience-model/short"),
            json!({}),
        ),
        (
            "an ARN with an empty resource id",
            format!("arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:configured-audience-model/"),
            json!({}),
        ),
        (
            "a value shorter than ConfiguredAudienceModelArn's 20-character minimum",
            "arn:aws:cleanrooms".to_string(),
            json!({}),
        ),
        (
            "an ARN naming another kind of resource, beside a name past NameString's maximum",
            format!("arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:audience-model/absent-model"),
            json!({ "name": "n".repeat(MAX_NAME + 1) }),
        ),
        (
            "the same, beside a description past ResourceDescription's maximum",
            format!("arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:audience-model/absent-model"),
            json!({ "description": "d".repeat(MAX_DESCRIPTION + 1) }),
        ),
        (
            "the same, beside an absent required member",
            format!("arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:audience-model/absent-model"),
            json!({ "seedAudience": Value::Null }),
        ),
        (
            "the same, beside a collaboration id outside UUID's pattern",
            format!("arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:audience-model/absent-model"),
            json!({ "collaborationId": "not-a-uuid" }),
        ),
    ];

    for (index, (case, arn, extra)) in rows.into_iter().enumerate() {
        let (status, payload) = start_audience_job(
            &router,
            REGION,
            audience_job_body(&format!("audience-job-{index}"), &arn, extra),
        )
        .await;
        assert_access_denied(case, status, &payload, &audience_denial(&arn));
    }
}

/// A value that DOES name a configured audience model leaves the request to the modelled walk:
/// the authorization decision moved ahead of validation, it did not replace it. The rows span
/// the partition, region, account and resource id `ConfiguredAudienceModelArn`'s pattern
/// admits, and then the members a request can violate a constraint on.
#[tokio::test]
async fn an_audience_generation_job_naming_a_configured_audience_model_is_validated() {
    let (router, _engine) = emulator();

    // Every reference the pattern admits resolves, so the call reaches the operation itself —
    // the partition, the region and the account are fields of the reference, not of the
    // endpoint, and the resource id is `[-a-zA-Z0-9_/.]+`.
    let references: Vec<(&str, String)> = vec![
        (
            "the emulator's own partition, region and account",
            audience_model_reference("seed-audience-model"),
        ),
        (
            "another partition",
            configured_audience_model_arn("aws-us-gov", "us-gov-west-1", ACCOUNT, "gov-model"),
        ),
        (
            "another region",
            configured_audience_model_arn("aws", OTHER_REGION, ACCOUNT, "other-region-model"),
        ),
        (
            "another account, as a shared model carries",
            configured_audience_model_arn("aws", REGION, "210987654321", "shared-model"),
        ),
        (
            "a resource id carrying the punctuation the pattern admits",
            audience_model_reference("team_a/model.v2-final"),
        ),
    ];

    for (case, arn) in references {
        let (status, payload) = start_audience_job(
            &router,
            REGION,
            audience_job_body("audience-job-resolved", &arn, json!({})),
        )
        .await;
        assert_reached_the_operation(case, status, &payload);
    }

    // (case, the constraint-violating members, the member the 400 must name)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a name past NameString's maximum",
            json!({ "name": "n".repeat(MAX_NAME + 1) }),
            "name",
        ),
        (
            "a description past ResourceDescription's maximum",
            json!({ "description": "d".repeat(MAX_DESCRIPTION + 1) }),
            "description",
        ),
        (
            "a collaboration id outside UUID's pattern",
            json!({ "collaborationId": "not-a-uuid" }),
            "collaborationId",
        ),
        (
            "an absent required member",
            json!({ "seedAudience": Value::Null }),
            "seedAudience",
        ),
    ];

    let resolved = audience_model_reference("validated-model");
    for (index, (case, extra, member)) in rows.into_iter().enumerate() {
        let (status, payload) = start_audience_job(
            &router,
            REGION,
            audience_job_body(&format!("resolved-job-{index}"), &resolved, extra),
        )
        .await;
        assert_validation_error(case, status, &payload, member);
    }
}

/// A trained model whose configured model algorithm association this endpoint cannot resolve is
/// an AUTHORIZATION failure naming the model and the action. The rows span whether the model is
/// held at all, whether its association is, which region holds them, which association the
/// request names for the model, and the constraint-violating member standing beside it — the
/// `instanceCount` past its range among them.
#[tokio::test]
async fn a_trained_model_inference_job_whose_association_is_unresolvable_is_denied() {
    let (router, engine) = emulator();
    let membership = uuid(1);

    // A model held here, trained through an association NOTHING created: the model resolves,
    // the association behind it does not.
    let without_association = seed_trained_model(
        &engine,
        &membership,
        &uuid(9),
        "no-association",
        REGION,
        &format!(
            "arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:membership/{membership}\
             /configured-model-algorithm-association/never-created"
        ),
    );

    // A model, and its association, held by the OTHER endpoint only.
    let (collaboration_elsewhere, membership_elsewhere) =
        collaboration_with_membership(&router, OTHER_REGION, "inference-elsewhere").await;
    let association_elsewhere =
        create_association(&router, OTHER_REGION, &membership_elsewhere, "elsewhere").await;
    let model_elsewhere = seed_trained_model(
        &engine,
        &membership_elsewhere,
        &collaboration_elsewhere,
        "held-elsewhere",
        OTHER_REGION,
        &association_elsewhere,
    );

    // A model held here whose association is held here too — the row that varies only the
    // association the REQUEST names.
    let (collaboration, membership_here) =
        collaboration_with_membership(&router, REGION, "inference-here").await;
    let association_here = create_association(&router, REGION, &membership_here, "here").await;
    let model_here = seed_trained_model(
        &engine,
        &membership_here,
        &collaboration,
        "held-here",
        REGION,
        &association_here,
    );

    // (case, the membership in the path, the reference, the members standing beside it)
    let rows: Vec<(&str, String, String, Value)> = vec![
        (
            "a trained model nothing was created under, beside an instanceCount past its range",
            membership.clone(),
            trained_model_arn(&membership, "never-created", REGION),
            json!({}),
        ),
        (
            "a trained model whose association nothing created",
            membership.clone(),
            without_association.clone(),
            json!({}),
        ),
        (
            "a trained model and association held by another region's endpoint",
            membership_elsewhere.clone(),
            model_elsewhere.clone(),
            json!({}),
        ),
        (
            "a held model the request runs through an association nothing created",
            membership_here.clone(),
            model_here.clone(),
            json!({
                "configuredModelAlgorithmAssociationArn": format!(
                    "arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:membership/{membership_here}\
                     /configured-model-algorithm-association/never-created"
                ),
            }),
        ),
        (
            "an unheld model beside a name past NameString's maximum",
            membership.clone(),
            trained_model_arn(&membership, "never-created", REGION),
            json!({ "name": "n".repeat(MAX_NAME + 1) }),
        ),
        (
            "an unheld model beside an absent required member",
            membership.clone(),
            trained_model_arn(&membership, "never-created", REGION),
            json!({ "dataSource": Value::Null }),
        ),
    ];

    for (index, (case, path_membership, arn, extra)) in rows.into_iter().enumerate() {
        let (status, payload) = start_inference_job(
            &router,
            REGION,
            &path_membership,
            inference_job_body(
                &format!("inference-job-{index}"),
                &arn,
                &path_membership,
                MAX_INSTANCE_COUNT + 1,
                extra,
            ),
        )
        .await;
        assert_access_denied(case, status, &payload, &inference_denial(&arn));
    }

    // The control: the very model the third row was refused at IS resolved by the endpoint that
    // holds it and its association.
    let (status, payload) = start_inference_job(
        &router,
        OTHER_REGION,
        &membership_elsewhere,
        inference_job_body(
            "inference-job-elsewhere",
            &model_elsewhere,
            &membership_elsewhere,
            1,
            json!({}),
        ),
    )
    .await;
    assert_reached_the_operation(
        "a model and association held by the endpoint the call reached",
        status,
        &payload,
    );
}

/// A trained model whose association this endpoint resolves leaves the request to the modelled
/// walk, `instanceCount` included. The rows span the association the request names for the
/// model and the member it violates a constraint on.
#[tokio::test]
async fn a_trained_model_inference_job_whose_association_resolves_is_validated() {
    let (router, engine) = emulator();
    let (collaboration, membership) =
        collaboration_with_membership(&router, REGION, "inference-validated").await;
    let association = create_association(&router, REGION, &membership, "validated").await;
    let model = seed_trained_model(
        &engine,
        &membership,
        &collaboration,
        "validated",
        REGION,
        &association,
    );

    // (case, the members standing beside the reference, the member the 400 must name)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "an instanceCount one past InferenceResourceConfig's maximum",
            json!({
                "resourceConfig": {
                    "instanceType": INSTANCE_TYPE,
                    "instanceCount": MAX_INSTANCE_COUNT + 1,
                },
            }),
            "instanceCount",
        ),
        (
            "an instanceCount below InferenceResourceConfig's minimum",
            json!({ "resourceConfig": { "instanceType": INSTANCE_TYPE, "instanceCount": 0 } }),
            "instanceCount",
        ),
        (
            "a name past NameString's maximum",
            json!({ "name": "n".repeat(MAX_NAME + 1) }),
            "name",
        ),
        (
            "an absent required member",
            json!({ "outputConfiguration": Value::Null }),
            "outputConfiguration",
        ),
    ];

    for (index, (case, extra, member)) in rows.into_iter().enumerate() {
        let (status, payload) = start_inference_job(
            &router,
            REGION,
            &membership,
            inference_job_body(
                &format!("validated-job-{index}"),
                &model,
                &membership,
                1,
                extra,
            ),
        )
        .await;
        assert_validation_error(case, status, &payload, member);
    }

    // A request within every constraint reaches the operation itself, whether it names the
    // association the model was trained through or leaves it to the model's own record.
    for (case, extra) in [
        ("the association left to the model's record", json!({})),
        (
            "the association the request names itself",
            json!({ "configuredModelAlgorithmAssociationArn": association.clone() }),
        ),
    ] {
        let (status, payload) = start_inference_job(
            &router,
            REGION,
            &membership,
            inference_job_body("validated-job-valid", &model, &membership, 1, extra),
        )
        .await;
        assert_reached_the_operation(case, status, &payload);
    }

    // Every instanceCount InferenceResourceConfig admits is within the range, so none of them
    // is refused: the bound is the modelled one and not a value one case happened to use.
    for count in [1, MAX_INSTANCE_COUNT / 2, MAX_INSTANCE_COUNT] {
        let (status, payload) = start_inference_job(
            &router,
            REGION,
            &membership,
            inference_job_body(
                &format!("validated-count-{count}"),
                &model,
                &membership,
                count,
                json!({}),
            ),
        )
        .await;
        assert_reached_the_operation(&format!("instanceCount {count}"), status, &payload);
    }
}
