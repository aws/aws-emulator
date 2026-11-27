// SPDX-License-Identifier: CC0-1.0

//! Agent Registry custom metadata on the discovery surface, on the ASSEMBLED emulator —
//! every service mounted, as `aws_emulator_router::build_router` assembles the one the
//! binary serves, so the Agent Registry DATA plane answers beside the CONTROL plane over the
//! one shared store.
//!
//! THE RULE THIS SUITE PINS: `RegistryRecordSummary$customMetadata` "is only present if the
//! registry has a custom metadata schema configured", so a record's custom metadata reaches
//! the discovery surface only through a registry that declares a custom metadata schema.
//! What the record stores is not changed by that: a registry declaring no schema accepts
//! metadata on its records and the control plane keeps reporting it, while the discovery
//! reads that serve the summary carry no such member. The condition is the REGISTRY's
//! current configuration, so declaring a schema on a registry that had none publishes the
//! metadata its records already carried.
//!
//! The rows span what the rule generalises over: the scope the schema is declared in (the
//! common attributes, one record type's fields), a registry declaring none, and a record
//! that attaches no metadata at all. Nothing here seeds a store directly — every registry
//! and record is authored through the control plane's own operations and read back through
//! both planes, so neither plane is asserted against a copy of the other's state.

use std::sync::Arc;
use std::time::{Duration, Instant};

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call below signs for.
const REGION: &str = "us-west-2";

/// The access key id every call signs with: a key neither STS nor IAM holds a record for,
/// which is the position AWS account root credentials are in, so no identity-based policy
/// bounds these reads.
const ACCOUNT_KEY_ID: &str = "AKIDTESTAGENTREGISTRY";

/// How long a wait for a transition the clock owes may take before a suite gives up.
/// Generous on purpose: the wait is on the CONDITION, so a loaded host only polls more.
const TRANSITION_DEADLINE: Duration = Duration::from_secs(60);

/// How long a poll loop pauses between two looks at a resource.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The `RegistryStatus` a registry is usable in.
const REGISTRY_READY: &str = "READY";

/// The `RegistryRecordStatus` a created record settles at, and the one it is discoverable in.
const DRAFT_STATUS: &str = "DRAFT";
const DISCOVERABLE_STATUS: &str = "APPROVED";

/// A `CustomMetadataSchemaDefinition` ("A JSON Schema") declaring the fields the records
/// below carry as the registry's common attributes.
const COMMON_SCHEMA: &str = "{\"type\":\"object\",\"properties\":{\
     \"environment\":{\"type\":\"string\"},\
     \"requiresApproval\":{\"type\":\"boolean\"}}}";

/// A `CustomMetadataSchemaDefinition` declaring one record type's own field.
const AGENT_SCHEMA: &str = "{\"type\":\"object\",\"properties\":{\"tier\":{\"type\":\"string\"}}}";

/// One answer from the assembled emulator, reduced to what this suite asserts on.
struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    fn str_field(&self, name: &str) -> &str {
        self.body[name].as_str().unwrap_or_default()
    }

    /// The summary this answer carries for `record_id`, when it carries one.
    fn summary_of(&self, record_id: &str) -> Option<Value> {
        self.body["registryRecords"]
            .as_array()?
            .iter()
            .find(|record| record["recordId"].as_str() == Some(record_id))
            .cloned()
    }
}

/// The whole emulator, exactly as the binary assembles it, over one shared engine. Request
/// authentication is off: this suite is about what a request is ANSWERED with, and the
/// signature of a hand-built request is not checked anywhere in this target.
fn assembled_emulator() -> MultiServiceRouter {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let mut router = build_router(engine);
    router.set_request_authentication(false);
    router
}

