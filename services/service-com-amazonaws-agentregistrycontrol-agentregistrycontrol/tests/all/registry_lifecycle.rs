// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the registry, registry-record and publisher slices of
//! `agent-registry-control`, driven over HTTP through the real router so every
//! assertion goes through routing, deserialization, the handler and serialization.
//!
//! The rule under test: each of these operations computes its answer from the state the
//! create operations persisted — a create stores the whole request and mints the
//! identifiers the model's patterns describe, a read reports exactly what is stored, a
//! delete removes it, and the asynchronous statuses the model documents (`CREATING` ->
//! `READY`, `CREATING` -> `DRAFT`, `DELETING` -> gone, a publisher's `ACTION_REQUIRED` ->
//! `ACTIVE`) are owed by the clock rather than by the caller's next call. Every test
//! therefore asserts over a TABLE of inputs, so it holds for a class of requests and not
//! for one recorded shape.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
/// The digest trait the skill-package checksum below is computed through.
use sha2::Digest;
use smithy_server_runtime::MultiServiceRouter;

/// The region in the test's SigV4 credential scope; every minted ARN must carry it.
const REGION: &str = "us-west-2";

/// A signed-looking request, so the router resolves this service's signing name
/// (`agent-registry`) and the handlers read the region from the credential scope.
/// The signature itself is never checked: the test router runs with authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTAGENTREGISTRY/20260101/us-west-2/agent-registry/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// Longer than the service's own asynchronous-transition span, so a transition the clock
/// owes has certainly come due by the time the next call is made.
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// Wait out a transition the clock owes, without blocking the test runtime.
async fn wait_past_the_transition() {
    tokio::time::sleep(PAST_THE_TRANSITION).await;
}

/// One HTTP answer, reduced to what these tests assert on.
struct Reply {
    status: u16,
    body: Value,
    /// The wire error code (`x-amzn-errortype`), present only on a failure.
    error_code: Option<String>,
}

impl Reply {
    fn field(&self, name: &str) -> &Value {
        &self.body[name]
    }

    fn str_field(&self, name: &str) -> &str {
        self.body[name].as_str().unwrap_or_default()
    }
}

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_agentregistrycontrol_agentregistrycontrol::EmulatedService::new(
            engine,
        ),
    );
    router
}

async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Option<Value>) -> Reply {
    let payload = match &body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serializable body")),
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://agent-registry-control.{REGION}.amazonaws.com{path}"
        ))
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(payload)
        .expect("a valid request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.rsplit('#').next().unwrap_or(value).to_string());
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Reply {
        status,
        body,
        error_code,
    }
}

/// Percent-encode a path segment the way an SDK encodes a non-greedy `@httpLabel`, so an
/// ARN (which carries `:` and `/`) can be given where the model allows the ARN form.
/// Every byte that is not ASCII-alphanumeric becomes `%XX`, which is what an SDK's
/// strictest label encoding does and is always a valid encoding of the byte.
fn encode_label(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn id_of(arn: &str) -> &str {
    arn.rsplit('/').next().unwrap_or(arn)
}

fn is_alnum_within(value: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&value.len()) && value.chars().all(|c| c.is_ascii_alphanumeric())
}

/// A sortable key for a modelled timestamp, whichever wire form the protocol renders it
/// in: epoch seconds (zero-padded so the keys sort numerically) or an RFC-3339 string
/// (which sorts lexicographically). Empty for anything that is not a timestamp at all.
fn timestamp_key(value: &Value) -> String {
    match value {
        Value::Number(number) => format!("{:020.3}", number.as_f64().unwrap_or_default()),
        Value::String(text) => text.clone(),
        _ => String::new(),
    }
}

/// `^arn:aws(-[^:]+)?:agent-registry:[a-z0-9-]+:[0-9]{12}:registry/[a-zA-Z0-9]{12,16}$`
fn registry_arn_is_well_formed(arn: &str) -> bool {
    let prefix = format!(
        "arn:aws:agent-registry:{REGION}:{}:registry/",
        engine::emulator_account_id()
    );
    arn.strip_prefix(&prefix)
        .is_some_and(|id| is_alnum_within(id, 12, 16))
}

/// `^…:registry/[a-zA-Z0-9]{12,16}/record/[a-zA-Z0-9]{12}$`
fn record_arn_is_well_formed(arn: &str, registry_arn: &str) -> bool {
    arn.strip_prefix(&format!("{registry_arn}/record/"))
        .is_some_and(|id| is_alnum_within(id, 12, 12))
}

async fn create_registry(router: &MultiServiceRouter, body: Value) -> Reply {
    call(router, "POST", "/registries", Some(body)).await
}

async fn get_registry(router: &MultiServiceRouter, identifier: &str) -> Reply {
    call(
        router,
        "GET",
        &format!("/registries/{}", encode_label(identifier)),
        None,
    )
    .await
}

async fn create_record(router: &MultiServiceRouter, registry_id: &str, body: Value) -> Reply {
    call(
        router,
        "POST",
        &format!("/registries/{}/records", encode_label(registry_id)),
        Some(body),
    )
    .await
}

async fn get_record(router: &MultiServiceRouter, registry_id: &str, record_id: &str) -> Reply {
    call(
        router,
        "GET",
        &format!(
            "/registries/{}/records/{}",
            encode_label(registry_id),
            encode_label(record_id)
        ),
        None,
    )
    .await
}

async fn delete_record(router: &MultiServiceRouter, registry_id: &str, record_id: &str) -> Reply {
    call(
        router,
        "DELETE",
        &format!(
            "/registries/{}/records/{}",
            encode_label(registry_id),
            encode_label(record_id)
        ),
        None,
    )
    .await
}

async fn list_records(router: &MultiServiceRouter, registry_id: &str, body: Value) -> Reply {
    call(
        router,
        "POST",
        &format!("/registries/{}/records-list", encode_label(registry_id)),
        Some(body),
    )
    .await
}

/// Create a registry and wait out the creation the clock owes, so it is `READY`.
/// Returns its registry id.
async fn ready_registry(router: &MultiServiceRouter, name: &str) -> String {
    let created = create_registry(router, json!({ "name": name })).await;
    assert_eq!(
        created.status, 202,
        "case {name}: CreateRegistry must answer 202, got {} {:?}",
        created.status, created.body
    );
    let registry_id = id_of(created.str_field("registryArn")).to_string();
    wait_past_the_transition().await;
    let got = get_registry(router, &registry_id).await;
    assert_eq!(
        got.str_field("status"),
        "READY",
        "case {name}: a created registry becomes READY, got {:?}",
        got.body
    );
    registry_id
}

/// An MCP server definition a supported MCP server schema version accepts: the MCP server
/// descriptor's content is validated against that schema, so a record of this type carries a
/// named server with a description and a version.
const MCP_SERVER_DEFINITION: &str =
    r#"{"name":"com.example/checkout","description":"Checkout tools","version":"1.4.2"}"#;

/// A `descriptors` value per `RecordType`, so each row of the record table carries a
/// descriptor of its own shape. Member names are `Descriptors`' own
/// (`smithy_query(query="shape", shape="Descriptors")`).
fn descriptors_for(record_type: &str) -> Value {
    match record_type {
        "MCP" => json!({ "mcpServer": { "data": MCP_SERVER_DEFINITION, "dataSchemaVersion": "2025-06-18" } }),
        "AGENT" => json!({ "a2aAgentCard": { "data": "{\"name\":\"checkout\"}", "dataSchemaVersion": "0.2.1" } }),
        "SKILL" => json!({ "agentSkillsDefinition": { "data": "{\"skills\":[]}" } }),
        "GATEWAY" => json!({ "http": { "source": { "fromUrl": { "url": "https://example.test/gateway" } } } }),
        _ => json!({ "custom": { "data": "{\"anything\":true}" } }),
    }
}

#[tokio::test]
async fn create_registry_persists_every_member_and_get_registry_reports_it() {
    // Rows differ along WHICH optional members the request carries and along the size of
    // `name` (the model's 1..64 bound), so the echo cannot be a fixed shape.
    let cases: Vec<(&str, Value)> = vec![
        ("only the required name", json!({ "name": "a" })),
        (
            "description and tags",
            json!({
                "name": "demo-registry",
                "description": "demo",
                "tags": { "team": "agents", "env": "test" }
            }),
        ),
        (
            "encryption and discovery configuration",
            json!({
                "name": "encrypted.registry/one",
                "encryptionConfiguration": { "kmsKeyArn": "arn:aws:kms:us-west-2:000000000000:key/12345678-1234-1234-1234-123456789012" },
                "discoveryConfiguration": { "authorizerType": "AWS_IAM" }
            }),
        ),
        (
            "approval, metadata schema and auto-detection configuration",
            json!({
                "name": "curated_registry-2",
                "approvalConfiguration": { "autoApprovalRules": ["APPROVE_ALL"] },
                "customMetadataSchemaConfiguration": { "defaultSchema": "{\"type\":\"object\"}" },
                "autoDetectionConfiguration": { "scope": "ORGANIZATION", "enabled": true }
            }),
        ),
        (
            "a name at the model's 64-character bound",
            json!({ "name": "r".repeat(64) }),
        ),
    ];

    let router = router();
    for (case, body) in cases {
        let created = create_registry(&router, body.clone()).await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: CreateRegistry answers 202, got {} {:?}",
            created.status, created.body
        );
        let registry_arn = created.str_field("registryArn").to_string();
        assert!(
            registry_arn_is_well_formed(&registry_arn),
            "case {case:?}: registryArn must be arn:aws:agent-registry:<region>:<account>:registry/<12-16 alnum>, got {registry_arn:?}"
        );
        let registry_id = id_of(&registry_arn).to_string();

        // Creation is asynchronous, so a caller reading immediately still sees CREATING.
        let creating = get_registry(&router, &registry_id).await;
        assert_eq!(
            creating.status, 200,
            "case {case:?}: GetRegistry answers 200, got {} {:?}",
            creating.status, creating.body
        );
        assert_eq!(
            creating.str_field("status"),
            "CREATING",
            "case {case:?}: a registry begins CREATING, got {:?}",
            creating.body
        );

        wait_past_the_transition().await;
        let got = get_registry(&router, &registry_id).await;
        assert_eq!(
            got.str_field("status"),
            "READY",
            "case {case:?}: the clock owes CREATING -> READY, got {:?}",
            got.body
        );
        assert_eq!(
            got.str_field("registryId"),
            registry_id,
            "case {case:?}: GetRegistry reports the minted id, got {:?}",
            got.body
        );
        assert_eq!(
            got.str_field("registryArn"),
            registry_arn,
            "case {case:?}: GetRegistry reports the minted ARN, got {:?}",
            got.body
        );

        // Every member the request carried is reported back from state, and one it did
        // not carry is absent rather than defaulted. `discoveryConfiguration` and
        // `approvalConfiguration` are not in this set: a registry carries one of each
        // whether or not the create named it, so they are asserted below as the EFFECTIVE
        // configuration the registry ends up with.
        for member in [
            "name",
            "description",
            "encryptionConfiguration",
            "customMetadataSchemaConfiguration",
        ] {
            match body.get(member) {
                Some(sent) => assert_eq!(
                    got.field(member),
                    sent,
                    "case {case:?}: {member} must be what CreateRegistry stored, got {:?}",
                    got.field(member)
                ),
                None => assert!(
                    got.field(member).is_null(),
                    "case {case:?}: {member} was never sent, so nothing may be reported, got {:?}",
                    got.field(member)
                ),
            }
        }

        // The registry's effective configuration: what the request asked for where it
        // asked, and the service's own default where it did not — an IAM-authorized
        // discovery configuration, and an approval workflow with no auto-approval rule in
        // it, which is the one that sends every submission to manual review.
        assert_eq!(
            got.field("discoveryConfiguration"),
            &body
                .get("discoveryConfiguration")
                .cloned()
                .unwrap_or_else(|| json!({ "authorizerType": "AWS_IAM" })),
            "case {case:?}: GetRegistry reports the effective discoveryConfiguration, got {:?}",
            got.field("discoveryConfiguration")
        );
        assert_eq!(
            got.field("approvalConfiguration"),
            &body
                .get("approvalConfiguration")
                .cloned()
                .unwrap_or_else(|| json!({ "autoApprovalRules": [] })),
            "case {case:?}: GetRegistry reports the effective approvalConfiguration, got {:?}",
            got.field("approvalConfiguration")
        );

        match body.get("autoDetectionConfiguration") {
            Some(sent) => {
                assert_eq!(
                    &got.field("autoDetection")["configuration"],
                    sent,
                    "case {case:?}: autoDetection reports the configuration asked for, got {:?}",
                    got.field("autoDetection")
                );
                assert_eq!(
                    got.field("autoDetection")["status"],
                    json!("ACTIVE"),
                    "case {case:?}: an enabled auto-detection is ACTIVE, got {:?}",
                    got.field("autoDetection")
                );
            }
            None => assert!(
                got.field("autoDetection").is_null(),
                "case {case:?}: no auto-detection was asked for, got {:?}",
                got.field("autoDetection")
            ),
        }

        let created_at = timestamp_key(got.field("createdAt"));
        let updated_at = timestamp_key(got.field("updatedAt"));
        assert!(
            !created_at.is_empty() && !updated_at.is_empty(),
            "case {case:?}: both timestamps are reported, got createdAt={:?} updatedAt={:?}",
            got.field("createdAt"),
            got.field("updatedAt")
        );
        assert!(
            updated_at >= created_at,
            "case {case:?}: the READY transition is not before creation, got createdAt={created_at:?} updatedAt={updated_at:?}"
        );

        // The model lets the label be the registry's full ARN as well as its bare id.
        let by_arn = get_registry(&router, &registry_arn).await;
        assert_eq!(
            by_arn.status, 200,
            "case {case:?}: GetRegistry accepts the ARN form, got {} {:?}",
            by_arn.status, by_arn.body
        );
        assert_eq!(
            by_arn.str_field("registryId"),
            registry_id,
            "case {case:?}: the ARN form names the same registry, got {:?}",
            by_arn.body
        );
    }
}

