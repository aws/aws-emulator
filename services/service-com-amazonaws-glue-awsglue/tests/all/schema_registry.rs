// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue schema registry slice: `CreateSchema` and the
//! `GetSchema` that reads back what it stored.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is the HTTP behaviour and not a direct call into a handler.
//!
//! The suites are tables: each row is one input along the axis the rule generalises over
//! (how the registry is addressed — defaulted, by name, by ARN, by both; which of the
//! three `DataFormat` values; which `Compatibility` mode, including none; whether a
//! `SchemaDefinition`, `Description` or `Tags` was sent; the casing of the schema name).
//! The recorded workflow's own case — a schema created in the default registry with
//! `Compatibility` `DISABLED` and an Avro definition — is one row among them.

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, RegistryEntry, SchemaEntry, DEFAULT_COMPATIBILITY,
    DEFAULT_REGISTRY_NAME, FIRST_SCHEMA_VERSION, MAX_REGISTRIES, MAX_SCHEMAS_PER_REGISTRY,
    MAX_STORED_SCHEMAS, NO_SCHEMA_VERSION, REGISTRY_STATUS_AVAILABLE, SCHEMA_STATUS_AVAILABLE,
    SCHEMA_VERSION_STATUS_AVAILABLE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, emulator_on, error_code, error_message, ok, ACCOUNT_ID};

/// A registry seeded straight into the store, so a row can address a registry other than
/// the default one, without this suite depending on `CreateRegistry` (which the
/// registries suite covers).
const SEEDED_REGISTRY: &str = "analytics-registry";

/// The Region every request from this harness is scoped to, which is the Region the
/// ARNs a response carries are minted for.
const REGION: &str = "us-east-1";

/// An Avro record schema: the shape the recorded workflow registers.
const AVRO_DEFINITION: &str = concat!(
    r#"{"type":"record","name":"Payment","namespace":"com.example","fields":"#,
    r#"[{"name":"id","type":"string"},{"name":"amount","type":"double"}]}"#
);

/// A JSON Schema document, for the `JSON` data format.
const JSON_DEFINITION: &str = concat!(
    r#"{"$schema":"http://json-schema.org/draft-07/schema#","type":"object","#,
    r#""properties":{"id":{"type":"string"}},"required":["id"]}"#
);

/// A protocol-buffer message, for the `PROTOBUF` data format.
const PROTOBUF_DEFINITION: &str =
    "syntax = \"proto3\"; package com.example; message Order { string id = 1; int64 lines = 2; }";

/// The ARN of registry `name` as this endpoint mints it.
fn registry_arn(name: &str) -> String {
    format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:registry/{name}")
}

/// The ARN of schema `schema` of registry `registry` as this endpoint mints it.
fn schema_arn(registry: &str, schema: &str) -> String {
    format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:schema/{registry}/{schema}")
}