async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Option<Value>) -> Reply {
    let payload = match &body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serializable body")),
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://agent-registry.{REGION}.amazonaws.com{path}"))
        .header("host", format!("agent-registry.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential={ACCOUNT_KEY_ID}/20260101/{REGION}/agent-registry/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(payload)
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Reply { status, body }
}

/// The last `/`-separated segment of an ARN: a registry id, or a record id.
fn id_of(arn: &str) -> &str {
    arn.rsplit('/').next().unwrap_or(arn)
}

// --- the control plane, as a client calls it ------------------------------------------

/// A registry whose `approvalConfiguration.autoApprovalRules` carry `APPROVE_ALL`, the one
/// rule the model declares, so a submission is approved rather than left for a curator, and
/// whose `customMetadataSchemaConfiguration` is the one the row states (omitted entirely for
/// a registry that declares no custom metadata schema).
async fn create_registry(
    router: &MultiServiceRouter,
    name: &str,
    configuration: Option<Value>,
) -> Reply {
    let mut body = json!({
        "name": name,
        "approvalConfiguration": { "autoApprovalRules": ["APPROVE_ALL"] },
    });
    if let Some(configuration) = configuration {
        body["customMetadataSchemaConfiguration"] = configuration;
    }
    call(router, "POST", "/registries", Some(body)).await
}

async fn get_registry(router: &MultiServiceRouter, registry_id: &str) -> Reply {
    call(router, "GET", &format!("/registries/{registry_id}"), None).await
}

async fn update_registry(router: &MultiServiceRouter, registry_id: &str, patch: Value) -> Reply {
    call(
        router,
        "PATCH",
        &format!("/registries/{registry_id}"),
        Some(patch),
    )
    .await
}

async fn create_record(
    router: &MultiServiceRouter,
    registry_id: &str,
    name: &str,
    record_type: &str,
    metadata: Option<Value>,
) -> Reply {
    let mut body = json!({
        "name": name,
        "recordType": record_type,
        "recordVersion": "1.0.0",
        "descriptors": { "custom": { "data": "{}" } },
    });
    if let Some(metadata) = metadata {
        body["customMetadata"] = metadata;
    }
    call(
        router,
        "POST",
        &format!("/registries/{registry_id}/records"),
        Some(body),
    )
    .await
}

async fn get_record(router: &MultiServiceRouter, registry_id: &str, record_id: &str) -> Reply {
    call(
        router,
        "GET",
        &format!("/registries/{registry_id}/records/{record_id}"),
        None,
    )
    .await
}

async fn submit_record(router: &MultiServiceRouter, registry_id: &str, record_id: &str) -> Reply {
    call(
        router,
        "POST",
        &format!("/registries/{registry_id}/records/{record_id}/submit-for-approval"),
        None,
    )
    .await
}

// --- the data plane's discovery reads --------------------------------------------------

async fn batch_get_discoverable(
    router: &MultiServiceRouter,
    registry_id: &str,
    record_id: &str,
) -> Reply {
    call(
        router,
        "POST",
        "/discoverable-records-batch",
        Some(json!({ "entries": [{ "registryId": registry_id, "recordIds": [record_id] }] })),
    )
    .await
}

async fn search_discoverable(router: &MultiServiceRouter, registry_id: &str) -> Reply {
    call(
        router,
        "POST",
        "/discoverable-records-search",
        Some(json!({ "searchQuery": "", "registryIds": [registry_id], "maxResults": 10 })),
    )
    .await
}

// --- waits on a transition the clock owes ---------------------------------------------

/// Look at a registry until it reports `REGISTRY_READY`, or give up at the deadline. The wait
/// is on the CONDITION — a transition the clock owes — never on a fixed span.
async fn registry_ready(router: &MultiServiceRouter, case: &str, registry_id: &str) {
    let started = Instant::now();
    loop {
        let reply = get_registry(router, registry_id).await;
        if reply.str_field("status") == REGISTRY_READY {
            return;
        }
        assert!(
            started.elapsed() < TRANSITION_DEADLINE,
            "case {case:?}: the registry never reached {REGISTRY_READY}, last answer {} {:?}",
            reply.status,
            reply.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Look at a record until the CONTROL plane reports `wanted`, or give up at the deadline.
async fn record_reaching(
    router: &MultiServiceRouter,
    case: &str,
    registry_id: &str,
    record_id: &str,
    wanted: &str,
) -> Reply {
    let started = Instant::now();
    loop {
        let reply = get_record(router, registry_id, record_id).await;
        if reply.str_field("status") == wanted {
            return reply;
        }
        assert!(
            started.elapsed() < TRANSITION_DEADLINE,
            "case {case:?}: the record never reached {wanted}, last answer {} {:?}",
            reply.status,
            reply.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One record carried to `APPROVED` through the control plane's own operations: a create, the
/// clock-owed `CREATING` -> `DRAFT`, and a submission the registry approves. Answers its id.
async fn approved_record(
    router: &MultiServiceRouter,
    case: &str,
    registry_id: &str,
    name: &str,
    record_type: &str,
    metadata: Option<Value>,
) -> String {
    let created = create_record(router, registry_id, name, record_type, metadata).await;
    assert_eq!(
        created.status, 202,
        "case {case:?}: CreateRegistryRecord answers 202, got {} {:?}",
        created.status, created.body
    );
    let record_id = id_of(created.str_field("recordArn")).to_string();
    record_reaching(router, case, registry_id, &record_id, DRAFT_STATUS).await;
    let submitted = submit_record(router, registry_id, &record_id).await;
    assert_eq!(
        (submitted.status, submitted.str_field("status")),
        (202, DISCOVERABLE_STATUS),
        "case {case:?}: a submission to an auto-approving registry approves the record, \
         got {} {:?}",
        submitted.status,
        submitted.body
    );
    record_id
}

/// What the discovery surface carries for `record_id` under `customMetadata`, read through the
/// batch read and through the search, which must answer identically: `Some(document)` for a
/// response carrying the member, `None` for one carrying no such member.
async fn published_metadata(
    router: &MultiServiceRouter,
    case: &str,
    registry_id: &str,
    record_id: &str,
) -> Option<Value> {
    let batch = batch_get_discoverable(router, registry_id, record_id).await;
    assert_eq!(
        batch.status, 200,
        "case {case:?}: BatchGetDiscoverableRegistryRecord answers 200, got {} {:?}",
        batch.status, batch.body
    );
    let from_batch = batch.summary_of(record_id).unwrap_or_else(|| {
        panic!(
            "case {case:?}: the approved record is on the discovery surface, got {:?}",
            batch.body
        )
    });

    let search = search_discoverable(router, registry_id).await;
    assert_eq!(
        search.status, 200,
        "case {case:?}: SearchDiscoverableRegistryRecords answers 200, got {} {:?}",
        search.status, search.body
    );
    let from_search = search.summary_of(record_id).unwrap_or_else(|| {
        panic!(
            "case {case:?}: the search carries the approved record, got {:?}",
            search.body
        )
    });
    assert_eq!(
        from_search, from_batch,
        "case {case:?}: a record renders identically in Search and BatchGet, \
         got {from_search:?} vs {from_batch:?}"
    );
    from_batch.get("customMetadata").cloned()
}

/// One row of the table: what the registry declares about custom metadata, what the record
/// attaches, and what each plane then reports for it.
struct Row {
    case: &'static str,
    /// The registry's name, unique within the account.
    registry: &'static str,
    /// The registry's `customMetadataSchemaConfiguration`, or `None` for a registry that
    /// declares no custom metadata schema.
    configuration: Option<Value>,
    /// The record's `recordType` — what a per-record-type schema override applies to.
    record_type: &'static str,
    /// The `customMetadata` the create attaches to the record.
    metadata: Option<Value>,
    /// The `customMetadata` the discovery surface then carries, or `None` for no member.
    published: Option<Value>,
}

#[tokio::test]
async fn discovery_publishes_custom_metadata_only_for_a_registry_declaring_a_schema() {
    let router = assembled_emulator();

    let rows: Vec<Row> = vec![
        Row {
            case: "a registry declaring the common attributes",
            registry: "metadata-common-attributes",
            configuration: Some(json!({ "defaultSchema": COMMON_SCHEMA })),
            record_type: "MCP",
            metadata: Some(json!({ "environment": "production", "requiresApproval": false })),
            published: Some(json!({ "environment": "production", "requiresApproval": false })),
        },
        Row {
            case: "a registry declaring one record type's own fields",
            registry: "metadata-record-type-fields",
            configuration: Some(json!({
                "recordTypeSchemaOverrides": [{ "recordType": "AGENT", "schema": AGENT_SCHEMA }],
            })),
            record_type: "AGENT",
            metadata: Some(json!({ "tier": "gold" })),
            published: Some(json!({ "tier": "gold" })),
        },
        Row {
            case: "a registry declaring no schema, over a record that attaches metadata",
            registry: "metadata-no-schema",
            configuration: None,
            record_type: "MCP",
            metadata: Some(json!({ "environment": "production" })),
            published: None,
        },
        Row {
            case: "a declaring registry whose record attaches no metadata",
            registry: "metadata-none-attached",
            configuration: Some(json!({ "defaultSchema": COMMON_SCHEMA })),
            record_type: "MCP",
            metadata: None,
            published: None,
        },
    ];

    for row in &rows {
        let Row {
            case,
            registry,
            configuration,
            record_type,
            metadata,
            published,
        } = row;
        let created = create_registry(&router, registry, configuration.clone()).await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: CreateRegistry answers 202, got {} {:?}",
            created.status, created.body
        );
        let registry_id = id_of(created.str_field("registryArn")).to_string();
        registry_ready(&router, case, &registry_id).await;

        let record_id = approved_record(
            &router,
            case,
            &registry_id,
            "metadata-record",
            record_type,
            metadata.clone(),
        )
        .await;

        // The control plane reports the metadata the record was created with, whatever its
        // registry declares: the record's own content is not what the condition is about.
        let held = get_record(&router, &registry_id, &record_id).await;
        assert_eq!(
            held.body.get("customMetadata").cloned(),
            metadata.clone(),
            "case {case:?}: the control plane reports the metadata the record carries, \
             got {:?}",
            held.body
        );

        assert_eq!(
            published_metadata(&router, case, &registry_id, &record_id).await,
            *published,
            "case {case:?}: the discovery surface publishes customMetadata only for a \
             registry declaring a custom metadata schema"
        );
    }
}

#[tokio::test]
async fn declaring_a_schema_on_a_registry_publishes_the_metadata_its_records_already_carried() {
    let router = assembled_emulator();
    let case = "a schema declared after the record was approved";

    // A registry declaring nothing about custom metadata, holding an approved record that
    // carries some: the surface publishes no `customMetadata` member for it.
    let created = create_registry(&router, "metadata-declared-later", None).await;
    assert_eq!(
        created.status, 202,
        "CreateRegistry answers 202, got {} {:?}",
        created.status, created.body
    );
    let registry_id = id_of(created.str_field("registryArn")).to_string();
    registry_ready(&router, case, &registry_id).await;
    let attached = json!({ "environment": "production", "requiresApproval": false });
    let record_id = approved_record(
        &router,
        case,
        &registry_id,
        "metadata-record",
        "MCP",
        Some(attached.clone()),
    )
    .await;
    assert_eq!(
        published_metadata(&router, case, &registry_id, &record_id).await,
        None,
        "case {case:?}: a registry declaring no schema publishes no customMetadata member"
    );

    // Declaring a schema on that registry is what puts its records' metadata on the surface:
    // the condition is the registry's configuration as it stands when the record is read, and
    // nothing of the record was touched.
    let updated = update_registry(
        &router,
        &registry_id,
        json!({
            "customMetadataSchemaConfiguration": {
                "optionalValue": { "defaultSchema": COMMON_SCHEMA },
            },
        }),
    )
    .await;
    assert_eq!(
        updated.status, 202,
        "case {case:?}: UpdateRegistry answers 202, got {} {:?}",
        updated.status, updated.body
    );
    registry_ready(&router, case, &registry_id).await;

    assert_eq!(
        published_metadata(&router, case, &registry_id, &record_id).await,
        Some(attached),
        "case {case:?}: declaring a schema publishes the metadata the record already carried"
    );
}