#[tokio::test]
async fn an_unknown_registry_or_record_is_a_resource_not_found() {
    let router = router();
    let registry_id = ready_registry(&router, "lookup-registry").await;
    let registry_arn = get_registry(&router, &registry_id)
        .await
        .str_field("registryArn")
        .to_string();

    // Rows: a bare id, the ARN form, and a record under a registry that does exist.
    let absent_registry = "zzzz999999zz";
    let cases: Vec<(&str, Reply)> = vec![
        (
            "bare registry id",
            get_registry(&router, absent_registry).await,
        ),
        (
            "registry ARN form",
            get_registry(
                &router,
                &format!(
                    "arn:aws:agent-registry:{REGION}:{}:registry/{absent_registry}",
                    engine::emulator_account_id()
                ),
            )
            .await,
        ),
        (
            "record of an existing registry",
            get_record(&router, &registry_id, "aaaa11112222").await,
        ),
        (
            "record of an absent registry",
            get_record(&router, absent_registry, "aaaa11112222").await,
        ),
        (
            "record named by its ARN form",
            get_record(
                &router,
                &registry_id,
                &format!("{registry_arn}/record/aaaa11112222"),
            )
            .await,
        ),
        (
            "deleting a record that is not there",
            delete_record(&router, &registry_id, "aaaa11112222").await,
        ),
        (
            "listing records of an absent registry",
            list_records(&router, absent_registry, json!({})).await,
        ),
    ];

    for (case, reply) in cases {
        assert_eq!(
            reply.status, 404,
            "case {case:?}: an unknown resource is 404, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: the modelled error code is ResourceNotFoundException, got {:?}",
            reply.error_code
        );
    }
}

#[tokio::test]
async fn a_duplicate_registry_name_conflicts_and_a_replayed_client_token_creates_nothing() {
    // Rows differ along the name's shape and the token's length (the model's 33..256).
    let cases = [
        ("short name", "x", "aaaaaaaabbbbccccddddeeeeffff11112222"),
        (
            "dotted name",
            "orders.registry",
            "tokenfortheordersregistrycreate00001",
        ),
        (
            "underscored name",
            "fleet-registry_01",
            "tokenforthefleetregistrycreate000002",
        ),
    ];

    let router = router();
    for (case, name, token) in cases {
        let first = create_registry(&router, json!({ "name": name, "clientToken": token })).await;
        assert_eq!(
            first.status, 202,
            "case {case:?}: the first create answers 202, got {} {:?}",
            first.status, first.body
        );
        let arn = first.str_field("registryArn").to_string();

        // Same token, same request: the service ignores the request and answers the ARN
        // the first call minted — no second registry.
        let replay = create_registry(&router, json!({ "name": name, "clientToken": token })).await;
        assert_eq!(
            replay.status, 202,
            "case {case:?}: a replayed token is not an error, got {} {:?}",
            replay.status, replay.body
        );
        assert_eq!(
            replay.str_field("registryArn"),
            arn,
            "case {case:?}: a replayed token answers the same ARN, got {:?}",
            replay.body
        );

        // A different token with a name a live registry holds is a conflict.
        let duplicate = create_registry(
            &router,
            json!({ "name": name, "clientToken": format!("other{token}") }),
        )
        .await;
        assert_eq!(
            duplicate.status, 409,
            "case {case:?}: a duplicate name is 409, got {} {:?}",
            duplicate.status, duplicate.body
        );
        assert_eq!(
            duplicate.error_code.as_deref(),
            Some("ConflictException"),
            "case {case:?}: the modelled error code is ConflictException, got {:?}",
            duplicate.error_code
        );

        // And the registry the first call created is still the one in state.
        let got = get_registry(&router, id_of(&arn)).await;
        assert_eq!(
            got.str_field("name"),
            name,
            "case {case:?}: the stored registry is unchanged, got {:?}",
            got.body
        );
    }
}

#[tokio::test]
async fn create_registry_record_persists_every_member_and_get_registry_record_reports_it() {
    let router = router();
    let registry_id = ready_registry(&router, "record-registry").await;
    let registry_arn = get_registry(&router, &registry_id)
        .await
        .str_field("registryArn")
        .to_string();

    // One row per `RecordType` the model declares (source: the RecordType enum,
    // `smithy_query(query="shape", shape="RecordType")`: AGENT | CUSTOM | GATEWAY | MCP |
    // SKILL), each with a descriptor of its own shape and a different set of optional
    // members — including the row that omits `recordVersion`.
    let cases: Vec<(&str, Value)> = vec![
        (
            "MCP with a version and metadata",
            json!({
                "name": "search-mcp",
                "recordType": "MCP",
                "descriptors": descriptors_for("MCP"),
                "displayName": "Search MCP",
                "description": "an MCP server",
                "recordVersion": "1.2.3",
                "customMetadata": { "owner": "search", "critical": true },
                "tags": { "env": "test" }
            }),
        ),
        (
            "AGENT without a version",
            json!({
                "name": "checkout-agent",
                "recordType": "AGENT",
                "descriptors": descriptors_for("AGENT")
            }),
        ),
        (
            "SKILL with provenance",
            json!({
                "name": "summarise.skill",
                "recordType": "SKILL",
                "descriptors": descriptors_for("SKILL"),
                "recordVersion": "2026-01-01",
                "provenance": [{
                    "relation": "DETECTED_FROM",
                    "sourceId": "arn:aws:bedrock-agentcore:us-west-2:000000000000:runtime/abc",
                    "sourceType": "AWS::BedrockAgentCore::Runtime"
                }]
            }),
        ),
        (
            "GATEWAY from a URL descriptor",
            json!({
                "name": "edge_gateway",
                "recordType": "GATEWAY",
                "descriptors": descriptors_for("GATEWAY"),
                "displayName": "Edge gateway"
            }),
        ),
        (
            "CUSTOM with the longest allowed display name",
            json!({
                "name": "custom-thing",
                "recordType": "CUSTOM",
                "descriptors": descriptors_for("CUSTOM"),
                "displayName": "d".repeat(255)
            }),
        ),
    ];

    for (case, body) in cases {
        let created = create_record(&router, &registry_id, body.clone()).await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: CreateRegistryRecord answers 202, got {} {:?}",
            created.status, created.body
        );
        let record_arn = created.str_field("recordArn").to_string();
        assert!(
            record_arn_is_well_formed(&record_arn, &registry_arn),
            "case {case:?}: recordArn must be <registryArn>/record/<12 alnum>, got {record_arn:?}"
        );
        assert_eq!(
            created.str_field("status"),
            "CREATING",
            "case {case:?}: a record is returned CREATING while it is processed, got {:?}",
            created.body
        );
        let record_id = id_of(&record_arn).to_string();

        let creating = get_record(&router, &registry_id, &record_id).await;
        assert_eq!(
            creating.str_field("status"),
            "CREATING",
            "case {case:?}: the transient status is visible to the next caller, got {:?}",
            creating.body
        );

        wait_past_the_transition().await;
        let got = get_record(&router, &registry_id, &record_id).await;
        assert_eq!(
            got.status, 200,
            "case {case:?}: GetRegistryRecord answers 200, got {} {:?}",
            got.status, got.body
        );
        assert_eq!(
            got.str_field("status"),
            "DRAFT",
            "case {case:?}: the clock owes CREATING -> DRAFT, got {:?}",
            got.body
        );
        assert_eq!(
            got.str_field("recordId"),
            record_id,
            "case {case:?}: the reported id is the minted one, got {:?}",
            got.body
        );
        assert_eq!(
            got.str_field("recordArn"),
            record_arn,
            "case {case:?}: the reported ARN is the minted one, got {:?}",
            got.body
        );
        assert_eq!(
            got.str_field("registryArn"),
            registry_arn,
            "case {case:?}: the record reports its parent registry, got {:?}",
            got.body
        );

        for member in [
            "name",
            "recordType",
            "descriptors",
            "displayName",
            "description",
            "customMetadata",
            "provenance",
        ] {
            match body.get(member) {
                Some(sent) => assert_eq!(
                    got.field(member),
                    sent,
                    "case {case:?}: {member} must be what CreateRegistryRecord stored, got {:?}",
                    got.field(member)
                ),
                None => assert!(
                    got.field(member).is_null(),
                    "case {case:?}: {member} was never sent, so nothing may be reported, got {:?}",
                    got.field(member)
                ),
            }
        }

        let expected_version = body
            .get("recordVersion")
            .and_then(Value::as_str)
            .unwrap_or("1");
        assert_eq!(
            got.str_field("recordVersion"),
            expected_version,
            "case {case:?}: recordVersion is the one stored, or the first version when none was given, got {:?}",
            got.body
        );
        assert_eq!(
            got.str_field("createdBy"),
            engine::emulator_account_id(),
            "case {case:?}: createdBy is the calling account, got {:?}",
            got.body
        );
        assert_eq!(
            got.field("createdByAutoDetection"),
            &json!(false),
            "case {case:?}: a record created through the API is not auto-detected, got {:?}",
            got.body
        );
    }
}

#[tokio::test]
async fn creating_a_record_needs_a_ready_registry() {
    let router = router();

    // A registry that is still being created cannot take records yet: it "becomes usable
    // once it reaches READY".
    let created = create_registry(&router, json!({ "name": "not-ready-yet" })).await;
    let registry_id = id_of(created.str_field("registryArn")).to_string();
    let too_early = create_record(
        &router,
        &registry_id,
        json!({
            "name": "too-early",
            "recordType": "CUSTOM",
            "descriptors": descriptors_for("CUSTOM")
        }),
    )
    .await;
    assert_eq!(
        too_early.status, 409,
        "case \"registry still CREATING\": a record cannot be created yet, got {} {:?}",
        too_early.status, too_early.body
    );
    assert_eq!(
        too_early.error_code.as_deref(),
        Some("ConflictException"),
        "case \"registry still CREATING\": the modelled error code is ConflictException, got {:?}",
        too_early.error_code
    );

    // Once the clock has made it READY the same request succeeds.
    wait_past_the_transition().await;
    let accepted = create_record(
        &router,
        &registry_id,
        json!({
            "name": "too-early",
            "recordType": "CUSTOM",
            "descriptors": descriptors_for("CUSTOM")
        }),
    )
    .await;
    assert_eq!(
        accepted.status, 202,
        "case \"registry READY\": the same request now succeeds, got {} {:?}",
        accepted.status, accepted.body
    );

    // A second record may not take a name a record of that registry already holds.
    let duplicate = create_record(
        &router,
        &registry_id,
        json!({
            "name": "too-early",
            "recordType": "MCP",
            "descriptors": descriptors_for("MCP")
        }),
    )
    .await;
    assert_eq!(
        duplicate.status, 409,
        "case \"duplicate record name\": record names are unique within a registry, got {} {:?}",
        duplicate.status, duplicate.body
    );
}

#[tokio::test]
async fn list_registry_records_pages_in_id_order_and_resumes_where_the_page_stopped() {
    let router = router();

    // Rows differ along how many records exist and how large a page is asked for,
    // including a page the same size as the set and one larger than it.
    let counts_and_pages = [(1usize, 1i64), (3, 1), (5, 2), (4, 4), (7, 3), (2, 100)];

    for (count, page_size) in counts_and_pages {
        let case = format!("{count} records, maxResults {page_size}");
        // A fresh registry per row, so each row's set is exactly what it created.
        let registry_id = ready_registry(&router, &format!("paging-{count}-{page_size}")).await;
        let registry_arn = get_registry(&router, &registry_id)
            .await
            .str_field("registryArn")
            .to_string();

        let mut created: Vec<String> = Vec::new();
        for index in 0..count {
            let reply = create_record(
                &router,
                &registry_id,
                json!({
                    "name": format!("record-{index}"),
                    "recordType": "CUSTOM",
                    "descriptors": descriptors_for("CUSTOM")
                }),
            )
            .await;
            assert_eq!(
                reply.status, 202,
                "case {case:?}: creating record {index} answers 202, got {} {:?}",
                reply.status, reply.body
            );
            created.push(id_of(reply.str_field("recordArn")).to_string());
        }
        created.sort();

        // Walk every page and collect what the pages reported.
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let body = match &token {
                Some(token) => json!({ "maxResults": page_size, "nextToken": token }),
                None => json!({ "maxResults": page_size }),
            };
            let reply = list_records(&router, &registry_id, body).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: ListRegistryRecords answers 200, got {} {:?}",
                reply.status, reply.body
            );
            let items = reply
                .field("registryRecords")
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                items.len() <= page_size as usize,
                "case {case:?}: a page never exceeds maxResults, got {} items",
                items.len()
            );
            for item in &items {
                assert_eq!(
                    item["registryArn"].as_str().unwrap_or_default(),
                    registry_arn,
                    "case {case:?}: every summary reports its parent registry, got {item:?}"
                );
                seen.push(item["recordId"].as_str().unwrap_or_default().to_string());
            }
            pages += 1;
            assert!(
                pages <= count + 1,
                "case {case:?}: pagination must terminate, got {pages} pages"
            );
            match reply.field("nextToken").as_str() {
                Some(next) => {
                    assert!(
                        !items.is_empty(),
                        "case {case:?}: a token is only handed out with a non-empty page"
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
        }

        let mut sorted_seen = seen.clone();
        sorted_seen.sort();
        assert_eq!(
            sorted_seen, created,
            "case {case:?}: paging reports every stored record exactly once, got {seen:?}"
        );
        let in_id_order = seen.windows(2).all(|pair| pair[0] < pair[1]);
        assert!(
            in_id_order,
            "case {case:?}: pages run in record-id order, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn list_registry_records_honours_the_filters_the_model_declares() {
    let router = router();
    let registry_id = ready_registry(&router, "filter-registry").await;

    // Three records that differ in name and recordType, so each filter attribute
    // (source: the RegistryRecordFilterName enum — name | status | recordType) selects a
    // different subset.
    for (name, record_type) in [
        ("alpha", "MCP"),
        ("beta", "AGENT"),
        ("gamma", "MCP"),
    ] {
        let reply = create_record(
            &router,
            &registry_id,
            json!({
                "name": name,
                "recordType": record_type,
                "descriptors": descriptors_for(record_type)
            }),
        )
        .await;
        assert_eq!(
            reply.status, 202,
            "case {name:?}: creating the record answers 202, got {} {:?}",
            reply.status, reply.body
        );
    }
    wait_past_the_transition().await;

    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "by name",
            json!({ "filters": [{ "name": "name", "values": ["beta"] }] }),
            vec!["beta"],
        ),
        (
            "by recordType",
            json!({ "filters": [{ "name": "recordType", "values": ["MCP"] }] }),
            vec!["alpha", "gamma"],
        ),
        (
            "by status",
            json!({ "filters": [{ "name": "status", "values": ["DRAFT"] }] }),
            vec!["alpha", "beta", "gamma"],
        ),
        (
            "by a status nothing holds",
            json!({ "filters": [{ "name": "status", "values": ["APPROVED"] }] }),
            vec![],
        ),
        (
            "two filters together",
            json!({ "filters": [
                { "name": "recordType", "values": ["MCP"] },
                { "name": "name", "values": ["gamma"] }
            ] }),
            vec!["gamma"],
        ),
        ("no filter", json!({}), vec!["alpha", "beta", "gamma"]),
    ];

    for (case, body, expected) in cases {
        let reply = list_records(&router, &registry_id, body).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: ListRegistryRecords answers 200, got {} {:?}",
            reply.status, reply.body
        );
        let mut names: Vec<String> = reply
            .field("registryRecords")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|item| item["name"].as_str().unwrap_or_default().to_string())
            .collect();
        names.sort();
        let selected: Vec<&str> = names.iter().map(String::as_str).collect();
        assert_eq!(
            selected, expected,
            "case {case:?}: the filter selects exactly the matching records, got {selected:?}"
        );
    }

    // A filter attribute outside `RegistryRecordFilterName`'s catalogue (source:
    // `REGISTRY_RECORD_FILTER_NAME_VALUES` = name | recordType | status) is refused, not
    // silently ignored: rows for an attribute the record has but the model does not make
    // filterable, for one no shape declares, and for the right name in the wrong case.
    for case in ["displayName", "not-an-attribute", "RecordType"] {
        let refused = list_records(
            &router,
            &registry_id,
            json!({ "filters": [{ "name": case, "values": ["alpha"] }] }),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a filter name outside the model's catalogue is a 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ValidationException"),
            "case {case:?}: the modelled error code is ValidationException, got {:?}",
            refused.error_code
        );
    }

    // A token this service never issued is refused, not treated as the first page.
    // Rows: one that is not even decodable, and one a different registry issued.
    let other_registry = ready_registry(&router, "other-filter-registry").await;
    for index in 0..2 {
        let reply = create_record(
            &router,
            &other_registry,
            json!({
                "name": format!("elsewhere-{index}"),
                "recordType": "CUSTOM",
                "descriptors": descriptors_for("CUSTOM")
            }),
        )
        .await;
        assert_eq!(
            reply.status, 202,
            "case \"seed the other registry\": got {} {:?}",
            reply.status, reply.body
        );
    }
    let issued_elsewhere = list_records(&router, &other_registry, json!({ "maxResults": 1 }))
        .await
        .str_field("nextToken")
        .to_string();
    assert!(
        !issued_elsewhere.is_empty(),
        "case \"token of another registry\": the other registry must hand out a token"
    );

    for (case, token) in [
        ("undecodable token", "not-a-token".to_string()),
        ("token of another registry", issued_elsewhere),
    ] {
        let refused = list_records(&router, &registry_id, json!({ "nextToken": token })).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a token this registry did not issue is a 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ValidationException"),
            "case {case:?}: the modelled error code is ValidationException, got {:?}",
            refused.error_code
        );
    }
}