/// Whether `value` has the shape the modelled `SchemaVersionIdString` pins: 36 characters,
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`.
///
/// Source: `smithy_query shape=SchemaVersionIdString` — length 36..36 and that pattern.
fn has_version_id_shape(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8usize, 4, 4, 4, 12]
            .iter()
            .zip(groups.iter())
            .all(|(width, group)| {
                group.len() == *width
                    && group
                        .chars()
                        .all(|character| character.is_ascii_digit() || ('a'..='f').contains(&character))
            })
}

/// A router with [`SEEDED_REGISTRY`] already present, and the engine behind it.
fn emulator_with_seeded_registry() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        state.lock().registries.insert(
            SEEDED_REGISTRY.to_string(),
            RegistryEntry::new(
                SEEDED_REGISTRY,
                Some("seeded for the schema suite".to_string()),
                None,
                SystemTime::UNIX_EPOCH,
            ),
        );
    }
    (router, engine)
}

/// A schema set seeded straight into the store, holding no version: reaching a ceiling
/// over the wire would take thousands of requests and would exercise the transport rather
/// than the bound.
fn seeded_schema(registry_name: &str, name: &str) -> SchemaEntry {
    SchemaEntry {
        name: name.to_string(),
        registry_name: registry_name.to_string(),
        description: None,
        data_format: "AVRO".to_string(),
        compatibility: DEFAULT_COMPATIBILITY.to_string(),
        status: SCHEMA_STATUS_AVAILABLE.to_string(),
        checkpoint: NO_SCHEMA_VERSION,
        created_time: SystemTime::UNIX_EPOCH,
        updated_time: SystemTime::UNIX_EPOCH,
        tags: Default::default(),
        versions: Vec::new(),
        // Nothing has been registered against it, so no version number has been assigned.
        highest_version_number: NO_SCHEMA_VERSION,
    }
}

/// The string member `member` of `body`.
fn text(case: &str, body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: {member} must be a string, got {body}"))
        .to_string()
}

/// The integer member `member` of `body`.
fn number(case: &str, body: &Value, member: &str) -> i64 {
    body.get(member)
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {case}: {member} must be a number, got {body}"))
}

/// One row of the create-then-read table.
struct Case {
    /// What this row varies, named in every assertion.
    label: &'static str,
    /// The `CreateSchema` request body.
    body: Value,
    /// The registry the create must write into.
    registry: String,
    /// The schema name the create must store.
    schema: &'static str,
    /// The compatibility mode the create must record.
    compatibility: &'static str,
    /// The version number the create registers, or `None` when it carries no
    /// `SchemaDefinition` and so registers none.
    version: Option<i64>,
}

/// The table: one row per input along the axis the rule generalises over.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "no RegistryId, AVRO, no Compatibility",
            body: json!({
                "SchemaName": "payments-avro",
                "DataFormat": "AVRO",
                "SchemaDefinition": AVRO_DEFINITION,
            }),
            registry: DEFAULT_REGISTRY_NAME.to_string(),
            schema: "payments-avro",
            compatibility: DEFAULT_COMPATIBILITY,
            version: Some(FIRST_SCHEMA_VERSION),
        },
        Case {
            // The recorded workflow's own shape: the default registry, named outright,
            // with versioning switched off and a description and tags alongside.
            label: "RegistryName default-registry, JSON, DISABLED, described and tagged",
            body: json!({
                "RegistryId": { "RegistryName": DEFAULT_REGISTRY_NAME },
                "SchemaName": "events-json",
                "DataFormat": "JSON",
                "Compatibility": "DISABLED",
                "Description": "events emitted by the checkout service",
                "SchemaDefinition": JSON_DEFINITION,
                "Tags": { "team": "checkout", "tier": "gold" },
            }),
            registry: DEFAULT_REGISTRY_NAME.to_string(),
            schema: "events-json",
            compatibility: "DISABLED",
            version: Some(FIRST_SCHEMA_VERSION),
        },
        Case {
            label: "RegistryName of a registry that already exists, PROTOBUF, FULL_ALL",
            body: json!({
                "RegistryId": { "RegistryName": SEEDED_REGISTRY },
                "SchemaName": "orders-proto",
                "DataFormat": "PROTOBUF",
                "Compatibility": "FULL_ALL",
                "SchemaDefinition": PROTOBUF_DEFINITION,
            }),
            registry: SEEDED_REGISTRY.to_string(),
            schema: "orders-proto",
            compatibility: "FULL_ALL",
            version: Some(FIRST_SCHEMA_VERSION),
        },
        Case {
            label: "RegistryArn instead of RegistryName, AVRO, NONE",
            body: json!({
                "RegistryId": { "RegistryArn": registry_arn(SEEDED_REGISTRY) },
                "SchemaName": "shipments-avro",
                "DataFormat": "AVRO",
                "Compatibility": "NONE",
                "SchemaDefinition": AVRO_DEFINITION,
            }),
            registry: SEEDED_REGISTRY.to_string(),
            schema: "shipments-avro",
            compatibility: "NONE",
            version: Some(FIRST_SCHEMA_VERSION),
        },
        Case {
            label: "both RegistryId members, naming the same registry",
            body: json!({
                "RegistryId": {
                    "RegistryName": SEEDED_REGISTRY,
                    "RegistryArn": registry_arn(SEEDED_REGISTRY),
                },
                "SchemaName": "returns-json",
                "DataFormat": "JSON",
                "Compatibility": "BACKWARD_ALL",
                "SchemaDefinition": JSON_DEFINITION,
            }),
            registry: SEEDED_REGISTRY.to_string(),
            schema: "returns-json",
            compatibility: "BACKWARD_ALL",
            version: Some(FIRST_SCHEMA_VERSION),
        },
        Case {
            // `SchemaDefinition` is optional, so a schema set can exist holding no
            // version at all.
            label: "no SchemaDefinition, so no version is registered",
            body: json!({
                "SchemaName": "reserved.name$1",
                "DataFormat": "PROTOBUF",
                "Compatibility": "FORWARD",
            }),
            registry: DEFAULT_REGISTRY_NAME.to_string(),
            schema: "reserved.name$1",
            compatibility: "FORWARD",
            version: None,
        },
        Case {
            // A schema name is unique inside its registry, not across the account: the
            // same name in another registry is another schema.
            label: "a name another registry already holds",
            body: json!({
                "RegistryId": { "RegistryName": SEEDED_REGISTRY },
                "SchemaName": "payments-avro",
                "DataFormat": "AVRO",
                "Compatibility": "FULL",
                "SchemaDefinition": AVRO_DEFINITION,
            }),
            registry: SEEDED_REGISTRY.to_string(),
            schema: "payments-avro",
            compatibility: "FULL",
            version: Some(FIRST_SCHEMA_VERSION),
        },
        Case {
            // `SchemaRegistryNameString`'s pattern admits both cases and the registry
            // documents no fold, so this is a different schema from the first row's.
            label: "the first row's name in another casing",
            body: json!({
                "SchemaName": "Payments-Avro",
                "DataFormat": "AVRO",
                "Compatibility": "FORWARD_ALL",
                "SchemaDefinition": AVRO_DEFINITION,
            }),
            registry: DEFAULT_REGISTRY_NAME.to_string(),
            schema: "Payments-Avro",
            compatibility: "FORWARD_ALL",
            version: Some(FIRST_SCHEMA_VERSION),
        },
    ]
}

/// The member set `CreateSchema` and `GetSchema` both report for a schema set, which must
/// agree member for member: a create's answer is a read of what it just persisted.
const SHARED_MEMBERS: [&str; 10] = [
    "RegistryName",
    "RegistryArn",
    "SchemaName",
    "SchemaArn",
    "Description",
    "DataFormat",
    "Compatibility",
    "SchemaCheckpoint",
    "LatestSchemaVersion",
    "NextSchemaVersion",
];

// ---------------------------------------------------------------------------
// Create, then read back
// ---------------------------------------------------------------------------

/// What `CreateSchema` persists is what `GetSchema` reports: the members the request
/// carried round-trip, the ARNs are minted from the registry and schema names, and the
/// version counters are computed from the versions the create registered.
#[tokio::test]
async fn created_schemas_are_read_back_from_the_store() {
    let (router, _engine) = emulator_with_seeded_registry();

    for case in cases() {
        let label = case.label;
        let created = ok(&router, "CreateSchema", &case.body).await;

        assert_eq!(
            text(label, &created, "SchemaName"),
            case.schema,
            "case {label}: expected SchemaName {}, got {created}",
            case.schema
        );
        assert_eq!(
            text(label, &created, "RegistryName"),
            case.registry,
            "case {label}: expected RegistryName {}, got {created}",
            case.registry
        );
        assert_eq!(
            text(label, &created, "RegistryArn"),
            registry_arn(&case.registry),
            "case {label}: expected the registry's ARN, got {created}"
        );
        assert_eq!(
            text(label, &created, "SchemaArn"),
            schema_arn(&case.registry, case.schema),
            "case {label}: expected the schema's ARN, got {created}"
        );
        assert_eq!(
            text(label, &created, "Compatibility"),
            case.compatibility,
            "case {label}: expected Compatibility {}, got {created}",
            case.compatibility
        );
        assert_eq!(
            Some(text(label, &created, "DataFormat")),
            case.body
                .get("DataFormat")
                .and_then(Value::as_str)
                .map(str::to_string),
            "case {label}: DataFormat must round-trip, got {created}"
        );
        assert_eq!(
            created.get("Description"),
            case.body.get("Description"),
            "case {label}: Description must round-trip, got {created}"
        );
        assert_eq!(
            text(label, &created, "SchemaStatus"),
            SCHEMA_STATUS_AVAILABLE,
            "case {label}: a created schema set is readable at once, got {created}"
        );
        // Definite and empty for a create that carried no tag.
        let expected_tags = case
            .body
            .get("Tags")
            .cloned()
            .unwrap_or_else(|| json!({}));
        assert_eq!(
            created.get("Tags"),
            Some(&expected_tags),
            "case {label}: expected Tags {expected_tags}, got {created}"
        );

        match case.version {
            Some(version) => {
                assert_eq!(
                    number(label, &created, "LatestSchemaVersion"),
                    version,
                    "case {label}: expected the first version to be registered, got {created}"
                );
                assert_eq!(
                    number(label, &created, "NextSchemaVersion"),
                    version + 1,
                    "case {label}: the next version is one past the latest, got {created}"
                );
                assert_eq!(
                    number(label, &created, "SchemaCheckpoint"),
                    version,
                    "case {label}: the checkpoint is set to the first version, got {created}"
                );
                let version_id = text(label, &created, "SchemaVersionId");
                assert!(
                    has_version_id_shape(&version_id),
                    "case {label}: expected a version-4 UUID SchemaVersionId, got {version_id:?}"
                );
                assert_eq!(
                    text(label, &created, "SchemaVersionStatus"),
                    SCHEMA_VERSION_STATUS_AVAILABLE,
                    "case {label}: a registered version is readable at once, got {created}"
                );
            }
            None => {
                assert_eq!(
                    number(label, &created, "LatestSchemaVersion"),
                    NO_SCHEMA_VERSION,
                    "case {label}: a schema set holding no version has no latest, got {created}"
                );
                assert_eq!(
                    number(label, &created, "NextSchemaVersion"),
                    FIRST_SCHEMA_VERSION,
                    "case {label}: the next version is the first one, got {created}"
                );
                assert_eq!(
                    number(label, &created, "SchemaCheckpoint"),
                    NO_SCHEMA_VERSION,
                    "case {label}: nothing was checkpointed, got {created}"
                );
                assert!(
                    created.get("SchemaVersionId").is_none(),
                    "case {label}: no version was registered, so there is no id: {created}"
                );
                assert!(
                    created.get("SchemaVersionStatus").is_none(),
                    "case {label}: no version was registered, so there is no status: {created}"
                );
            }
        }

        // Read it back both ways a `SchemaId` can address it: by name inside its registry,
        // and by the ARN the create reported.
        let by_name = ok(
            &router,
            "GetSchema",
            &json!({
                "SchemaId": { "SchemaName": case.schema, "RegistryName": case.registry },
            }),
        )
        .await;
        let by_arn = ok(
            &router,
            "GetSchema",
            &json!({
                "SchemaId": { "SchemaArn": schema_arn(&case.registry, case.schema) },
            }),
        )
        .await;
        assert_eq!(
            by_name, by_arn,
            "case {label}: a SchemaId by name and one by ARN must name the same schema"
        );
        for member in SHARED_MEMBERS {
            assert_eq!(
                by_name.get(member),
                created.get(member),
                "case {label}: GetSchema must report the stored {member}; expected {:?}, got {:?}",
                created.get(member),
                by_name.get(member)
            );
        }
        assert_eq!(
            text(label, &by_name, "SchemaStatus"),
            SCHEMA_STATUS_AVAILABLE,
            "case {label}: expected the stored status, got {by_name}"
        );
        let created_time = text(label, &by_name, "CreatedTime");
        assert_eq!(
            created_time,
            text(label, &by_name, "UpdatedTime"),
            "case {label}: nothing has updated the schema set, got {by_name}"
        );
        assert!(
            ::aws_smithy_types::DateTime::from_str(
                &created_time,
                ::aws_smithy_types::date_time::Format::DateTime,
            )
            .is_ok(),
            "case {label}: expected a date-time CreatedTime, got {created_time:?}"
        );
    }
}

/// A `GetSchema` reports and writes nothing: two reads of the same schema answer the same
/// thing, and the store holds exactly the records the creates made.
#[tokio::test]
async fn reading_a_schema_changes_nothing() {
    let (router, engine) = emulator_with_seeded_registry();
    ok(
        &router,
        "CreateSchema",
        &json!({
            "SchemaName": "idempotent-read",
            "DataFormat": "AVRO",
            "SchemaDefinition": AVRO_DEFINITION,
        }),
    )
    .await;

    let id = json!({ "SchemaId": { "SchemaName": "idempotent-read" } });
    let first = ok(&router, "GetSchema", &id).await;
    let second = ok(&router, "GetSchema", &id).await;
    assert_eq!(
        first, second,
        "case repeated read: a read must not change what a later caller sees"
    );

    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let store = state.lock();
    assert_eq!(
        store.registries.len(),
        2,
        "case repeated read: expected the seeded and the default registry, got {:?}",
        store.registries.keys().collect::<Vec<_>>()
    );
    let registry = store
        .registry(DEFAULT_REGISTRY_NAME)
        .expect("the default registry was materialised by the create");
    assert_eq!(
        registry.schemas.len(),
        1,
        "case repeated read: expected the one created schema, got {:?}",
        registry.schemas.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        registry.status, REGISTRY_STATUS_AVAILABLE,
        "case repeated read: expected an available registry, got {:?}",
        registry.status
    );
}

// ---------------------------------------------------------------------------
// The default registry, and the registries a create may not conjure
// ---------------------------------------------------------------------------

/// The default registry is materialised by the create that reaches it, once, whether the
/// request named it or defaulted to it — and no OTHER registry name is brought into
/// existence by a create: one this endpoint does not hold is a miss, and the refused create
/// leaves nothing behind.
#[tokio::test]
async fn only_the_default_registry_is_created_on_demand() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    // Nothing is seeded: a fresh store holds no registry at all.
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        assert!(
            state.lock().registries.is_empty(),
            "case fresh store: the default registry is materialised on demand, not seeded"
        );
    }

    let rows: Vec<(&str, Value)> = vec![
        (
            "defaulted",
            json!({ "SchemaName": "first", "DataFormat": "AVRO" }),
        ),
        (
            "named outright",
            json!({
                "RegistryId": { "RegistryName": DEFAULT_REGISTRY_NAME },
                "SchemaName": "second",
                "DataFormat": "JSON",
            }),
        ),
        (
            "named by ARN",
            json!({
                "RegistryId": { "RegistryArn": registry_arn(DEFAULT_REGISTRY_NAME) },
                "SchemaName": "third",
                "DataFormat": "PROTOBUF",
            }),
        ),
    ];
    for (label, body) in rows {
        let created = ok(&router, "CreateSchema", &body).await;
        assert_eq!(
            text(label, &created, "RegistryName"),
            DEFAULT_REGISTRY_NAME,
            "case {label}: expected the default registry, got {created}"
        );
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let store = state.lock();
        assert_eq!(
            store.registries.len(),
            1,
            "case {label}: the default registry is materialised once, got {:?}",
            store.registries.keys().collect::<Vec<_>>()
        );
    }

    // Every other name must already have been created.
    let misses: Vec<(&str, Value)> = vec![
        (
            "unknown RegistryName",
            json!({
                "RegistryId": { "RegistryName": "never-created" },
                "SchemaName": "orphan",
                "DataFormat": "AVRO",
            }),
        ),
        (
            "unknown RegistryArn",
            json!({
                "RegistryId": { "RegistryArn": registry_arn("never-created") },
                "SchemaName": "orphan",
                "DataFormat": "AVRO",
            }),
        ),
        (
            "RegistryArn of another account",
            json!({
                "RegistryId": {
                    "RegistryArn": format!("arn:aws:glue:{REGION}:111122223333:registry/elsewhere"),
                },
                "SchemaName": "orphan",
                "DataFormat": "AVRO",
            }),
        ),
        (
            "RegistryArn of another Region",
            json!({
                "RegistryId": {
                    "RegistryArn": format!("arn:aws:glue:eu-west-1:{ACCOUNT_ID}:registry/elsewhere"),
                },
                "SchemaName": "orphan",
                "DataFormat": "AVRO",
            }),
        ),
    ];
    for (label, body) in misses {
        let (status, refusal) = call(&router, "CreateSchema", &body).await;
        assert_eq!(
            status, 400,
            "case {label}: expected 400, got {status} with {refusal}"
        );
        assert!(
            error_code(&refusal).contains("EntityNotFoundException"),
            "case {label}: expected EntityNotFoundException, got {refusal}"
        );
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let store = state.lock();
        assert_eq!(
            store.registries.len(),
            1,
            "case {label}: a refused create must leave no registry behind, got {:?}",
            store.registries.keys().collect::<Vec<_>>()
        );
    }
}

/// A second create of a name a registry already holds is refused, and leaves the stored
/// schema exactly as the first create made it.
#[tokio::test]
async fn a_duplicate_schema_name_is_refused_per_registry() {
    let (router, _engine) = emulator_with_seeded_registry();
    let first = json!({
        "SchemaName": "twice",
        "DataFormat": "AVRO",
        "Compatibility": "BACKWARD",
        "SchemaDefinition": AVRO_DEFINITION,
    });
    let created = ok(&router, "CreateSchema", &first).await;

    for (label, body) in [
        ("the same request again", first.clone()),
        (
            "the same name with a different format",
            json!({
                "SchemaName": "twice",
                "DataFormat": "JSON",
                "SchemaDefinition": JSON_DEFINITION,
            }),
        ),
    ] {
        let (status, refusal) = call(&router, "CreateSchema", &body).await;
        assert_eq!(
            status, 400,
            "case {label}: expected 400, got {status} with {refusal}"
        );
        assert!(
            error_code(&refusal).contains("AlreadyExistsException"),
            "case {label}: expected AlreadyExistsException, got {refusal}"
        );
        assert_eq!(
            error_message(&refusal),
            "Schema already exists. SchemaName: twice",
            "case {label}: the refusal names the member the create addressed the schema \
             set by, got {refusal}"
        );
    }

    // The same name in ANOTHER registry is another schema, and the first is untouched.
    let elsewhere = ok(
        &router,
        "CreateSchema",
        &json!({
            "RegistryId": { "RegistryName": SEEDED_REGISTRY },
            "SchemaName": "twice",
            "DataFormat": "PROTOBUF",
            "SchemaDefinition": PROTOBUF_DEFINITION,
        }),
    )
    .await;
    assert_eq!(
        text("another registry", &elsewhere, "DataFormat"),
        "PROTOBUF",
        "case another registry: expected its own format, got {elsewhere}"
    );
    let reread = ok(
        &router,
        "GetSchema",
        &json!({ "SchemaId": { "SchemaName": "twice" } }),
    )
    .await;
    assert_eq!(
        reread.get("DataFormat"),
        created.get("DataFormat"),
        "case another registry: the first schema is untouched, got {reread}"
    );
}

// ---------------------------------------------------------------------------
// References that resolve to nothing
// ---------------------------------------------------------------------------

/// How a `SchemaId` fails: a wrapper carrying neither member, or an ARN that is not a Glue
/// schema ARN, or two members disagreeing, is a bad parameter; one that is well formed but
/// names nothing this endpoint holds is a miss.
#[tokio::test]
async fn an_unresolvable_schema_id_is_refused() {
    let (router, _engine) = emulator_with_seeded_registry();
    ok(
        &router,
        "CreateSchema",
        &json!({
            "SchemaName": "present",
            "DataFormat": "AVRO",
            "SchemaDefinition": AVRO_DEFINITION,
        }),
    )
    .await;

    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "neither SchemaArn nor SchemaName",
            json!({ "SchemaId": {} }),
            "InvalidInputException",
        ),
        (
            // The modelled `GlueResourceArn` pattern already refuses an ARN of another
            // service's namespace, so what reaches the handler is a Glue ARN that is not
            // a schema's: a truncated one carries no resource at all.
            "a truncated Glue ARN",
            json!({ "SchemaId": { "SchemaArn": "arn:aws:glue:" } }),
            "InvalidInputException",
        ),
        (
            "a Glue ARN of another resource type",
            json!({
                "SchemaId": {
                    "SchemaArn": format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:database/analytics"),
                },
            }),
            "InvalidInputException",
        ),
        (
            "a Glue registry ARN where a schema ARN belongs",
            json!({ "SchemaId": { "SchemaArn": registry_arn(DEFAULT_REGISTRY_NAME) } }),
            "InvalidInputException",
        ),
        (
            "a schema ARN naming no registry",
            json!({
                "SchemaId": {
                    "SchemaArn": format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:schema/present"),
                },
            }),
            "InvalidInputException",
        ),
        (
            "two members naming different schemas",
            json!({
                "SchemaId": {
                    "SchemaArn": schema_arn(DEFAULT_REGISTRY_NAME, "present"),
                    "SchemaName": "absent",
                },
            }),
            "InvalidInputException",
        ),
        (
            "a name no registry holds",
            json!({ "SchemaId": { "SchemaName": "absent" } }),
            "EntityNotFoundException",
        ),
        (
            "a name held by another registry",
            json!({ "SchemaId": { "SchemaName": "present", "RegistryName": SEEDED_REGISTRY } }),
            "EntityNotFoundException",
        ),
        (
            "a registry no account holds",
            json!({ "SchemaId": { "SchemaName": "present", "RegistryName": "never-created" } }),
            "EntityNotFoundException",
        ),
        (
            "a schema ARN of another account",
            json!({
                "SchemaId": {
                    "SchemaArn": format!(
                        "arn:aws:glue:{REGION}:111122223333:schema/{DEFAULT_REGISTRY_NAME}/present"
                    ),
                },
            }),
            "EntityNotFoundException",
        ),
        (
            "a schema ARN of another Region",
            json!({
                "SchemaId": {
                    "SchemaArn": format!(
                        "arn:aws:glue:eu-west-1:{ACCOUNT_ID}:schema/{DEFAULT_REGISTRY_NAME}/present"
                    ),
                },
            }),
            "EntityNotFoundException",
        ),
    ];
    for (label, body, expected) in rows {
        let (status, refusal) = call(&router, "GetSchema", &body).await;
        assert_eq!(
            status, 400,
            "case {label}: expected 400, got {status} with {refusal}"
        );
        assert!(
            error_code(&refusal).contains(expected),
            "case {label}: expected {expected}, got {refusal}"
        );
    }
}

/// The same for a `CreateSchema`'s `RegistryId`: a wrapper carrying neither member, and two
/// members naming different registries, are bad parameters rather than misses.
#[tokio::test]
async fn an_unresolvable_registry_id_is_refused() {
    let (router, _engine) = emulator_with_seeded_registry();

    let rows: Vec<(&str, Value)> = vec![
        (
            "neither RegistryArn nor RegistryName",
            json!({ "RegistryId": {}, "SchemaName": "orphan", "DataFormat": "AVRO" }),
        ),
        (
            // As above: the modelled pattern refuses another service's namespace before
            // the handler, so the handler's job is a Glue ARN that is not a registry's.
            "a truncated Glue ARN",
            json!({
                "RegistryId": { "RegistryArn": "arn:aws:glue:" },
                "SchemaName": "orphan",
                "DataFormat": "AVRO",
            }),
        ),
        (
            "a Glue ARN of another resource type",
            json!({
                "RegistryId": {
                    "RegistryArn": format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:database/analytics"),
                },
                "SchemaName": "orphan",
                "DataFormat": "AVRO",
            }),
        ),
        (
            "a Glue schema ARN where a registry ARN belongs",
            json!({
                "RegistryId": { "RegistryArn": schema_arn(SEEDED_REGISTRY, "orders-proto") },
                "SchemaName": "orphan",
                "DataFormat": "AVRO",
            }),
        ),
        (
            "two members naming different registries",
            json!({
                "RegistryId": {
                    "RegistryName": DEFAULT_REGISTRY_NAME,
                    "RegistryArn": registry_arn(SEEDED_REGISTRY),
                },
                "SchemaName": "orphan",
                "DataFormat": "AVRO",
            }),
        ),
    ];
    for (label, body) in rows {
        let (status, refusal) = call(&router, "CreateSchema", &body).await;
        assert_eq!(
            status, 400,
            "case {label}: expected 400, got {status} with {refusal}"
        );
        assert!(
            error_code(&refusal).contains("InvalidInputException"),
            "case {label}: expected InvalidInputException, got {refusal}"
        );
    }
}

// ---------------------------------------------------------------------------
// Capacity
// ---------------------------------------------------------------------------

/// The store holds a bounded number of schemas per registry and a bounded number of
/// registries: the create at each ceiling succeeds, the one past it is refused with the
/// modelled limit error, and freeing a slot lets a create succeed again — the ceiling is a
/// bound on what is stored, not a latch on how many creates were ever made.
#[tokio::test]
async fn a_create_past_a_schema_registry_ceiling_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    // Reaching a ceiling over the wire would take thousands of requests and would
    // exercise the transport rather than the bound.
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = state.lock();
        let mut registry = RegistryEntry::new(DEFAULT_REGISTRY_NAME, None, None, SystemTime::UNIX_EPOCH);
        for index in 0..MAX_SCHEMAS_PER_REGISTRY - 1 {
            let name = format!("seeded-{index}");
            registry
                .schemas
                .insert(name.clone(), seeded_schema(DEFAULT_REGISTRY_NAME, &name));
        }
        store
            .registries
            .insert(DEFAULT_REGISTRY_NAME.to_string(), registry);
    }

    let body = |name: &str| json!({ "SchemaName": name, "DataFormat": "AVRO" });
    ok(&router, "CreateSchema", &body("last-schema")).await;
    let (status, refusal) = call(&router, "CreateSchema", &body("one-too-many")).await;
    assert_eq!(
        status, 400,
        "case schema {} of {MAX_SCHEMAS_PER_REGISTRY}: expected 400, got {status} with {refusal}",
        MAX_SCHEMAS_PER_REGISTRY + 1
    );
    assert!(
        error_code(&refusal).contains("ResourceNumberLimitExceededException"),
        "case one schema too many: expected ResourceNumberLimitExceededException, got {refusal}"
    );
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = state.lock();
        store
            .registry_mut(DEFAULT_REGISTRY_NAME)
            .expect("the seeded registry")
            .schemas
            .remove("seeded-0");
    }
    ok(&router, "CreateSchema", &body("one-too-many")).await;

    // The registry ceiling, on a store filled to it with registries the default one is not
    // among: the create that would materialise it is refused.
    let fresh: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let fresh_router = emulator_on(fresh.clone());
    {
        let state = fresh.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = state.lock();
        for index in 0..MAX_REGISTRIES {
            let name = format!("seeded-registry-{index}");
            store.registries.insert(
                name.clone(),
                RegistryEntry::new(&name, None, None, SystemTime::UNIX_EPOCH),
            );
        }
    }
    let (status, refusal) = call(&fresh_router, "CreateSchema", &body("needs-a-registry")).await;
    assert_eq!(
        status, 400,
        "case registry {} of {MAX_REGISTRIES}: expected 400, got {status} with {refusal}",
        MAX_REGISTRIES + 1
    );
    assert!(
        error_code(&refusal).contains("ResourceNumberLimitExceededException"),
        "case one registry too many: expected ResourceNumberLimitExceededException, got {refusal}"
    );
    {
        let state = fresh.state::<ComAmazonawsGlueAwsglueState>();
        state.lock().registries.remove("seeded-registry-0");
    }
    ok(&fresh_router, "CreateSchema", &body("needs-a-registry")).await;
}

/// The store-wide schema ceiling, which the per-registry one alone would let a caller walk
/// past: a store already holding [`MAX_STORED_SCHEMAS`] spread over registries that each
/// have room of their own still refuses the next create.
#[tokio::test]
async fn a_create_past_the_store_wide_schema_ceiling_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = state.lock();
        let mut remaining = MAX_STORED_SCHEMAS;
        let mut index = 0;
        while remaining > 0 {
            let take = remaining.min(MAX_SCHEMAS_PER_REGISTRY);
            let registry_name = format!("full-{index}");
            let mut registry =
                RegistryEntry::new(&registry_name, None, None, SystemTime::UNIX_EPOCH);
            for schema in 0..take {
                let name = format!("seeded-{schema}");
                registry
                    .schemas
                    .insert(name.clone(), seeded_schema(&registry_name, &name));
            }
            store.registries.insert(registry_name, registry);
            remaining -= take;
            index += 1;
        }
        // The registry the create writes into has room of its own, so only the store-wide
        // bound can refuse it.
        store.registries.insert(
            DEFAULT_REGISTRY_NAME.to_string(),
            RegistryEntry::new(DEFAULT_REGISTRY_NAME, None, None, SystemTime::UNIX_EPOCH),
        );
    }

    let body = |name: &str| json!({ "SchemaName": name, "DataFormat": "AVRO" });
    let (status, refusal) = call(&router, "CreateSchema", &body("one-too-many")).await;
    assert_eq!(
        status, 400,
        "case schema {} of {MAX_STORED_SCHEMAS}: expected 400, got {status} with {refusal}",
        MAX_STORED_SCHEMAS + 1
    );
    assert!(
        error_code(&refusal).contains("ResourceNumberLimitExceededException"),
        "case one schema too many for the store: expected \
         ResourceNumberLimitExceededException, got {refusal}"
    );

    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = state.lock();
        store
            .registry_mut("full-0")
            .expect("the seeded registry")
            .schemas
            .remove("seeded-0");
    }
    ok(&router, "CreateSchema", &body("one-too-many")).await;
}

/// Two schemas never share a `SchemaVersionId`: each version's id is minted once, at
/// registration.
#[tokio::test]
async fn every_registered_version_gets_its_own_id() {
    let router = emulator();
    let mut seen: Vec<String> = Vec::new();
    for index in 0..8 {
        let name = format!("unique-{index}");
        let created = ok(
            &router,
            "CreateSchema",
            &json!({
                "SchemaName": name,
                "DataFormat": "AVRO",
                "SchemaDefinition": AVRO_DEFINITION,
            }),
        )
        .await;
        let id = text(&name, &created, "SchemaVersionId");
        assert!(
            !seen.contains(&id),
            "case {name}: expected a fresh SchemaVersionId, got the already-seen {id:?}"
        );
        seen.push(id);
    }
}