#[tokio::test]
async fn list_registry_records_pages_at_the_models_maximum_when_no_max_results_is_given() {
    let router = router();
    let registry_id = ready_registry(&router, "default-page-registry").await;

    // One more record than the model's `MaxResults` maximum (1..100), so a request that
    // names no `maxResults` cannot answer with the whole registry.
    let total = 101;
    for index in 0..total {
        let reply = create_record(
            &router,
            &registry_id,
            json!({
                "name": format!("bulk-{index:03}"),
                "recordType": "CUSTOM",
                "descriptors": descriptors_for("CUSTOM")
            }),
        )
        .await;
        assert_eq!(
            reply.status, 202,
            "case \"bulk record {index}\": got {} {:?}",
            reply.status, reply.body
        );
    }

    let first = list_records(&router, &registry_id, json!({})).await;
    let first_page = first
        .field("registryRecords")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        first_page.len(),
        100,
        "case \"no maxResults\": the page is capped at the model's maximum, got {} items",
        first_page.len()
    );
    let token = first.str_field("nextToken").to_string();
    assert!(
        !token.is_empty(),
        "case \"no maxResults\": a capped page hands out a nextToken, got {:?}",
        first.body
    );

    let second = list_records(&router, &registry_id, json!({ "nextToken": token })).await;
    let second_page = second
        .field("registryRecords")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        second_page.len(),
        total - 100,
        "case \"resume the default page\": the rest of the registry follows, got {} items",
        second_page.len()
    );
    assert!(
        second.field("nextToken").is_null(),
        "case \"resume the default page\": the last page carries no token, got {:?}",
        second.body
    );
}

#[tokio::test]
async fn an_idempotency_token_is_scoped_to_its_operation_and_dropped_with_its_resource() {
    let router = router();
    // One token, used by two different creates: the record create must create a record
    // rather than replay the registry's answer.
    let token = "onetokenusedbytwodifferentcreates001";

    let registry = create_registry(&router, json!({ "name": "scoped-token", "clientToken": token }))
        .await;
    assert_eq!(
        registry.status, 202,
        "case \"create the registry\": got {} {:?}",
        registry.status, registry.body
    );
    let registry_arn = registry.str_field("registryArn").to_string();
    let registry_id = id_of(&registry_arn).to_string();
    wait_past_the_transition().await;

    let record = create_record(
        &router,
        &registry_id,
        json!({
            "name": "scoped-token-record",
            "recordType": "CUSTOM",
            "descriptors": descriptors_for("CUSTOM"),
            "clientToken": token
        }),
    )
    .await;
    assert_eq!(
        record.status, 202,
        "case \"same token, other operation\": got {} {:?}",
        record.status, record.body
    );
    let record_arn = record.str_field("recordArn").to_string();
    assert!(
        record_arn.starts_with(&format!("{registry_arn}/record/")),
        "case \"same token, other operation\": the record create must mint a record ARN, got {record_arn:?}"
    );

    // Deleting the resource drops what the token remembered, so the same token creates
    // afresh instead of answering with something that no longer exists.
    let deleted = delete_record(&router, &registry_id, id_of(&record_arn)).await;
    assert_eq!(
        deleted.status, 200,
        "case \"delete the record\": got {} {:?}",
        deleted.status, deleted.body
    );
    let recreated = create_record(
        &router,
        &registry_id,
        json!({
            "name": "scoped-token-record",
            "recordType": "CUSTOM",
            "descriptors": descriptors_for("CUSTOM"),
            "clientToken": token
        }),
    )
    .await;
    assert_eq!(
        recreated.status, 202,
        "case \"replay after deletion\": got {} {:?}",
        recreated.status, recreated.body
    );
    assert_ne!(
        recreated.str_field("recordArn"),
        record_arn,
        "case \"replay after deletion\": a token naming a deleted record must create a new one, got {:?}",
        recreated.body
    );
    let readable = get_record(&router, &registry_id, id_of(recreated.str_field("recordArn"))).await;
    assert_eq!(
        readable.status, 200,
        "case \"replay after deletion\": the new record is readable, got {} {:?}",
        readable.status, readable.body
    );

    // A registry holding a record is not deleted, so the record (and the token that
    // remembers it) outlives the attempt: the refused delete leaves the registry READY and
    // the replay still answers the stored record.
    let registry_delete_refused = call(
        &router,
        "DELETE",
        &format!("/registries/{registry_id}"),
        None,
    )
    .await;
    assert_eq!(
        registry_delete_refused.status, 409,
        "case \"delete the parent registry while it holds a record\": got {} {:?}",
        registry_delete_refused.status,
        registry_delete_refused.body
    );
    let replay_after_refusal = create_record(
        &router,
        &registry_id,
        json!({
            "name": "scoped-token-record",
            "recordType": "CUSTOM",
            "descriptors": descriptors_for("CUSTOM"),
            "clientToken": token
        }),
    )
    .await;
    assert_eq!(
        replay_after_refusal.status, 202,
        "case \"replay after the registry's delete was refused\": a replay is not refused, got {} {:?}",
        replay_after_refusal.status,
        replay_after_refusal.body
    );
    assert_eq!(
        replay_after_refusal.str_field("recordArn"),
        recreated.str_field("recordArn"),
        "case \"replay after the registry's delete was refused\": the replay answers the stored record, got {:?}",
        replay_after_refusal.body
    );
}

#[tokio::test]
async fn delete_registry_record_removes_it_from_state() {
    let router = router();
    let registry_id = ready_registry(&router, "delete-record-registry").await;

    let mut ids = Vec::new();
    for index in 0..3 {
        let reply = create_record(
            &router,
            &registry_id,
            json!({
                "name": format!("doomed-{index}"),
                "recordType": "CUSTOM",
                "descriptors": descriptors_for("CUSTOM")
            }),
        )
        .await;
        ids.push(id_of(reply.str_field("recordArn")).to_string());
    }

    // Rows: delete each of the three in turn, so the removal is not tied to one position.
    for (index, record_id) in ids.iter().enumerate() {
        let case = format!("record {index}");
        let deleted = delete_record(&router, &registry_id, record_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: DeleteRegistryRecord answers 200, got {} {:?}",
            deleted.status, deleted.body
        );

        let got = get_record(&router, &registry_id, record_id).await;
        assert_eq!(
            got.status, 404,
            "case {case:?}: a deleted record is gone, got {} {:?}",
            got.status, got.body
        );

        let listed = list_records(&router, &registry_id, json!({})).await;
        let remaining: Vec<String> = listed
            .field("registryRecords")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|item| item["recordId"].as_str().unwrap_or_default().to_string())
            .collect();
        assert!(
            !remaining.contains(record_id),
            "case {case:?}: a deleted record is no longer listed, got {remaining:?}"
        );
        assert_eq!(
            remaining.len(),
            ids.len() - index - 1,
            "case {case:?}: only the deleted record left the registry, got {remaining:?}"
        );

        // Deleting it again is the modelled 404 for an unknown record, not a panic.
        let again = delete_record(&router, &registry_id, record_id).await;
        assert_eq!(
            again.status, 404,
            "case {case:?}: deleting an absent record is 404, got {} {:?}",
            again.status, again.body
        );
    }
}

#[tokio::test]
async fn delete_registry_moves_an_emptied_registry_to_deleting_and_then_removes_it() {
    let router = router();
    // Created with a token, so the test can also show that removing the registry drops
    // what that token remembered.
    let token = "tokenforthedoomedregistrycreate00001";
    let created = create_registry(
        &router,
        json!({ "name": "doomed-registry", "clientToken": token }),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case \"create the registry\": got {} {:?}",
        created.status, created.body
    );
    let first_arn = created.str_field("registryArn").to_string();
    let registry_id = id_of(&first_arn).to_string();
    wait_past_the_transition().await;
    let record = create_record(
        &router,
        &registry_id,
        json!({
            "name": "doomed-record",
            "recordType": "CUSTOM",
            "descriptors": descriptors_for("CUSTOM")
        }),
    )
    .await;
    let record_id = id_of(record.str_field("recordArn")).to_string();

    // A registry is deleted only once it is empty, so the record goes first; the refusal a
    // non-empty registry answers with has its own suite
    // (`tests/all/registry_delete_precondition.rs`).
    let removed_record = delete_record(&router, &registry_id, &record_id).await;
    assert_eq!(
        removed_record.status, 200,
        "case \"empty the registry first\": DeleteRegistryRecord answers 200, got {} {:?}",
        removed_record.status, removed_record.body
    );

    let deleted = call(
        &router,
        "DELETE",
        &format!("/registries/{registry_id}"),
        None,
    )
    .await;
    assert_eq!(
        deleted.status, 202,
        "case \"delete an emptied READY registry\": DeleteRegistry answers 202, got {} {:?}",
        deleted.status, deleted.body
    );
    assert_eq!(
        deleted.str_field("status"),
        "DELETING",
        "case \"delete an emptied READY registry\": deletion is asynchronous, so the status is DELETING, got {:?}",
        deleted.body
    );

    // While deletion is in flight the registry still reports itself, as DELETING.
    let during = get_registry(&router, &registry_id).await;
    assert_eq!(
        during.str_field("status"),
        "DELETING",
        "case \"read during deletion\": the transient status is visible, got {:?}",
        during.body
    );

    // @idempotent: a second delete reports the same status and does not fail.
    let again = call(
        &router,
        "DELETE",
        &format!("/registries/{registry_id}"),
        None,
    )
    .await;
    assert_eq!(
        again.status, 202,
        "case \"delete again\": a repeated delete is accepted, got {} {:?}",
        again.status, again.body
    );
    assert_eq!(
        again.str_field("status"),
        "DELETING",
        "case \"delete again\": the status is unchanged, got {:?}",
        again.body
    );

    wait_past_the_transition().await;
    let gone = get_registry(&router, &registry_id).await;
    assert_eq!(
        gone.status, 404,
        "case \"read after deletion\": the registry is removed, got {} {:?}",
        gone.status, gone.body
    );
    let record_gone = get_record(&router, &registry_id, &record_id).await;
    assert_eq!(
        record_gone.status, 404,
        "case \"read a record of a registry that was deleted\": neither is there any more, got {} {:?}",
        record_gone.status, record_gone.body
    );

    // The name the deleted registry held is free again, and the token that created it
    // no longer names anything, so the same token creates a NEW registry.
    let reused = create_registry(
        &router,
        json!({ "name": "doomed-registry", "clientToken": token }),
    )
    .await;
    assert_eq!(
        reused.status, 202,
        "case \"reuse the name of a removed registry\": the name is free again, got {} {:?}",
        reused.status, reused.body
    );
    assert_ne!(
        reused.str_field("registryArn"),
        first_arn,
        "case \"replay a token naming a removed registry\": it must create a new registry, got {:?}",
        reused.body
    );
}

// ---------------------------------------------------------------------------------------
// A registry's custom-metadata schema, and the configuration blocks a read reports.
//
// The rule under test: a registry's `customMetadataSchemaConfiguration` constrains every
// write of a record's `customMetadata` — the `defaultSchema` for every record type plus the
// override declared for the record's own type — so metadata that does not satisfy it is
// refused with a 400 `ValidationException` naming the offending node, before any state
// precondition of the write; and a read reports the verdict the registry's CURRENT schema
// gives the stored metadata, together with the registry's effective discovery and approval
// configuration.
// ---------------------------------------------------------------------------------------

/// The registry's `defaultSchema`: the common attributes every record type may carry.
const COMMON_ATTRIBUTE_SCHEMA: &str =
    r#"{"type":"object","properties":{"owner":{"type":"string"}}}"#;

/// The `recordTypeSchemaOverrides` schema for `CUSTOM` records: a required property whose
/// value has to come from an enumeration.
const CUSTOM_RECORD_SCHEMA: &str = r#"{"type":"object","properties":{"environment":{"enum":["dev","prod"]}},"required":["environment"]}"#;

/// The `recordTypeSchemaOverrides` schema for `MCP` records, so a second record type is
/// constrained differently from the first.
const MCP_RECORD_SCHEMA: &str =
    r#"{"type":"object","properties":{"tier":{"type":"integer"}},"required":["tier"]}"#;

/// The configuration a registry under test carries: the common attributes plus one
/// override per constrained record type. Record types with no override carry the common
/// attributes alone.
fn schema_configuration() -> Value {
    json!({
        "defaultSchema": COMMON_ATTRIBUTE_SCHEMA,
        "recordTypeSchemaOverrides": [
            { "recordType": "CUSTOM", "schema": CUSTOM_RECORD_SCHEMA },
            { "recordType": "MCP", "schema": MCP_RECORD_SCHEMA }
        ]
    })
}

/// How this service words a refusal of custom metadata the registry's schema does not
/// admit: the fixed text with the request's record type, then the failure — the JSON
/// pointer of the offending node and the constraint it does not satisfy.
fn schema_refusal(record_type: &str, failure: &str) -> String {
    format!("Custom metadata validation failed for record type {record_type}: {failure}")
}

/// Create a registry from a whole request body and wait for the creation the clock owes to
/// be reported, so it is `READY`; returns its registry id. [`ready_registry`] for a registry
/// whose request carries nothing but a name.
async fn ready_registry_with(router: &MultiServiceRouter, body: Value) -> String {
    let created = create_registry(router, body.clone()).await;
    assert_eq!(
        created.status, 202,
        "case {body:?}: CreateRegistry must answer 202, got {} {:?}",
        created.status, created.body
    );
    let registry_id = id_of(created.str_field("registryArn")).to_string();
    registry_at_status(router, &registry_id, "READY", &format!("{body:?}")).await;
    registry_id
}

/// How long a test looks for a status the clock owes before giving up. GENEROUS: what is
/// waited for is a CONDITION the next call reports, not a span of time, so a loaded host only
/// makes the loop go round more often.
const STATUS_DEADLINE: Duration = Duration::from_secs(30);

/// How often the loop looks again.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Poll `GetRegistry` until the registry reports `status`, and answer the reply that did.
/// Panics, naming the case and the last status seen, if the deadline passes first.
async fn registry_at_status(
    router: &MultiServiceRouter,
    registry_id: &str,
    status: &str,
    case: &str,
) -> Reply {
    let deadline = tokio::time::Instant::now() + STATUS_DEADLINE;
    loop {
        let got = get_registry(router, registry_id).await;
        if got.str_field("status") == status {
            return got;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case:?}: the registry never reported {status}, last answer {} {:?}",
            got.status,
            got.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Poll `GetRegistryRecord` until the record reports `status`, and answer the reply that did.
/// Panics, naming the case and the last status seen, if the deadline passes first.
async fn record_at_status(
    router: &MultiServiceRouter,
    registry_id: &str,
    record_id: &str,
    status: &str,
    case: &str,
) -> Reply {
    let deadline = tokio::time::Instant::now() + STATUS_DEADLINE;
    loop {
        let got = get_record(router, registry_id, record_id).await;
        if got.str_field("status") == status {
            return got;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case:?}: the record never reported {status}, last answer {} {:?}",
            got.status,
            got.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// A router over this service together with the engine its state lives in, so a test can
/// also act on the stored registry the way an update of its schema would.
fn router_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_agentregistrycontrol_agentregistrycontrol::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

/// Replace the schema a stored registry declares for `record_type`, which is the state an
/// update of the registry's `customMetadataSchemaConfiguration` leaves behind: what a
/// record's compliance verdict is computed against from then on. The common attributes the
/// registry declares are left alone, so only the record type's own schema changes.
fn declare_record_type_schema(
    engine: &Arc<engine::InMemoryEngine>,
    registry_id: &str,
    record_type: &str,
    schema: &str,
) {
    use service_com_amazonaws_agentregistrycontrol_agentregistrycontrol::ops::types::{
        CustomMetadataSchemaConfiguration, RecordTypeSchemaOverride,
    };
    use service_com_amazonaws_agentregistrycontrol_agentregistrycontrol::state as owner;

    let state = engine.state::<owner::ComAmazonawsAgentregistrycontrolAgentregistrycontrolState>();
    let mut state = state.lock();
    let registry = state
        .registries
        .get_mut(registry_id)
        .expect("the registry under test is stored");
    let default_schema = registry
        .custom_metadata_schema_configuration
        .as_ref()
        .and_then(|configuration| configuration.default_schema.clone());
    registry.custom_metadata_schema_configuration = Some(CustomMetadataSchemaConfiguration {
        default_schema,
        record_type_schema_overrides: Some(vec![RecordTypeSchemaOverride {
            record_type: record_type.to_string(),
            schema: schema.to_string(),
        }]),
    });
}

/// What a create carrying a row's metadata must answer: a refusal naming the failure the
/// registry's schema reports, or the record created and reading back a compliance verdict.
/// A create is refused for metadata it SUPPLIES; what the record then conforms to is read
/// back as `customMetadataSchemaComplianceStatus`.
enum Expected {
    /// 400 `ValidationException` whose message states this failure.
    Refused(&'static str),
    /// 202, and the stored record reads back this compliance status — or no status at all
    /// (`None`), where no verdict about its metadata could be reached.
    Created(Option<&'static str>),
}

#[tokio::test]
async fn custom_metadata_must_satisfy_the_schema_the_registry_declares_for_the_record_type() {
    let router = router();
    let registry_id = ready_registry_with(
        &router,
        json!({
            "name": "schema-enforcing-registry",
            "customMetadataSchemaConfiguration": schema_configuration()
        }),
    )
    .await;

    // Rows differ along the record type whose schema applies, the keyword that fails
    // (enumeration, required property, declared type, a field the schemas do not
    // enumerate), whether the metadata is present at all, and which of the two schemas is
    // the one that is not satisfied — the override's or the common attributes', which apply
    // to a record type that has no override of its own. The last column is what the create
    // must answer.
    let cases: Vec<(&str, &str, Option<Value>, Expected)> = vec![
        (
            "a value outside the enumeration the override declares",
            "CUSTOM",
            Some(json!({ "environment": "staging" })),
            Expected::Refused(
                "/environment: does not have a value in the enumeration [\"dev\", \"prod\"]",
            ),
        ),
        (
            "metadata omitting a property the override requires",
            "CUSTOM",
            Some(json!({ "owner": "platform" })),
            Expected::Refused(": required property 'environment' not found"),
        ),
        (
            // `customMetadata` is not a required member, so a create that carries none
            // asserts nothing about the record's metadata: it is accepted, and the record
            // reads back the verdict its missing metadata earns.
            "no metadata at all where the override requires a property",
            "CUSTOM",
            None,
            Expected::Created(Some("NON_COMPLIANT")),
        ),
        (
            "metadata the override and the common attributes both admit",
            "CUSTOM",
            Some(json!({ "environment": "prod", "owner": "platform" })),
            Expected::Created(Some("COMPLIANT")),
        ),
        (
            // `tier` is enumerated by the MCP override alone, so it is not a field a
            // CUSTOM record may carry.
            "a field only another record type's override declares",
            "CUSTOM",
            Some(json!({ "environment": "dev", "tier": 3 })),
            Expected::Refused(
                ": property 'tier' is not defined in the schema and the schema does not \
                 allow additional properties",
            ),
        ),
        (
            "a value whose type is not the one the override declares",
            "MCP",
            Some(json!({ "tier": "gold" })),
            Expected::Refused("/tier: string found, integer expected"),
        ),
        (
            "a second record type's own override, satisfied",
            "MCP",
            Some(json!({ "tier": 3 })),
            Expected::Created(Some("COMPLIANT")),
        ),
        (
            "a record type with no override, satisfying the common attributes",
            "AGENT",
            Some(json!({ "owner": "agents" })),
            Expected::Created(Some("COMPLIANT")),
        ),
        (
            "a record type with no override, violating the common attributes",
            "AGENT",
            Some(json!({ "owner": 7 })),
            Expected::Refused("/owner: integer found, string expected"),
        ),
    ];

    for (at, (case, record_type, metadata, expected)) in cases.into_iter().enumerate() {
        let name = format!("probe-{at}");
        let mut body = json!({
            "name": name,
            "recordType": record_type,
            "descriptors": descriptors_for(record_type)
        });
        if let Some(metadata) = &metadata {
            body["customMetadata"] = metadata.clone();
        }
        let created = create_record(&router, &registry_id, body).await;

        let failure = match expected {
            Expected::Created(compliance) => {
                assert_eq!(
                    created.status, 202,
                    "case {case:?}: a create the schema does not refuse is accepted, got {} {:?}",
                    created.status, created.body
                );
                // And the record reads back the verdict the registry's schema gives what it
                // carries — or no verdict, where nothing could be known about it.
                let record_id = id_of(created.str_field("recordArn")).to_string();
                let got = record_at_status(&router, &registry_id, &record_id, "DRAFT", case).await;
                assert_eq!(
                    got.field("customMetadataSchemaComplianceStatus"),
                    &compliance.map_or(Value::Null, |status| json!(status)),
                    "case {case:?}: the stored record reads back {compliance:?}, got {:?}",
                    got.body
                );
                continue;
            }
            Expected::Refused(failure) => failure,
        };

        assert_eq!(
            created.status, 400,
            "case {case:?}: metadata the schema does not admit is refused with 400, got {} {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.error_code.as_deref(),
            Some("ValidationException"),
            "case {case:?}: the modelled error code is ValidationException, got {:?}",
            created.error_code
        );
        assert_eq!(
            created.str_field("message"),
            schema_refusal(record_type, failure),
            "case {case:?}: the refusal states the record type and the failure, got {:?}",
            created.body
        );
        assert_eq!(
            created.str_field("reason"),
            "FieldValidationFailed",
            "case {case:?}: the refusal is a field validation failure, got {:?}",
            created.body
        );
    }
}

#[tokio::test]
async fn metadata_the_schema_refuses_leaves_the_record_name_free() {
    let router = router();
    let registry_id = ready_registry_with(
        &router,
        json!({
            "name": "nothing-written-registry",
            "customMetadataSchemaConfiguration": schema_configuration()
        }),
    )
    .await;

    // A create the schema refuses writes nothing, so the name it asked for is still free
    // and the registry still holds no record — a later call must not be answered about a
    // name conflict with a record that was never created.
    let name = "arc-probe.one";
    for (case, metadata, failure) in [
        (
            "a value outside the enumeration",
            json!({ "environment": "staging" }),
            "/environment: does not have a value in the enumeration [\"dev\", \"prod\"]",
        ),
        (
            "a missing required property",
            json!({ "owner": "platform" }),
            ": required property 'environment' not found",
        ),
    ] {
        let refused = create_record(
            &router,
            &registry_id,
            json!({
                "name": name,
                "recordType": "CUSTOM",
                "descriptors": descriptors_for("CUSTOM"),
                "customMetadata": metadata
            }),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: the refusal is a 400 validation failure and not a name conflict, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.str_field("message"),
            schema_refusal("CUSTOM", failure),
            "case {case:?}: the refusal names the metadata failure, got {:?}",
            refused.body
        );
        let listed = list_records(&router, &registry_id, json!({})).await;
        assert_eq!(
            listed.field("registryRecords"),
            &json!([]),
            "case {case:?}: a refused create writes no record, got {:?}",
            listed.body
        );
    }

    // So the same name, with metadata the schema admits, creates the record.
    let accepted = create_record(
        &router,
        &registry_id,
        json!({
            "name": name,
            "recordType": "CUSTOM",
            "descriptors": descriptors_for("CUSTOM"),
            "customMetadata": { "environment": "dev" }
        }),
    )
    .await;
    assert_eq!(
        accepted.status, 202,
        "case \"the name a refused create asked for\": it is still free, got {} {:?}",
        accepted.status, accepted.body
    );
}

#[tokio::test]
async fn a_records_compliance_is_computed_against_the_registrys_current_schema() {
    let (router, engine) = router_and_engine();
    let registry_id = ready_registry_with(
        &router,
        json!({
            "name": "compliance-registry",
            "customMetadataSchemaConfiguration": schema_configuration()
        }),
    )
    .await;
    let created = create_record(
        &router,
        &registry_id,
        json!({
            "name": "compliance-probe",
            "recordType": "CUSTOM",
            "descriptors": descriptors_for("CUSTOM"),
            "customMetadata": { "environment": "dev", "owner": "platform" }
        }),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case \"a record the schema admits\": it is created, got {} {:?}",
        created.status, created.body
    );
    let record_id = id_of(created.str_field("recordArn")).to_string();
    record_at_status(
        &router,
        &registry_id,
        &record_id,
        "DRAFT",
        "a record the schema admits",
    )
    .await;

    // Each row is a schema the registry declares for this record type, and the verdict the
    // record's unchanged metadata earns under it: the record is never written again, so
    // what changes the verdict is the schema alone.
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "the schema the record was written under",
            CUSTOM_RECORD_SCHEMA,
            json!("COMPLIANT"),
        ),
        (
            "a schema whose enumeration no longer admits the stored value",
            r#"{"type":"object","properties":{"environment":{"enum":["prod"]}},"required":["environment"]}"#,
            json!("NON_COMPLIANT"),
        ),
        (
            "a schema requiring a property the stored metadata does not carry",
            r#"{"type":"object","required":["costCentre"]}"#,
            json!("NON_COMPLIANT"),
        ),
        (
            "a schema that admits the stored metadata again",
            r#"{"type":"object","properties":{"environment":{"type":"string"}}}"#,
            json!("COMPLIANT"),
        ),
        (
            // The common attributes still declare `owner`, so what this schema no longer
            // enumerates is `environment`: a field the stored metadata carries and no
            // applicable schema declares any more.
            "a schema that no longer enumerates a field the stored metadata carries",
            r#"{"type":"object","properties":{"tier":{"type":"integer"}}}"#,
            json!("NON_COMPLIANT"),
        ),
        (
            // No verdict can be reached about metadata whose schema cannot be read, so
            // nothing is claimed about it rather than claiming either verdict.
            "a schema document that is not a JSON Schema at all",
            "not a schema",
            Value::Null,
        ),
    ];

    for (case, schema, expected) in cases {
        declare_record_type_schema(&engine, &registry_id, "CUSTOM", schema);
        let got = get_record(&router, &registry_id, &record_id).await;
        assert_eq!(
            got.field("customMetadataSchemaComplianceStatus"),
            &expected,
            "case {case:?}: GetRegistryRecord reports the verdict of the current schema, got {:?}",
            got.body
        );
        // The stored record itself is untouched by the verdict a read reports.
        assert_eq!(
            got.field("customMetadata"),
            &json!({ "environment": "dev", "owner": "platform" }),
            "case {case:?}: reporting a verdict changes no stored metadata, got {:?}",
            got.body
        );
        // And a list reports the same verdict for the same record, since both compute it
        // from the registry's schema and the record's metadata.
        let listed = list_records(&router, &registry_id, json!({})).await;
        assert_eq!(
            &listed.field("registryRecords")[0]["customMetadataSchemaComplianceStatus"],
            &expected,
            "case {case:?}: the list summary carries the same verdict, got {:?}",
            listed.body
        );
    }
}

#[tokio::test]
async fn a_record_of_a_registry_with_no_schema_carries_no_metadata_to_violate() {
    let router = router();
    let registry_id = ready_registry(&router, "unconstrained-registry").await;
    for (case, body) in [
        (
            "metadata of any shape",
            json!({
                "name": "free-form",
                "recordType": "CUSTOM",
                "descriptors": descriptors_for("CUSTOM"),
                "customMetadata": { "anything": "goes", "nested": true }
            }),
        ),
        (
            "no metadata at all",
            json!({
                "name": "no-metadata",
                "recordType": "MCP",
                "descriptors": descriptors_for("MCP")
            }),
        ),
    ] {
        let created = create_record(&router, &registry_id, body).await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: a registry that declares no schema constrains nothing, got {} {:?}",
            created.status, created.body
        );
        let record_id = id_of(created.str_field("recordArn")).to_string();
        let got = record_at_status(&router, &registry_id, &record_id, "DRAFT", case).await;
        assert_eq!(
            got.field("customMetadataSchemaComplianceStatus"),
            &json!("COMPLIANT"),
            "case {case:?}: there is no schema the metadata fails to satisfy, got {:?}",
            got.body
        );    }
}

#[tokio::test]
async fn an_edit_must_leave_custom_metadata_the_registrys_schema_admits() {
    let router = router();
    let registry_id = ready_registry_with(
        &router,
        json!({
            "name": "edit-schema-registry",
            "customMetadataSchemaConfiguration": schema_configuration()
        }),
    )
    .await;
    let created = create_record(
        &router,
        &registry_id,
        json!({
            "name": "edit-probe",
            "recordType": "CUSTOM",
            "descriptors": descriptors_for("CUSTOM"),
            "customMetadata": { "environment": "dev" }
        }),
    )
    .await;
    let record_id = id_of(created.str_field("recordArn")).to_string();
    record_at_status(&router, &registry_id, &record_id, "DRAFT", "edit-probe").await;

    // Rows differ along what the patch would leave on the record: a replacement set the
    // schema does not admit, a cleared set where the schema requires a property, and a
    // replacement set it does admit.
    let cases: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "a replacement value outside the enumeration",
            json!({ "customMetadata": { "optionalValue": { "environment": "staging" } } }),
            Some("/environment: does not have a value in the enumeration [\"dev\", \"prod\"]"),
        ),
        (
            "clearing metadata the schema requires",
            json!({ "customMetadata": {} }),
            Some(": required property 'environment' not found"),
        ),
        (
            "a replacement set the schema admits",
            json!({ "customMetadata": { "optionalValue": { "environment": "prod", "owner": "platform" } } }),
            None,
        ),
    ];

    for (case, patch, failure) in cases {
        let patched = call(
            &router,
            "PATCH",
            &format!(
                "/registries/{}/records/{}",
                encode_label(&registry_id),
                encode_label(&record_id)
            ),
            Some(patch),
        )
        .await;
        match failure {
            Some(failure) => {
                assert_eq!(
                    patched.status, 400,
                    "case {case:?}: an edit the schema does not admit is refused, got {} {:?}",
                    patched.status, patched.body
                );
                assert_eq!(
                    patched.str_field("message"),
                    schema_refusal("CUSTOM", failure),
                    "case {case:?}: the refusal states the record type and the failure, got {:?}",
                    patched.body
                );
                // The refused edit wrote nothing: the record still carries what it had.
                let got = get_record(&router, &registry_id, &record_id).await;
                assert_eq!(
                    got.field("customMetadata"),
                    &json!({ "environment": "dev" }),
                    "case {case:?}: a refused edit leaves the stored metadata alone, got {:?}",
                    got.body
                );
            }
            None => {
                assert_eq!(
                    patched.status, 202,
                    "case {case:?}: an edit the schema admits is accepted, got {} {:?}",
                    patched.status, patched.body
                );
                assert_eq!(
                    patched.field("customMetadataSchemaComplianceStatus"),
                    &json!("COMPLIANT"),
                    "case {case:?}: the edited record satisfies the schema, got {:?}",
                    patched.body
                );
                let got =
                    record_at_status(&router, &registry_id, &record_id, "DRAFT", case).await;
                assert_eq!(
                    got.field("customMetadata"),
                    &json!({ "environment": "prod", "owner": "platform" }),
                    "case {case:?}: the replacement set is what a later read reports, got {:?}",
                    got.body
                );
            }
        }
    }
}

/// Submit a record for approval: `POST /registries/{registryId}/records/{recordId}/submit-for-approval`.
async fn submit_record(router: &MultiServiceRouter, registry_id: &str, record_id: &str) -> Reply {
    call(
        router,
        "POST",
        &format!(
            "/registries/{}/records/{}/submit-for-approval",
            encode_label(registry_id),
            encode_label(record_id)
        ),
        None,
    )
    .await
}

/// Patch a record: `PATCH /registries/{registryId}/records/{recordId}`.
async fn patch_record(
    router: &MultiServiceRouter,
    registry_id: &str,
    record_id: &str,
    body: Value,
) -> Reply {
    call(
        router,
        "PATCH",
        &format!(
            "/registries/{}/records/{}",
            encode_label(registry_id),
            encode_label(record_id)
        ),
        Some(body),
    )
    .await
}

#[tokio::test]
async fn custom_metadata_may_carry_only_the_fields_the_registrys_schema_enumerates() {
    let router = router();

    // A registry's schema states "the custom metadata fields that records in the registry
    // can carry", so a member it does not enumerate is as much a failure as a member whose
    // value it does not admit. Rows differ along HOW the schema speaks about members it does
    // not name: one that names its `properties` and nothing else (the closed case, with and
    // without the keyword spelled out), one that admits further members, one that matches
    // member names by pattern, one that names no members at all, and one that states its
    // members in a subschema this service does not read — the last four leaving nothing
    // undeclared. The metadata of every row carries the same two fields, so what changes
    // the answer is the schema alone.
    let metadata = json!({ "environment": "dev", "costCentre": "RD-7" });
    let undeclared = ": property 'costCentre' is not defined in the schema and the schema \
                      does not allow additional properties";
    let cases: Vec<(&str, &str, Expected)> = vec![
        (
            "a schema naming its properties and nothing further",
            r#"{"type":"object","properties":{"environment":{"type":"string","enum":["dev","prod"]}},"required":["environment"]}"#,
            Expected::Refused(undeclared),
        ),
        (
            "a schema that spells out that it admits no further members",
            r#"{"type":"object","properties":{"environment":{"type":"string"}},"additionalProperties":false}"#,
            Expected::Refused(undeclared),
        ),
        (
            "a schema that enumerates the member too",
            r#"{"type":"object","properties":{"environment":{"type":"string"},"costCentre":{"type":"string"}}}"#,
            Expected::Created(Some("COMPLIANT")),
        ),
        (
            "a schema that admits further members",
            r#"{"type":"object","properties":{"environment":{"type":"string"}},"additionalProperties":true}"#,
            Expected::Created(Some("COMPLIANT")),
        ),
        (
            "a schema that matches member names by pattern",
            r#"{"type":"object","patternProperties":{"^[a-zA-Z]+$":{"type":"string"}}}"#,
            Expected::Created(Some("COMPLIANT")),
        ),
        (
            "a schema that names no member at all",
            r#"{"type":"object"}"#,
            Expected::Created(Some("COMPLIANT")),
        ),
        (
            "a schema stating its members in a subschema",
            r#"{"allOf":[{"type":"object","properties":{"environment":{"type":"string"}}}]}"#,
            Expected::Created(Some("COMPLIANT")),
        ),
        (
            // It names `properties`, but it is no schema this evaluator can apply, so it
            // claims NOTHING about the metadata — the fields it names included. Nothing is
            // refused for being undeclared under it, and no verdict is reported.
            "a document that names properties but is no schema at all",
            r#"{"type":"object","properties":{"environment":{"type":"not-a-json-type"}}}"#,
            Expected::Created(None),
        ),
    ];

    for (at, (case, schema, expected)) in cases.into_iter().enumerate() {
        let registry_id = ready_registry_with(
            &router,
            json!({
                "name": format!("enumerated-fields-{at}"),
                "customMetadataSchemaConfiguration": { "defaultSchema": schema }
            }),
        )
        .await;
        let created = create_record(
            &router,
            &registry_id,
            json!({
                "name": "enumerated-probe",
                "recordType": "CUSTOM",
                "descriptors": descriptors_for("CUSTOM"),
                "customMetadata": metadata.clone()
            }),
        )
        .await;

        match expected {
            Expected::Refused(failure) => {
                assert_eq!(
                    created.status, 400,
                    "case {case:?}: a member the schema does not enumerate is refused, got {} {:?}",
                    created.status, created.body
                );
                assert_eq!(
                    created.error_code.as_deref(),
                    Some("ValidationException"),
                    "case {case:?}: the modelled error code is ValidationException, got {:?}",
                    created.error_code
                );
                assert_eq!(
                    created.str_field("message"),
                    schema_refusal("CUSTOM", failure),
                    "case {case:?}: the refusal names the undeclared member, got {:?}",
                    created.body
                );
                // Nothing was written, so the registry still holds no record.
                let listed = list_records(&router, &registry_id, json!({})).await;
                assert_eq!(
                    listed.field("registryRecords"),
                    &json!([]),
                    "case {case:?}: a refused create writes no record, got {:?}",
                    listed.body
                );
            }
            Expected::Created(compliance) => {
                assert_eq!(
                    created.status, 202,
                    "case {case:?}: a schema that leaves the member declared accepts it, got {} {:?}",
                    created.status, created.body
                );
                let record_id = id_of(created.str_field("recordArn")).to_string();
                let got = record_at_status(&router, &registry_id, &record_id, "DRAFT", case).await;
                assert_eq!(
                    got.field("customMetadataSchemaComplianceStatus"),
                    &compliance.map_or(Value::Null, |status| json!(status)),
                    "case {case:?}: the record reads back {compliance:?}, got {:?}",
                    got.body
                );
            }
        }
    }
}

#[tokio::test]
async fn a_create_that_supplies_no_metadata_is_accepted_and_its_verdict_gates_the_submission() {
    let router = router();

    // `customMetadata` is not a required member of a create, so a create that supplies none
    // asserts nothing about metadata and is accepted; what the record conforms to is then a
    // verdict a read reports, and the gate it has to pass is the submission that would
    // publish it. Rows differ along the registry's approval configuration, since that
    // decides only what an ACCEPTED submission lands at, never whether the record's
    // metadata lets it be submitted at all.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a registry that auto-approves submissions",
            json!({ "autoApprovalRules": ["APPROVE_ALL"] }),
            "APPROVED",
        ),
        (
            "a registry whose submissions wait for a curator",
            json!({ "autoApprovalRules": [] }),
            "PENDING_APPROVAL",
        ),
    ];
    // The field the registry requires of every record type, and the only one it enumerates.
    let schema = r#"{"type":"object","properties":{"environment":{"type":"string","enum":["dev","prod"]},"critical":{"type":"boolean"}},"required":["environment"]}"#;
    let missing_required = ": required property 'environment' not found";

    for (at, (case, approval, submitted_status)) in cases.into_iter().enumerate() {
        let registry_id = ready_registry_with(
            &router,
            json!({
                "name": format!("verdict-gate-{at}"),
                "approvalConfiguration": approval,
                "customMetadataSchemaConfiguration": { "defaultSchema": schema }
            }),
        )
        .await;

        let created = create_record(
            &router,
            &registry_id,
            json!({
                "name": "gated-probe",
                "recordType": "CUSTOM",
                "descriptors": descriptors_for("CUSTOM")
            }),
        )
        .await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: a create supplying no metadata is accepted, got {} {:?}",
            created.status, created.body
        );
        let record_id = id_of(created.str_field("recordArn")).to_string();
        let draft = record_at_status(&router, &registry_id, &record_id, "DRAFT", case).await;

        // The record exists and reads back the verdict its missing metadata earns, in a
        // read of the record itself and in a summary of the page it is listed on.
        assert_eq!(
            draft.field("customMetadataSchemaComplianceStatus"),
            &json!("NON_COMPLIANT"),
            "case {case:?}: a record missing a required field reads NON_COMPLIANT, got {:?}",
            draft.body
        );
        let listed = list_records(&router, &registry_id, json!({})).await;
        assert_eq!(
            &listed.field("registryRecords")[0]["customMetadataSchemaComplianceStatus"],
            &json!("NON_COMPLIANT"),
            "case {case:?}: the summary carries the same verdict, got {:?}",
            listed.body
        );

        // Submitting it is what the registry's schema refuses, in the same sentence a write
        // of that metadata is refused with — and the record stays where it was.
        let refused = submit_record(&router, &registry_id, &record_id).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: submitting a record the schema does not admit is refused, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ValidationException"),
            "case {case:?}: the modelled error code is ValidationException, got {:?}",
            refused.error_code
        );
        assert_eq!(
            refused.str_field("message"),
            schema_refusal("CUSTOM", missing_required),
            "case {case:?}: the refusal names the record type and the failure, got {:?}",
            refused.body
        );
        let still_draft = get_record(&router, &registry_id, &record_id).await;
        assert_eq!(
            still_draft.str_field("status"),
            "DRAFT",
            "case {case:?}: a refused submission leaves the record in DRAFT, got {:?}",
            still_draft.body
        );

        // An edit that supplies no metadata either is not refused for what the record
        // already carries: it asserts nothing about the metadata.
        let described = patch_record(
            &router,
            &registry_id,
            &record_id,
            json!({ "description": { "optionalValue": "awaiting its metadata" } }),
        )
        .await;
        assert_eq!(
            described.status, 202,
            "case {case:?}: an edit supplying no metadata is accepted, got {} {:?}",
            described.status, described.body
        );
        record_at_status(&router, &registry_id, &record_id, "DRAFT", case).await;

        // And once an edit supplies metadata the schema admits, the record reads COMPLIANT
        // and the submission the verdict held back is accepted.
        let filled = patch_record(
            &router,
            &registry_id,
            &record_id,
            json!({ "customMetadata": { "optionalValue": { "environment": "prod" } } }),
        )
        .await;
        assert_eq!(
            filled.status, 202,
            "case {case:?}: an edit supplying metadata the schema admits is accepted, got {} {:?}",
            filled.status, filled.body
        );
        let compliant = record_at_status(&router, &registry_id, &record_id, "DRAFT", case).await;
        assert_eq!(
            compliant.field("customMetadataSchemaComplianceStatus"),
            &json!("COMPLIANT"),
            "case {case:?}: the filled-in metadata satisfies the schema, got {:?}",
            compliant.body
        );
        let submitted = submit_record(&router, &registry_id, &record_id).await;
        assert_eq!(
            submitted.status, 202,
            "case {case:?}: a compliant record is submitted, got {} {:?}",
            submitted.status, submitted.body
        );
        assert_eq!(
            submitted.str_field("status"),
            submitted_status,
            "case {case:?}: the submission lands where the approval configuration decides, got {:?}",
            submitted.body
        );
    }
}

/// The registries a create that supplies NO `customMetadata` is made under, one row per
/// way the applicable schema can constrain (or fail to constrain) such a record: the case,
/// the registry's `customMetadataSchemaConfiguration` (`Value::Null` for a registry that
/// declares none), the `recordType` the create carries, and the property the schema that
/// applies to that record type REQUIRES — `None` where it requires none.
///
/// The record types are `RecordType`'s own catalogue (`smithy_query(query="shape",
/// shape="RecordType")`: AGENT | CUSTOM | GATEWAY | MCP | SKILL), every one of which a row
/// covers; the required properties differ in name and in declared type, so the message's
/// variable part cannot be a fixed word.
fn absent_metadata_table() -> Vec<(&'static str, Value, &'static str, Option<&'static str>)> {
    vec![
        (
            "a defaultSchema requiring a string property from an enumeration",
            json!({ "defaultSchema": r#"{"type":"object","properties":{"environment":{"type":"string","enum":["dev","prod"]},"critical":{"type":"boolean"}},"required":["environment"]}"# }),
            "CUSTOM",
            Some("environment"),
        ),
        (
            "an override for the record's OWN type requiring an integer property",
            json!({
                "defaultSchema": r#"{"type":"object"}"#,
                "recordTypeSchemaOverrides": [
                    { "recordType": "MCP", "schema": r#"{"type":"object","properties":{"tier":{"type":"integer"}},"required":["tier"]}"# }
                ]
            }),
            "MCP",
            Some("tier"),
        ),
        (
            "an override for ANOTHER record type, leaving this one the common attributes",
            json!({
                "defaultSchema": r#"{"type":"object"}"#,
                "recordTypeSchemaOverrides": [
                    { "recordType": "CUSTOM", "schema": r#"{"type":"object","properties":{"environment":{"type":"string"}},"required":["environment"]}"# }
                ]
            }),
            "AGENT",
            None,
        ),
        (
            "a defaultSchema requiring a boolean property",
            json!({ "defaultSchema": r#"{"type":"object","properties":{"critical":{"type":"boolean"}},"required":["critical"]}"# }),
            "SKILL",
            Some("critical"),
        ),
        (
            "a defaultSchema requiring a property the issue's schema never named",
            json!({ "defaultSchema": r#"{"type":"object","properties":{"ownerEmail":{"type":"string"}},"required":["ownerEmail"]}"# }),
            "AGENT",
            Some("ownerEmail"),
        ),
        (
            "a defaultSchema that declares properties but requires none",
            json!({ "defaultSchema": r#"{"type":"object","properties":{"owner":{"type":"string"}}}"# }),
            "GATEWAY",
            None,
        ),
        (
            "a registry that declares no schema at all",
            Value::Null,
            "CUSTOM",
            None,
        ),
    ]
}

#[tokio::test]
async fn a_create_carrying_no_metadata_is_never_refused_for_the_registrys_schema() {
    let router = router();

    // `customMetadata` is NOT a required input member, so a create that carries none asserts
    // nothing about a record's metadata: whatever the registry's schema requires, the create
    // is accepted and the record settles in DRAFT. Whether the record satisfies the schema
    // that applies to its record type is then a verdict every read reports, and the gate the
    // submission that would publish it has to pass — so the refusal lives on the submission,
    // not on the create. Every registry here auto-approves submissions, so an accepted
    // submission lands APPROVED and the gate is the only thing that can hold one back.
    for (at, (case, configuration, record_type, required)) in
        absent_metadata_table().into_iter().enumerate()
    {
        let mut registry_body = json!({
            "name": format!("absent-metadata-registry-{at}"),
            "approvalConfiguration": { "autoApprovalRules": ["APPROVE_ALL"] }
        });
        if !configuration.is_null() {
            registry_body["customMetadataSchemaConfiguration"] = configuration;
        }
        let registry_id = ready_registry_with(&router, registry_body).await;

        let created = create_record(
            &router,
            &registry_id,
            json!({
                "name": format!("absent-metadata-{at}"),
                "recordType": record_type,
                "descriptors": descriptors_for(record_type)
            }),
        )
        .await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: a create carrying no customMetadata is accepted, got {} {:?}",
            created.status, created.body
        );
        let record_id = id_of(created.str_field("recordArn")).to_string();
        let draft = record_at_status(&router, &registry_id, &record_id, "DRAFT", case).await;

        // The verdict the record's absent metadata earns, read back from the record itself
        // and from the summary of the page it is listed on.
        let expected_compliance = if required.is_some() {
            "NON_COMPLIANT"
        } else {
            "COMPLIANT"
        };
        assert_eq!(
            draft.field("customMetadataSchemaComplianceStatus"),
            &json!(expected_compliance),
            "case {case:?}: the created record reads back {expected_compliance}, got {:?}",
            draft.body
        );
        let listed = list_records(&router, &registry_id, json!({})).await;
        assert_eq!(
            &listed.field("registryRecords")[0]["customMetadataSchemaComplianceStatus"],
            &json!(expected_compliance),
            "case {case:?}: the listed summary carries the same verdict, got {:?}",
            listed.body
        );
        // A record exists to read a verdict from, so the create wrote one.
        assert_eq!(
            draft.str_field("recordArn"),
            created.str_field("recordArn"),
            "case {case:?}: the record the create answered is the one that was stored, got {:?}",
            draft.body
        );

        let submitted = submit_record(&router, &registry_id, &record_id).await;
        match required {
            Some(property) => {
                assert_eq!(
                    submitted.status, 400,
                    "case {case:?}: submitting a record the schema does not admit is refused, \
                     got {} {:?}",
                    submitted.status, submitted.body
                );
                assert_eq!(
                    submitted.error_code.as_deref(),
                    Some("ValidationException"),
                    "case {case:?}: the modelled error code is ValidationException, got {:?}",
                    submitted.error_code
                );
                assert_eq!(
                    submitted.str_field("message"),
                    schema_refusal(
                        record_type,
                        &format!(": required property '{property}' not found")
                    ),
                    "case {case:?}: the refusal names the record type and the required \
                     property, got {:?}",
                    submitted.body
                );
                let still_draft = get_record(&router, &registry_id, &record_id).await;
                assert_eq!(
                    still_draft.str_field("status"),
                    "DRAFT",
                    "case {case:?}: a refused submission leaves the record in DRAFT, got {:?}",
                    still_draft.body
                );
            }
            None => {
                assert_eq!(
                    submitted.status, 202,
                    "case {case:?}: a record nothing holds back is submitted, got {} {:?}",
                    submitted.status, submitted.body
                );
                assert_eq!(
                    submitted.str_field("status"),
                    "APPROVED",
                    "case {case:?}: the registry auto-approves what it admits, got {:?}",
                    submitted.body
                );
            }
        }
    }
}

#[tokio::test]
async fn get_registry_reports_the_effective_discovery_and_approval_configuration() {
    let router = router();

    // Rows differ along which configuration block the request carries and how fully: a
    // request that names neither, one that names an empty block of each, and ones that
    // name a value for each. The authorizer types are the model's own catalogue
    // (`smithy_query(query="shape", shape="RegistryAuthorizerType")`: AWS_IAM |
    // CUSTOM_JWT) and the rule is the model's one `AutoApprovalRule`, APPROVE_ALL.
    let cases: Vec<(&str, Value, Value, Value)> = vec![
        (
            "neither block named",
            json!({ "name": "effective-none" }),
            json!({ "authorizerType": "AWS_IAM" }),
            json!({ "autoApprovalRules": [] }),
        ),
        (
            "an empty block of each",
            json!({
                "name": "effective-empty",
                "discoveryConfiguration": {},
                "approvalConfiguration": {}
            }),
            json!({ "authorizerType": "AWS_IAM" }),
            json!({ "autoApprovalRules": [] }),
        ),
        (
            "an explicitly empty rule list",
            json!({
                "name": "effective-manual-review",
                "approvalConfiguration": { "autoApprovalRules": [] }
            }),
            json!({ "authorizerType": "AWS_IAM" }),
            json!({ "autoApprovalRules": [] }),
        ),
        (
            "the IAM authorizer and an auto-approval rule",
            json!({
                "name": "effective-auto-approval",
                "discoveryConfiguration": { "authorizerType": "AWS_IAM" },
                "approvalConfiguration": { "autoApprovalRules": ["APPROVE_ALL"] }
            }),
            json!({ "authorizerType": "AWS_IAM" }),
            json!({ "autoApprovalRules": ["APPROVE_ALL"] }),
        ),
        (
            "a custom JWT authorizer",
            json!({
                "name": "effective-custom-jwt",
                "discoveryConfiguration": {
                    "authorizerType": "CUSTOM_JWT",
                    "authorizerConfiguration": {
                        "customJWTAuthorizer": {
                            "discoveryUrl": "https://idp.example.test/.well-known/openid-configuration",
                            "allowedAudience": ["registry-consumers"]
                        }
                    }
                }
            }),
            json!({
                "authorizerType": "CUSTOM_JWT",
                "authorizerConfiguration": {
                    "customJWTAuthorizer": {
                        "discoveryUrl": "https://idp.example.test/.well-known/openid-configuration",
                        "allowedAudience": ["registry-consumers"]
                    }
                }
            }),
            json!({ "autoApprovalRules": [] }),
        ),
    ];

    for (case, body, discovery, approval) in cases {
        let registry_id = ready_registry_with(&router, body).await;
        let got = get_registry(&router, &registry_id).await;
        assert_eq!(
            got.field("discoveryConfiguration"),
            &discovery,
            "case {case:?}: GetRegistry reports the effective discoveryConfiguration, got {:?}",
            got.field("discoveryConfiguration")
        );
        assert_eq!(
            got.field("approvalConfiguration"),
            &approval,
            "case {case:?}: GetRegistry reports the effective approvalConfiguration, got {:?}",
            got.field("approvalConfiguration")
        );
    }
}

// ---------------------------------------------------------------------------------------
// The publisher slice: a publisher of a domain, and the association that binds a registry
// record to one. Same rule as above — every answer is computed from what the writes
// persisted — over the resources the publisher surface owns.
// ---------------------------------------------------------------------------------------

/// The DNS label a domain's challenge record sits at, as `DomainChallenge$recordName`
/// documents it (`smithy_query(query="shape", shape="DomainChallenge")`).
const CHALLENGE_LABEL: &str = "_acme-challenge";

/// The ARN of a publisher: `PublisherIdentifierArn` is the ARN of a GLOBAL resource, so the
/// region field is empty and the resource segment carries the domain.
fn publisher_arn(domain: &str) -> String {
    engine::emulator_arn("agent-registry", "", domain)
}

async fn create_publisher(router: &MultiServiceRouter, body: Value) -> Reply {
    call(router, "POST", "/publishers", Some(body)).await
}

async fn get_publisher(router: &MultiServiceRouter, arn: &str) -> Reply {
    call(
        router,
        "GET",
        &format!("/publishers/{}", encode_label(arn)),
        None,
    )
    .await
}

async fn delete_publisher(router: &MultiServiceRouter, arn: &str) -> Reply {
    call(
        router,
        "DELETE",
        &format!("/publishers/{}", encode_label(arn)),
        None,
    )
    .await
}

async fn list_publishers(router: &MultiServiceRouter) -> Vec<Value> {
    let listed = call(router, "GET", "/publishers", None).await;
    assert_eq!(
        listed.status, 200,
        "ListPublishers must answer 200, got {} {:?}",
        listed.status, listed.body
    );
    listed.body["items"].as_array().cloned().unwrap_or_default()
}

async fn associate_publisher(router: &MultiServiceRouter, body: Value) -> Reply {
    call(router, "POST", "/associations", Some(body)).await
}

/// The associations `ListPublisherAssociations` reports. The `recordArn` label of the
/// association routes is GREEDY, so the ARN goes into the path as it stands.
async fn list_associations(router: &MultiServiceRouter) -> Vec<Value> {
    let listed = call(router, "GET", "/associations", None).await;
    assert_eq!(
        listed.status, 200,
        "ListPublisherAssociations must answer 200, got {} {:?}",
        listed.status, listed.body
    );
    listed.body["items"].as_array().cloned().unwrap_or_default()
}

/// Poll `GetPublisher` until the publisher reports `status`, and answer the reply that did.
/// Panics, naming the case and the last status seen, if the deadline passes first.
async fn publisher_at_status(
    router: &MultiServiceRouter,
    arn: &str,
    status: &str,
    case: &str,
) -> Reply {
    let deadline = tokio::time::Instant::now() + STATUS_DEADLINE;
    loop {
        let got = get_publisher(router, arn).await;
        if got.str_field("status") == status {
            return got;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case:?}: the publisher never reported {status}, last answer {} {:?}",
            got.status,
            got.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Create a publisher for `domain` and wait for the domain verification the clock owes, so
/// it is `ACTIVE` and accepts associations. Returns its ARN.
async fn active_publisher(router: &MultiServiceRouter, domain: &str) -> String {
    let created = create_publisher(router, json!({ "value": { "domain": domain } })).await;
    assert_eq!(
        created.status, 201,
        "case {domain}: CreatePublisher must answer 201, got {} {:?}",
        created.status, created.body
    );
    let arn = created.str_field("publisherArn").to_string();
    publisher_at_status(router, &arn, "ACTIVE", domain).await;
    arn
}

/// Create a record in a registry that approves every submission and take it through that
/// approval, so it is the `APPROVED` record an association needs. Returns its ARN.
async fn approved_record(
    router: &MultiServiceRouter,
    registry_id: &str,
    name: &str,
    record_type: &str,
) -> String {
    let created = create_record(
        router,
        registry_id,
        json!({
            "name": name,
            "recordType": record_type,
            "descriptors": descriptors_for(record_type),
        }),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case {name}: CreateRegistryRecord must answer 202, got {} {:?}",
        created.status, created.body
    );
    let record_arn = created.str_field("recordArn").to_string();
    let record_id = id_of(&record_arn).to_string();
    record_at_status(router, registry_id, &record_id, "DRAFT", name).await;
    let submitted = submit_record(router, registry_id, &record_id).await;
    assert_eq!(
        submitted.status, 202,
        "case {name}: SubmitRegistryRecordForApproval must answer 202, got {} {:?}",
        submitted.status, submitted.body
    );
    record_at_status(router, registry_id, &record_id, "APPROVED", name).await;
    record_arn
}

/// A registry whose approval configuration approves every submitted record, so a record can
/// be brought to `APPROVED` without a curator. `APPROVE_ALL` is the one `AutoApprovalRule`
/// the model declares (`smithy_query(query="shape", shape="ApprovalConfiguration")`).
async fn registry_approving_every_record(router: &MultiServiceRouter, name: &str) -> String {
    ready_registry_with(
        router,
        json!({
            "name": name,
            "approvalConfiguration": { "autoApprovalRules": ["APPROVE_ALL"] }
        }),
    )
    .await
}

#[tokio::test]
async fn create_publisher_persists_the_domain_and_its_challenge_and_the_reads_report_them() {
    // Rows differ along the domain's shape and along whether the create carried tags, so
    // what the reads report is that row's own stored publisher.
    let cases: Vec<(&str, &str, Option<Value>)> = vec![
        ("a two-label domain", "acme.example", None),
        (
            "a domain with tags",
            "tools.acme.example",
            Some(json!({ "team": "catalog", "tier": "gold" })),
        ),
        ("a four-label domain", "eu.west.acme.example", None),
        ("a hyphenated label", "a-very-long-label.example", None),
    ];

    for (case, domain, tags) in cases {
        let router = router();
        let mut body = json!({ "value": { "domain": domain } });
        if let Some(tags) = tags.clone() {
            body["tags"] = tags;
        }
        let created = create_publisher(&router, body).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: CreatePublisher must answer 201, got {} {:?}",
            created.status, created.body
        );
        // "The publisher starts in ACTION_REQUIRED, and the response includes a
        // domainChallenge" whose record the caller publishes to prove ownership.
        assert_eq!(
            (
                created.str_field("status"),
                created.str_field("identifierType"),
                created.field("value")["domain"].as_str().unwrap_or_default(),
            ),
            ("ACTION_REQUIRED", "DOMAIN", domain),
            "case {case:?}: a created publisher waits for its domain and reports the identifier \
             the create named, got {:?}",
            created.body
        );
        assert_eq!(
            created.field("domainChallenge")["recordName"]
                .as_str()
                .unwrap_or_default(),
            format!("{CHALLENGE_LABEL}.{domain}"),
            "case {case:?}: the challenge names the TXT record on the publisher's own domain, \
             got {:?}",
            created.body
        );
        assert!(
            !created.field("domainChallenge")["recordValue"]
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "case {case:?}: the challenge carries the value to publish, got {:?}",
            created.body
        );
        let arn = created.str_field("publisherArn").to_string();
        assert_eq!(
            arn,
            publisher_arn(domain),
            "case {case:?}: a publisher's ARN is region-less and carries the domain, got {arn:?}"
        );

        let got = get_publisher(&router, &arn).await;
        assert_eq!(
            (
                got.status,
                got.field("value").clone(),
                got.field("createdAt").clone(),
                got.field("tags").clone(),
                got.field("domainChallenge").clone(),
            ),
            (
                200,
                created.field("value").clone(),
                created.field("createdAt").clone(),
                created.field("tags").clone(),
                created.field("domainChallenge").clone(),
            ),
            "case {case:?}: GetPublisher reports the publisher the create stored, got {} {:?}",
            got.status,
            got.body
        );
        assert_eq!(
            got.field("tags").clone(),
            tags.clone().unwrap_or(Value::Null),
            "case {case:?}: a publisher reports the tags its create attached and none otherwise, \
             got {:?}",
            got.body
        );

        // The verification is the service's own work, owed by the clock: the publisher
        // becomes ACTIVE without the caller doing anything further.
        publisher_at_status(&router, &arn, "ACTIVE", case).await;
        let items = list_publishers(&router).await;
        assert_eq!(
            items.len(),
            1,
            "case {case:?}: the account's one publisher is listed once, got {items:?}"
        );
        assert_eq!(
            (
                items[0]["publisherArn"].as_str().unwrap_or_default(),
                items[0]["status"].as_str().unwrap_or_default(),
                items[0]["associationCount"].as_i64().unwrap_or(-1),
                items[0]["pendingAssociationApprovalCount"]
                    .as_i64()
                    .unwrap_or(-1),
            ),
            (arn.as_str(), "ACTIVE", 0, 0),
            "case {case:?}: the summary reports the publisher as it stands and counts the \
             associations it holds, got {items:?}"
        );

        // A second publisher for the same domain is refused however the domain is cased: a
        // DNS name is case-insensitive, so both name the one domain this account publishes.
        for cased in [domain.to_uppercase(), domain.to_string()] {
            let duplicate =
                create_publisher(&router, json!({ "value": { "domain": cased } })).await;
            assert_eq!(
                (duplicate.status, duplicate.error_code.as_deref()),
                (409, Some("ConflictException")),
                "case {case:?}: a second publisher for {cased:?} is a conflict, got {} {:?}",
                duplicate.status,
                duplicate.body
            );
        }

        // The delete leaves the publisher in DELETING and the clock owes its removal.
        let deleted = delete_publisher(&router, &arn).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: DeletePublisher must answer 204, got {} {:?}",
            deleted.status, deleted.body
        );
        let deadline = tokio::time::Instant::now() + STATUS_DEADLINE;
        loop {
            let polled = get_publisher(&router, &arn).await;
            if polled.status == 404 {
                assert_eq!(
                    polled.error_code.as_deref(),
                    Some("ResourceNotFoundException"),
                    "case {case:?}: a removed publisher is reported as not found, got {:?}",
                    polled.body
                );
                break;
            }
            assert_eq!(
                polled.str_field("status"),
                "DELETING",
                "case {case:?}: a publisher being deleted reports DELETING until it is gone, \
                 got {} {:?}",
                polled.status,
                polled.body
            );
            assert!(
                tokio::time::Instant::now() < deadline,
                "case {case:?}: a deleted publisher is removed, last answer {} {:?}",
                polled.status,
                polled.body
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
        assert!(
            list_publishers(&router).await.is_empty(),
            "case {case:?}: a removed publisher is listed no more"
        );
    }
}

/// One row of the public-identifier table: the association to make, and the identifier the
/// model's layout gives it.
struct IdentifierCase {
    case: &'static str,
    name: &'static str,
    record_type: &'static str,
    subdomain: Option<&'static str>,
    categories: &'static [&'static str],
    expected: String,
}

#[tokio::test]
async fn associate_publisher_mints_the_public_identifier_in_the_layout_the_model_states() {
    let router = router();
    let registry_id = registry_approving_every_record(&router, "publisher-identifiers").await;
    let domain = "acme.example";
    let publisher = active_publisher(&router, domain).await;

    // The layout and the kind each `RecordType` contributes are the ones `PublicIdentifier`
    // states (`smithy_query(query="doc", shape="PublicIdentifier")`):
    // `urn:air:[<subdomain>.]<domain>:<namespace>:<identity>[;<kind>]`, with `MCP` -> `mcp`,
    // `AGENT` and `CUSTOM` -> `agent`, `SKILL` -> `skill`, `WEB_BOT` -> `webbot`, and no kind
    // for `GATEWAY`. The rows differ along the subdomain, the number of categories and the
    // record type, which is everything the layout varies over.
    let cases: Vec<IdentifierCase> = vec![
        IdentifierCase {
            case: "no subdomain and no categories",
            name: "weather",
            record_type: "MCP",
            subdomain: None,
            categories: &[],
            expected: format!("urn:air:{domain}:weather;mcp"),
        },
        IdentifierCase {
            case: "a subdomain and two categories",
            name: "finance-agent",
            record_type: "AGENT",
            subdomain: Some("agents"),
            categories: &["financial", "money"],
            expected: format!("urn:air:agents.{domain}:financial:money:finance-agent;agent"),
        },
        IdentifierCase {
            case: "a record type that carries no kind",
            name: "edge-gateway",
            record_type: "GATEWAY",
            subdomain: None,
            categories: &["tools"],
            expected: format!("urn:air:{domain}:tools:edge-gateway"),
        },
        IdentifierCase {
            case: "a custom record carries the agent kind",
            name: "bespoke.thing",
            record_type: "CUSTOM",
            subdomain: None,
            categories: &[],
            expected: format!("urn:air:{domain}:bespoke.thing;agent"),
        },
        IdentifierCase {
            case: "a skill under a subdomain",
            name: "pdf-skill",
            record_type: "SKILL",
            subdomain: Some("skills"),
            categories: &[],
            expected: format!("urn:air:skills.{domain}:pdf-skill;skill"),
        },
        IdentifierCase {
            case: "a web bot under a deep namespace",
            name: "crawler",
            record_type: "WEB_BOT",
            subdomain: None,
            categories: &["search", "crawlers", "public"],
            expected: format!("urn:air:{domain}:search:crawlers:public:crawler;webbot"),
        },
    ];

    let mut minted: Vec<(String, String)> = Vec::new();
    for IdentifierCase {
        case,
        name,
        record_type,
        subdomain,
        categories,
        expected,
    } in cases
    {
        let record_arn = approved_record(&router, &registry_id, name, record_type).await;
        let mut body = json!({ "recordArn": record_arn, "publisherArn": publisher });
        if let Some(subdomain) = subdomain {
            body["subdomain"] = json!(subdomain);
        }
        if !categories.is_empty() {
            body["categories"] = json!(categories);
        }
        let associated = associate_publisher(&router, body).await;
        assert_eq!(
            associated.status, 201,
            "case {case:?}: AssociatePublisher must answer 201, got {} {:?}",
            associated.status, associated.body
        );
        assert_eq!(
            associated.str_field("publicIdentifier"),
            expected,
            "case {case:?}: the public identifier follows the layout the model states, got {:?}",
            associated.body
        );
        assert_eq!(
            (
                associated.str_field("recordArn"),
                associated.str_field("publisher"),
                associated.str_field("status"),
            ),
            (record_arn.as_str(), publisher.as_str(), "PENDING_APPROVAL"),
            "case {case:?}: a new association names both resources and waits for a curator, \
             got {:?}",
            associated.body
        );
        minted.push((record_arn, expected));
    }

    let mut listed: Vec<(String, String)> = list_associations(&router)
        .await
        .iter()
        .map(|association| {
            (
                association["recordArn"].as_str().unwrap_or_default().into(),
                association["publicIdentifier"]
                    .as_str()
                    .unwrap_or_default()
                    .into(),
            )
        })
        .collect();
    listed.sort();
    let mut wanted = minted.clone();
    wanted.sort();
    assert_eq!(
        listed, wanted,
        "ListPublisherAssociations reports every association that was minted"
    );

    let items = list_publishers(&router).await;
    assert_eq!(
        (
            items[0]["associationCount"].as_i64().unwrap_or(-1),
            items[0]["pendingAssociationApprovalCount"]
                .as_i64()
                .unwrap_or(-1),
        ),
        (minted.len() as i64, minted.len() as i64),
        "the summary counts the associations the publisher holds and the ones still waiting \
         for a decision, got {items:?}"
    );
}

#[tokio::test]
async fn an_association_is_refused_unless_the_record_and_its_identifier_are_free() {
    let router = router();
    let registry_id = registry_approving_every_record(&router, "publisher-refusals").await;
    let publisher = active_publisher(&router, "acme.example").await;

    let draft = create_record(
        &router,
        &registry_id,
        json!({
            "name": "not-approved",
            "recordType": "CUSTOM",
            "descriptors": descriptors_for("CUSTOM"),
        }),
    )
    .await;
    let draft_arn = draft.str_field("recordArn").to_string();
    record_at_status(
        &router,
        &registry_id,
        id_of(&draft_arn),
        "DRAFT",
        "not-approved",
    )
    .await;

    let taken = approved_record(&router, &registry_id, "already-associated", "MCP").await;
    let free = approved_record(&router, &registry_id, "free-record", "AGENT").await;
    let first = associate_publisher(
        &router,
        json!({ "recordArn": taken, "publisherArn": publisher }),
    )
    .await;
    assert_eq!(
        first.status, 201,
        "the record is associated once before the refusals, got {} {:?}",
        first.status, first.body
    );

    // A second registry holding a record of the SAME name: the identifier minted for it
    // under the same publisher is the one the first record already holds.
    let other_registry = registry_approving_every_record(&router, "publisher-refusals-two").await;
    let namesake = approved_record(&router, &other_registry, "already-associated", "MCP").await;
    // An ARN of `RegistryRecordArn`'s shape that names no record of this account.
    let unknown_record = format!(
        "{}/record/bbbbbbbbbbbb",
        engine::emulator_arn("agent-registry", REGION, "registry/aaaaaaaaaaaa")
    );

    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a record that is not approved",
            json!({ "recordArn": draft_arn, "publisherArn": publisher }),
            409,
            "ConflictException",
        ),
        (
            "a record that already has an association",
            json!({ "recordArn": taken, "publisherArn": publisher }),
            409,
            "ConflictException",
        ),
        (
            "a public identifier another record holds",
            json!({ "recordArn": namesake, "publisherArn": publisher }),
            409,
            "ConflictException",
        ),
        (
            "a record ARN that names nothing",
            json!({ "recordArn": unknown_record, "publisherArn": publisher }),
            400,
            "ValidationException",
        ),
        (
            "a publisher ARN that names nothing",
            json!({ "recordArn": free, "publisherArn": publisher_arn("nowhere.example") }),
            400,
            "ValidationException",
        ),
    ];

    for (case, body, status, code) in cases {
        let refused = associate_publisher(&router, body).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (status, Some(code)),
            "case {case:?}: the association is refused with {code} at {status}, got {} {:?}",
            refused.status,
            refused.body
        );
        assert_eq!(
            list_associations(&router).await.len(),
            1,
            "case {case:?}: a refused association writes nothing"
        );
    }

    // A publisher holding an association cannot be deleted; once the record is
    // disassociated — which releases its public identifier — it can, and the identifier can
    // be minted again.
    let refused = delete_publisher(&router, &publisher).await;
    assert_eq!(
        (refused.status, refused.error_code.as_deref()),
        (409, Some("ConflictException")),
        "a publisher holding an association cannot be deleted, got {} {:?}",
        refused.status,
        refused.body
    );
    let identifier = first.str_field("publicIdentifier").to_string();
    for case in ["the association it holds", "no association at all"] {
        let released = call(&router, "DELETE", &format!("/associations/{taken}"), None).await;
        assert_eq!(
            released.status, 204,
            "case {case:?}: DisassociatePublisher must answer 204, got {} {:?}",
            released.status, released.body
        );
        assert!(
            list_associations(&router).await.is_empty(),
            "case {case:?}: the record's association is gone"
        );
    }
    let again = associate_publisher(
        &router,
        json!({ "recordArn": namesake, "publisherArn": publisher }),
    )
    .await;
    assert_eq!(
        (again.status, again.str_field("publicIdentifier")),
        (201, identifier.as_str()),
        "a released public identifier can be minted again, for the other record that bears \
         the name, got {} {:?}",
        again.status,
        again.body
    );
}

#[tokio::test]
async fn a_curators_decision_is_what_the_association_reports() {
    let router = router();
    let registry_id = registry_approving_every_record(&router, "publisher-decisions").await;
    let publisher = active_publisher(&router, "acme.example").await;
    let record_arn = approved_record(&router, &registry_id, "under-review", "MCP").await;
    let associated = associate_publisher(
        &router,
        json!({ "recordArn": record_arn, "publisherArn": publisher }),
    )
    .await;
    assert_eq!(
        (associated.status, associated.str_field("status")),
        (201, "PENDING_APPROVAL"),
        "a new association waits for a decision, got {} {:?}",
        associated.status,
        associated.body
    );

    // The rows walk the decisions: a verdict with a reason, the same verdict with another
    // reason (which updates the reason), the same verdict with an empty reason (which leaves
    // none to report), and the other verdict. `AssociationApprovalStatus` declares the two
    // values (`smithy_query(query="shape", shape="AssociationApprovalStatus")`).
    let cases: Vec<(&str, &str, &str, Option<&str>)> = vec![
        (
            "approved with a reason",
            "APPROVED",
            "meets the catalogue bar",
            Some("meets the catalogue bar"),
        ),
        (
            "the same decision with another reason",
            "APPROVED",
            "re-reviewed after an edit",
            Some("re-reviewed after an edit"),
        ),
        ("an empty reason reports none", "APPROVED", "", None),
        (
            "turned down",
            "REJECTED",
            "fails the catalogue bar",
            Some("fails the catalogue bar"),
        ),
    ];

    for (case, approval_status, reason, reported) in cases {
        let decided = call(
            &router,
            "PATCH",
            &format!("/associations/{record_arn}"),
            Some(json!({ "approvalStatus": approval_status, "approvalStatusReason": reason })),
        )
        .await;
        assert_eq!(
            decided.status, 200,
            "case {case:?}: UpdatePublisherAssociationApprovalStatus must answer 200, got {} {:?}",
            decided.status, decided.body
        );
        let held = list_associations(&router).await;
        assert_eq!(
            held.len(),
            1,
            "case {case:?}: the decision changed the association rather than adding one, got \
             {held:?}"
        );
        assert_eq!(
            (
                held[0]["status"].as_str().unwrap_or_default(),
                held[0]["statusReason"].as_str(),
            ),
            (approval_status, reported),
            "case {case:?}: the association reports the decision and the reason it was given, \
             got {held:?}"
        );
    }

    // A decided association is still held but no longer waiting for a decision.
    let items = list_publishers(&router).await;
    assert_eq!(
        (
            items[0]["associationCount"].as_i64().unwrap_or(-1),
            items[0]["pendingAssociationApprovalCount"]
                .as_i64()
                .unwrap_or(-1),
        ),
        (1, 0),
        "the summary separates the associations held from the ones pending, got {items:?}"
    );

    // "If the caller has no publisher association for the registry record, the request fails
    // with AccessDeniedException", and a verification restart is refused for a publisher
    // whose verification is settled — neither writes anything.
    let unknown_record = format!(
        "{}/record/bbbbbbbbbbbb",
        engine::emulator_arn("agent-registry", REGION, "registry/aaaaaaaaaaaa")
    );
    let undecidable = call(
        &router,
        "PATCH",
        &format!("/associations/{unknown_record}"),
        Some(json!({ "approvalStatus": "APPROVED", "approvalStatusReason": "" })),
    )
    .await;
    assert_eq!(
        (undecidable.status, undecidable.error_code.as_deref()),
        (403, Some("AccessDeniedException")),
        "a decision on an association this account does not hold is refused, got {} {:?}",
        undecidable.status,
        undecidable.body
    );

    let before = get_publisher(&router, &publisher).await;
    for (case, arn, status, code) in [
        (
            "a publisher whose domain is verified",
            publisher.clone(),
            409,
            "ConflictException",
        ),
        (
            "an ARN that names no publisher",
            publisher_arn("nowhere.example"),
            400,
            "ValidationException",
        ),
    ] {
        let refused = call(
            &router,
            "POST",
            &format!("/publishers/{}/verification", encode_label(&arn)),
            None,
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (status, Some(code)),
            "case {case:?}: ValidatePublisher is refused with {code} at {status}, got {} {:?}",
            refused.status,
            refused.body
        );
    }
    let after = get_publisher(&router, &publisher).await;
    assert_eq!(
        (
            after.str_field("status"),
            after.field("domainChallenge").clone(),
            after.field("updatedAt").clone(),
        ),
        (
            before.str_field("status"),
            before.field("domainChallenge").clone(),
            before.field("updatedAt").clone(),
        ),
        "a refused verification restart writes nothing, got {:?}",
        after.body
    );
}

// ---------------------------------------------------------------------------------------
// The skill package a registry record carries: stored by the write, reported by the read as
// the checksum of what was stored, and patched by the edit.
// ---------------------------------------------------------------------------------------

/// The SHA-256 checksum of `content`, "as 64 lowercase hexadecimal characters" — the form
/// `Sha256Hash` declares (`smithy_query(query="shape", shape="Sha256Hash")`).
fn artifact_checksum(content: &[u8]) -> String {
    hex::encode(sha2::Sha256::digest(content))
}

/// A package's bytes as a JSON request carries them: base64, which the AWS SDKs encode for
/// the caller.
fn artifact_content(content: &[u8]) -> Value {
    json!(aws_smithy_types::base64::encode(content))
}

/// One row of the skill-package table: the bytes to upload, the checksum the request states
/// for them (if any), and the status the write is refused with (if it is).
struct PackageCase {
    case: &'static str,
    content: Vec<u8>,
    stated: Option<String>,
    refused: Option<u16>,
}

#[tokio::test]
async fn a_stored_skill_package_is_reported_as_the_checksum_of_its_bytes() {
    let router = router();
    let registry_id = ready_registry(&router, "record-skill-packages").await;

    // Rows differ along the package's bytes and along whether the request stated the
    // checksum itself: the checksum a read reports is computed over the content either way,
    // and a checksum that is not the content's is refused.
    let cases: Vec<PackageCase> = vec![
        PackageCase {
            case: "a short archive, checksum stated",
            content: b"PK\x03\x04 a tiny skill".to_vec(),
            stated: Some(artifact_checksum(b"PK\x03\x04 a tiny skill")),
            refused: None,
        },
        PackageCase {
            case: "a short archive, no checksum stated",
            content: b"PK\x03\x04 another skill".to_vec(),
            stated: None,
            refused: None,
        },
        PackageCase {
            case: "bytes that are not text",
            content: (0_u8..=255).collect(),
            stated: None,
            refused: None,
        },
        PackageCase {
            case: "a single byte",
            content: vec![7],
            stated: None,
            refused: None,
        },
        PackageCase {
            case: "a checksum of other content",
            content: b"PK\x03\x04 a third skill".to_vec(),
            stated: Some(artifact_checksum(b"something else")),
            refused: Some(400),
        },
    ];

    for (
        index,
        PackageCase {
            case,
            content,
            stated,
            refused,
        },
    ) in cases.into_iter().enumerate()
    {
        let name = format!("packaged-skill-{index}");
        let mut artifact = json!({ "content": artifact_content(&content) });
        if let Some(stated) = stated {
            artifact["sha256"] = json!(stated);
        }
        let created = create_record(
            &router,
            &registry_id,
            json!({
                "name": name,
                "recordType": "SKILL",
                "descriptors": descriptors_for("SKILL"),
                "artifact": artifact,
            }),
        )
        .await;
        if let Some(status) = refused {
            assert_eq!(
                (created.status, created.error_code.as_deref()),
                (status, Some("ValidationException")),
                "case {case:?}: a package the request does not describe is refused, got {} {:?}",
                created.status,
                created.body
            );
            continue;
        }
        assert_eq!(
            created.status, 202,
            "case {case:?}: CreateRegistryRecord must answer 202, got {} {:?}",
            created.status, created.body
        );
        let record_id = id_of(created.str_field("recordArn")).to_string();
        let got = record_at_status(&router, &registry_id, &record_id, "DRAFT", case).await;
        assert_eq!(
            got.field("artifact")["sha256"].as_str().unwrap_or_default(),
            artifact_checksum(&content),
            "case {case:?}: the read reports the checksum of the package that was stored, got \
             {:?}",
            got.body
        );
        // "Responses never include the package bytes", and this emulator serves no location
        // the package could be downloaded from, so it claims no download URL either.
        assert_eq!(
            (
                got.field("artifact")["content"].clone(),
                got.field("artifact")["downloadUrl"].clone(),
            ),
            (Value::Null, Value::Null),
            "case {case:?}: the read carries neither the bytes nor a download location, got {:?}",
            got.body
        );
    }

    // A record uploaded with no package has none to report, and the patch wrapper the model
    // declares for it decides what an edit does: omitted leaves it, a wrapper carrying a
    // package replaces it, an empty wrapper removes it
    // (`smithy_query(query="shape", shape="UpdatedRegistryRecordArtifact")`).
    let created = create_record(
        &router,
        &registry_id,
        json!({
            "name": "edited-skill",
            "recordType": "SKILL",
            "descriptors": descriptors_for("SKILL"),
            "artifact": { "content": artifact_content(b"PK\x03\x04 the first package") },
        }),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "the record is created with a package, got {} {:?}",
        created.status, created.body
    );
    let record_id = id_of(created.str_field("recordArn")).to_string();
    let replacement = b"PK\x03\x04 the replacement package".to_vec();
    let edits: Vec<(&str, Value, Option<String>)> = vec![
        (
            "an edit that names no package",
            json!({ "displayName": { "optionalValue": "Edited skill" } }),
            Some(artifact_checksum(b"PK\x03\x04 the first package")),
        ),
        (
            "an edit that uploads another package",
            json!({ "artifact": { "optionalValue": { "content": artifact_content(&replacement) } } }),
            Some(artifact_checksum(&replacement)),
        ),
        ("an edit that removes the package", json!({ "artifact": {} }), None),
    ];

    for (case, body, expected) in edits {
        // An edit applies to a record whose own asynchronous change has finished, so each
        // row waits for the record to settle before it edits again.
        record_at_status(&router, &registry_id, &record_id, "DRAFT", case).await;
        let edited = patch_record(&router, &registry_id, &record_id, body).await;
        assert_eq!(
            edited.status, 202,
            "case {case:?}: UpdateRegistryRecord must answer 202, got {} {:?}",
            edited.status, edited.body
        );
        let got = get_record(&router, &registry_id, &record_id).await;
        match expected {
            Some(expected) => assert_eq!(
                got.field("artifact")["sha256"].as_str().unwrap_or_default(),
                expected,
                "case {case:?}: the record reports the package the edit left on it, got {:?}",
                got.body
            ),
            None => assert_eq!(
                got.field("artifact").clone(),
                Value::Null,
                "case {case:?}: the record reports no package once the edit removed it, got {:?}",
                got.body
            ),
        }
    }
}

