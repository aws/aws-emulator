// SPDX-License-Identifier: CC0-1.0

//! What a metadata transfer job RUNS, end to end over HTTP with the emulator's
//! own Amazon S3 mounted beside this service.
//!
//! The rule every suite here asserts: a metadata transfer job READS the
//! metadata its source names and WRITES what that metadata describes into its
//! destination, then reports the run — `COMPLETED` with a `progress` count of
//! the entries it moved and a `reportUrl` for the report it wrote — and reports
//! the modelled failure state only when the source could not be read or the
//! destination could not be written.
//!
//! So the destination is read back through this service's OWN operations
//! (`GetComponentType`, `GetEntity`) and the objects through Amazon S3's own:
//! what the job reports as transferred is what those two really hold
//! afterwards. The rows differ along what the rule generalises over — which
//! ends the transfer names, what the document describes (component types,
//! entities, a child listed before its parent, property values of several data
//! types), how many entries it carries, and whether the destination already
//! holds them — never along a mere rename.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    report_bucket, METADATA_TRANSFER_JOB_ERROR_CODE, METADATA_TRANSFER_JOB_STATE_COMPLETED,
    METADATA_TRANSFER_JOB_STATE_ERROR, METADATA_TRANSFER_JOB_VALIDATION_ERROR_CODE,
    ROOT_PARENT_ENTITY_ID,
};

use crate::harness::{workspace_arn, Emulator, Resp, REGION};

/// The bucket every document below is staged in and exported to.
const BUCKET: &str = "twinmaker-metadata";

/// The workspace a document is imported into.
const DESTINATION: &str = "importWorkspace";

/// The component type an entity component a document describes instantiates: a
/// component names the component type it instantiates, so the destination holds
/// this one before such a document lands. It defines no property of its own, so
/// a component of it has exactly the properties the document declares.
const COMPONENT_HOLDER_TYPE: &str = "com.example.documentHolder";

/// How long a suite here waits for an outcome the clock owes before it gives
/// up.
///
/// Generous on purpose: the emulator's own span is a fraction of this, and what
/// the wait is for is the CONDITION (the job reaching a terminal state), so a
/// slow or loaded host makes the wait longer rather than the test fail.
const SETTLE_DEADLINE: Duration = Duration::from_secs(30);

/// How long the poll loop pauses between looks.
const POLL_PAUSE: Duration = Duration::from_millis(20);

/// An Amazon S3 client over this emulator's own in-process callback — the seam
/// a transfer reads and writes objects through, so an object staged here is the
/// object the transfer reads.
fn s3(emulator: &Emulator) -> aws_sdk_s3::Client {
    let callback = smithy_server_runtime::EmbeddedEmulator::from_router(emulator.router().clone());
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
        .region(aws_sdk_s3::config::Region::new(REGION))
        .endpoint_url(format!("https://s3.{REGION}.amazonaws.com"))
        .force_path_style(true)
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    aws_sdk_s3::Client::from_conf(config)
}

/// Create `bucket` in this emulator's object store.
///
/// The Region is stated as the location constraint it has to be: a regional
/// endpoint creates a bucket in its own Region, and the unspecified constraint
/// stands for `us-east-1` alone.
async fn create_bucket(emulator: &Emulator, bucket: &str) {
    let created = s3(emulator)
        .create_bucket()
        .bucket(bucket)
        .create_bucket_configuration(
            aws_sdk_s3::types::CreateBucketConfiguration::builder()
                .location_constraint(aws_sdk_s3::types::BucketLocationConstraint::from(REGION))
                .build(),
        )
        .send()
        .await;
    assert!(
        created.is_ok(),
        "staging {bucket}: the bucket must be created, got {created:?}"
    );
}

/// Stage one object through Amazon S3's OWN operations.
async fn stage_object(emulator: &Emulator, key: &str, body: Vec<u8>) {
    let put = s3(emulator)
        .put_object()
        .bucket(BUCKET)
        .key(key)
        .body(aws_sdk_s3::primitives::ByteStream::from(body))
        .send()
        .await;
    assert!(
        put.is_ok(),
        "staging s3://{BUCKET}/{key}: the object must be stored, got {put:?}"
    );
}

/// The bytes of the object at `bucket`/`key`, or `None` when the store holds
/// none.
async fn fetch_object(emulator: &Emulator, bucket: &str, key: &str) -> Option<Vec<u8>> {
    let read = s3(emulator)
        .get_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await
        .ok()?;
    let body = read.body.collect().await.ok()?;
    Some(body.into_bytes().to_vec())
}

/// The JSON document the object at `bucket`/`key` holds.
async fn fetch_document(emulator: &Emulator, bucket: &str, key: &str) -> Value {
    let bytes = fetch_object(emulator, bucket, key)
        .await
        .unwrap_or_else(|| panic!("s3://{bucket}/{key}: the object must be served"));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| panic!("s3://{bucket}/{key}: the object must hold a JSON document"))
}

/// An emulator with the object store mounted, holding `BUCKET` and the
/// workspaces `workspaces` names.
async fn emulator_with(workspaces: &[&str]) -> Emulator {
    let emulator = Emulator::with_object_store();
    create_bucket(&emulator, BUCKET).await;
    for workspace_id in workspaces {
        assert_eq!(
            emulator.create_workspace(workspace_id).await.status,
            200,
            "the workspace {workspace_id:?} the cases name must exist"
        );
    }
    emulator
}

/// `CreateMetadataTransferJob` with an Amazon S3 source and an IoT TwinMaker
/// destination, asserted accepted.
async fn import_job(emulator: &Emulator, job_id: &str, key: &str, workspace_id: &str) {
    let created = emulator
        .create_metadata_transfer_job(json!({
            "metadataTransferJobId": job_id,
            "sources": [{
                "type": "s3",
                "s3Configuration": {"location": format!("arn:aws:s3:::{BUCKET}/{key}")},
            }],
            "destination": {
                "type": "iottwinmaker",
                "iotTwinMakerConfiguration": {"workspace": workspace_arn(workspace_id)},
            },
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "job {job_id:?}: expected 200 from CreateMetadataTransferJob, got {created:?}"
    );
}

/// The job once the clock has made its outcome due.
///
/// The emulator stores the instant and never sleeps for it, so a test that
/// wants the settled job is the one that looks again until it is there —
/// waiting for the CONDITION and not for a span.
async fn settled_job(emulator: &Emulator, job_id: &str) -> Resp {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let read = emulator.get_metadata_transfer_job(job_id).await;
        let state = read.member("status")["state"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if state == METADATA_TRANSFER_JOB_STATE_COMPLETED
            || state == METADATA_TRANSFER_JOB_STATE_ERROR
        {
            return read;
        }
        assert!(
            Instant::now() < deadline,
            "job {job_id:?}: the clock must settle the job, last read {read:?}"
        );
        tokio::time::sleep(POLL_PAUSE).await;
    }
}

/// The four counts a job reports as its progress.
fn progress_of(read: &Resp) -> (i64, i64, i64, i64) {
    let progress = read.member("progress");
    let count = |member: &str| progress[member].as_i64().unwrap_or(-1);
    (
        count("totalCount"),
        count("succeededCount"),
        count("skippedCount"),
        count("failedCount"),
    )
}

/// The ids one document names, by member.
fn named_ids(document: &Value, member: &str, id_member: &str) -> Vec<String> {
    document[member]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry[id_member].as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The documents the import rows carry: `(case, document)`.
///
/// The rows differ along what the document DESCRIBES — a component type alone,
/// an entity alone, both together, a child listed before the parent it hangs
/// off, and property values of several of the modelled data types — and along
/// how many entries it carries.
fn import_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "a component type alone",
            json!({
                "componentTypes": [{
                    "componentTypeId": "com.example.pump",
                    "componentTypeName": "Pump",
                    "description": "a pump",
                    "isSingleton": false,
                    "propertyDefinitions": {
                        "pressure": {
                            "dataType": {"type": "DOUBLE", "unitOfMeasure": "bar"},
                            "isTimeSeries": true,
                        },
                        "serial": {
                            "dataType": {"type": "STRING"},
                            "isExternalId": true,
                        },
                    },
                }],
            }),
        ),
        (
            "an entity alone",
            json!({
                "entities": [{
                    "entityId": "site1",
                    "entityName": "Site 1",
                    "description": "the first site",
                }],
            }),
        ),
        (
            "a component type and an entity instantiating it",
            json!({
                "componentTypes": [{
                    "componentTypeId": "com.example.valve",
                    "propertyDefinitions": {
                        "open": {"dataType": {"type": "BOOLEAN"}},
                    },
                }],
                "entities": [{
                    "entityId": "valve1",
                    "entityName": "Valve 1",
                    "components": {
                        "valve": {
                            "componentTypeId": "com.example.valve",
                            "properties": {
                                "open": {"value": {"booleanValue": true}},
                            },
                        },
                    },
                }],
            }),
        ),
        (
            "a child listed before the parent it hangs off",
            json!({
                "entities": [
                    {
                        "entityId": "childFirst",
                        "entityName": "Child",
                        "parentEntityId": "parentSecond",
                    },
                    {"entityId": "parentSecond", "entityName": "Parent"},
                ],
            }),
        ),
        (
            "property values of several data types",
            json!({
                "componentTypes": [{
                    "componentTypeId": "com.example.readings",
                    "propertyDefinitions": {
                        "count": {"dataType": {"type": "INTEGER"}},
                        "label": {"dataType": {"type": "STRING"}},
                        "ratio": {"dataType": {"type": "DOUBLE"}},
                        "samples": {
                            "dataType": {"type": "LIST", "nestedType": {"type": "INTEGER"}},
                        },
                    },
                }],
                "entities": [{
                    "entityId": "readings1",
                    "entityName": "Readings 1",
                    "components": {
                        "readings": {
                            "componentTypeId": "com.example.readings",
                            "properties": {
                                "count": {"value": {"integerValue": 7}},
                                "label": {"value": {"stringValue": "inlet"}},
                                "ratio": {"value": {"doubleValue": 0.5}},
                                "samples": {"value": {"listValue": [
                                    {"integerValue": 1},
                                    {"integerValue": 2},
                                ]}},
                            },
                        },
                    },
                }],
            }),
        ),
    ]
}

/// RULE: a job whose source is an Amazon S3 metadata document reads that
/// document and writes what it describes into the destination workspace, then
/// settles `COMPLETED` reporting one entry per resource it moved and a
/// `reportUrl` for the report of the run; the destination holds every component
/// type and entity the document described.
#[tokio::test]
async fn a_transfer_writes_the_document_its_source_describes_into_the_destination() {
    for (case, document) in import_rows() {
        // One emulator per row, so what the destination holds afterwards is
        // what THIS document described and nothing a neighbouring row wrote.
        let emulator = emulator_with(&[DESTINATION]).await;
        let key = "documents/import.json";
        stage_object(
            &emulator,
            key,
            serde_json::to_vec(&document).expect("the document serializes"),
        )
        .await;

        let job_id = "importJob";
        import_job(&emulator, job_id, key, DESTINATION).await;
        let settled = settled_job(&emulator, job_id).await;
        assert_eq!(
            settled.member("status")["state"].as_str(),
            Some(METADATA_TRANSFER_JOB_STATE_COMPLETED),
            "case {case:?}: a run that read its source and wrote its destination completes, \
             got {settled:?}"
        );

        let component_type_ids = named_ids(&document, "componentTypes", "componentTypeId");
        let entity_ids = named_ids(&document, "entities", "entityId");
        let entries = component_type_ids.len() + entity_ids.len();
        let expected = (entries as i64, entries as i64, 0, 0);
        let actual = progress_of(&settled);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected every entry written, got {actual:?} in {settled:?}"
        );

        // The destination really holds them: read back through this service's
        // own operations, not through the job's own report.
        for component_type_id in &component_type_ids {
            let read = emulator
                .get_component_type(DESTINATION, component_type_id)
                .await;
            assert_eq!(
                (read.status, read.member("componentTypeId").as_str()),
                (200, Some(component_type_id.as_str())),
                "case {case:?}: the destination must hold {component_type_id:?}, got {read:?}"
            );
        }
        for entity in document["entities"].as_array().into_iter().flatten() {
            let entity_id = entity["entityId"].as_str().unwrap_or_default();
            let read = emulator.get_entity(DESTINATION, entity_id).await;
            assert_eq!(
                (
                    read.status,
                    read.member("entityId").as_str(),
                    read.member("entityName").as_str(),
                ),
                (
                    200,
                    Some(entity_id),
                    entity["entityName"].as_str(),
                ),
                "case {case:?}: the destination must hold the entity {entity_id:?}, got {read:?}"
            );
            // A described parent is the parent the entity hangs off; one the
            // document named none for hangs off the root.
            let expected_parent = entity["parentEntityId"]
                .as_str()
                .unwrap_or(ROOT_PARENT_ENTITY_ID);
            assert_eq!(
                read.member("parentEntityId").as_str(),
                Some(expected_parent),
                "case {case:?}: the entity {entity_id:?} must hang off {expected_parent:?}, \
                 got {read:?}"
            );
            // Every component the document described, with the value it
            // described for each property.
            for (component_name, component) in
                entity["components"].as_object().into_iter().flatten()
            {
                let written = &read.member("components")[component_name];
                assert_eq!(
                    written["componentTypeId"].as_str(),
                    component["componentTypeId"].as_str(),
                    "case {case:?}: the component {component_name:?} of {entity_id:?} must \
                     instantiate the described component type, got {read:?}"
                );
                for (property_name, property) in
                    component["properties"].as_object().into_iter().flatten()
                {
                    assert_eq!(
                        &written["properties"][property_name]["value"],
                        &property["value"],
                        "case {case:?}: the property {property_name:?} of {entity_id:?} must \
                         carry the described value, got {read:?}"
                    );
                }
            }
        }

        // The run wrote its report, and the report says what the job reports.
        let report_url = settled
            .member("reportUrl")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let bucket = report_bucket(REGION);
        let key = format!("{job_id}/transfer-report.json");
        assert!(
            report_url.ends_with(&format!("/{bucket}/{key}")),
            "case {case:?}: the reportUrl must address the report object, got {report_url:?}"
        );
        let report = fetch_document(&emulator, &bucket, &key).await;
        assert_eq!(
            (
                report["state"].as_str(),
                report["progress"]["totalCount"].as_i64(),
                report["progress"]["succeededCount"].as_i64(),
                report["entries"].as_array().map(Vec::len),
            ),
            (
                Some(METADATA_TRANSFER_JOB_STATE_COMPLETED),
                Some(entries as i64),
                Some(entries as i64),
                Some(entries),
            ),
            "case {case:?}: the report must describe the run, got {report}"
        );
    }
}

/// RULE: a job whose destination is Amazon S3 writes the metadata of the
/// workspace its source names as one document there, and that document read
/// back into another workspace describes the same component types and entities
/// — the two halves of the transfer are inverses.
#[tokio::test]
async fn a_workspace_exported_and_imported_back_describes_the_same_metadata() {
    const SOURCE: &str = "exportWorkspace";
    let emulator = emulator_with(&[SOURCE, DESTINATION]).await;

    // A workspace built through this service's OWN creates, so what the export
    // reads is what a caller really put there.
    let created = emulator
        .create_component_type(
            SOURCE,
            "com.example.motor",
            json!({
                "componentTypeName": "Motor",
                "description": "a motor",
                "isSingleton": true,
                "propertyDefinitions": {
                    "rpm": {"dataType": {"type": "INTEGER"}, "isTimeSeries": false},
                    "model": {"dataType": {"type": "STRING"}},
                },
                "propertyGroups": {
                    "readings": {"groupType": "TABULAR", "propertyNames": ["rpm"]},
                },
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the source component type must be created, got {created:?}"
    );
    for (entity_id, body) in [
        (
            "plant",
            json!({"entityId": "plant", "entityName": "Plant", "description": "the plant"}),
        ),
        (
            "motor1",
            json!({
                "entityId": "motor1",
                "entityName": "Motor 1",
                "parentEntityId": "plant",
                "components": {
                    "motor": {
                        "componentTypeId": "com.example.motor",
                        "properties": {
                            "rpm": {"value": {"integerValue": 1500}},
                            "model": {"value": {"stringValue": "M-12"}},
                        },
                    },
                },
            }),
        ),
    ] {
        let created = emulator.create_entity(SOURCE, body).await;
        assert_eq!(
            created.status, 200,
            "the source entity {entity_id:?} must be created, got {created:?}"
        );
    }

    // Export: the workspace out to Amazon S3, under a prefix of the bucket.
    let export_job = "exportJob";
    let exported = emulator
        .create_metadata_transfer_job(json!({
            "metadataTransferJobId": export_job,
            "sources": [{
                "type": "iottwinmaker",
                "iotTwinMakerConfiguration": {"workspace": workspace_arn(SOURCE)},
            }],
            "destination": {
                "type": "s3",
                "s3Configuration": {"location": format!("arn:aws:s3:::{BUCKET}/exports")},
            },
        }))
        .await;
    assert_eq!(
        exported.status, 200,
        "the export must be accepted, got {exported:?}"
    );
    let settled = settled_job(&emulator, export_job).await;
    assert_eq!(
        (
            settled.member("status")["state"].as_str(),
            progress_of(&settled),
        ),
        (Some(METADATA_TRANSFER_JOB_STATE_COMPLETED), (3, 3, 0, 0)),
        "the export must write the workspace's one component type and two entities, \
         got {settled:?}"
    );

    // The object the export wrote is a metadata document describing exactly
    // those resources.
    let exported_key = format!("exports/{export_job}-metadata.json");
    let document = fetch_document(&emulator, BUCKET, &exported_key).await;
    assert_eq!(
        (
            named_ids(&document, "componentTypes", "componentTypeId"),
            named_ids(&document, "entities", "entityId"),
        ),
        (
            vec!["com.example.motor".to_string()],
            vec!["motor1".to_string(), "plant".to_string()],
        ),
        "the exported document must describe the workspace's own metadata, got {document}"
    );

    // Import: the very object the export wrote, into another workspace.
    import_job(&emulator, "reimportJob", &exported_key, DESTINATION).await;
    let settled = settled_job(&emulator, "reimportJob").await;
    assert_eq!(
        (
            settled.member("status")["state"].as_str(),
            progress_of(&settled),
        ),
        (Some(METADATA_TRANSFER_JOB_STATE_COMPLETED), (3, 3, 0, 0)),
        "the re-import must write every entry the document described, got {settled:?}"
    );

    // The two workspaces now describe the same metadata, read back through this
    // service's own operations.
    let source_type = emulator
        .get_component_type(SOURCE, "com.example.motor")
        .await;
    let destination_type = emulator
        .get_component_type(DESTINATION, "com.example.motor")
        .await;
    assert_eq!(
        component_type_facts(&destination_type),
        component_type_facts(&source_type),
        "the imported component type must describe the exported one, got {destination_type:?}"
    );
    for entity_id in ["plant", "motor1"] {
        let source_entity = emulator.get_entity(SOURCE, entity_id).await;
        let destination_entity = emulator.get_entity(DESTINATION, entity_id).await;
        assert_eq!(
            entity_facts(&destination_entity),
            entity_facts(&source_entity),
            "the imported entity {entity_id:?} must describe the exported one, \
             got {destination_entity:?}"
        );
    }
}

/// The members of a component type a metadata document describes, as a read
/// reports them — what has to survive an export and an import.
///
/// The members a document does not carry are left out: a resource's ARN,
/// instants and provisioning state belong to the workspace that holds it, not
/// to the metadata.
fn component_type_facts(read: &Resp) -> Value {
    json!({
        "componentTypeId": read.member("componentTypeId"),
        "componentTypeName": read.member("componentTypeName"),
        "description": read.member("description"),
        "isSingleton": read.member("isSingleton"),
        "extendsFrom": read.member("extendsFrom"),
        "propertyDefinitions": read.member("propertyDefinitions"),
        "propertyGroups": read.member("propertyGroups"),
    })
}

/// The members of an entity a metadata document describes, as a read reports
/// them — the mirror of [`component_type_facts`].
fn entity_facts(read: &Resp) -> Value {
    let components: Value = read
        .member("components")
        .as_object()
        .map(|components| {
            components
                .iter()
                .map(|(component_name, component)| {
                    let properties: Value = component["properties"]
                        .as_object()
                        .map(|properties| {
                            properties
                                .iter()
                                .map(|(property_name, property)| {
                                    (property_name.clone(), property["value"].clone())
                                })
                                .collect::<serde_json::Map<String, Value>>()
                        })
                        .map_or(Value::Null, Value::Object);
                    (
                        component_name.clone(),
                        json!({
                            "componentTypeId": component["componentTypeId"],
                            "description": component["description"],
                            "properties": properties,
                        }),
                    )
                })
                .collect::<serde_json::Map<String, Value>>()
        })
        .map_or(Value::Null, Value::Object);
    json!({
        "entityId": read.member("entityId"),
        "entityName": read.member("entityName"),
        "description": read.member("description"),
        "parentEntityId": read.member("parentEntityId"),
        "components": components,
    })
}

/// RULE: an entry the destination already holds is SKIPPED and one it cannot
/// take is FAILED — each counted in the job's progress, which is what a
/// `COMPLETED` run reports them through — while a source that could not be read
/// at all fails the run itself with the modelled error that says why.
#[tokio::test]
async fn entries_the_destination_cannot_take_are_counted_and_an_unreadable_source_fails_the_run() {
    let emulator = emulator_with(&[DESTINATION]).await;

    // A document the destination takes whole, staged once: the first import
    // writes it, and the second finds every entry already there.
    let document = json!({
        "componentTypes": [{"componentTypeId": "com.example.tank"}],
        "entities": [{"entityId": "tank1", "entityName": "Tank 1"}],
    });
    stage_object(
        &emulator,
        "twice.json",
        serde_json::to_vec(&document).expect("the document serializes"),
    )
    .await;
    // A document describing resources no workspace could be asked to create: a
    // component type in the service's own reserved namespace, and an entity
    // hanging off a parent nothing describes.
    stage_object(
        &emulator,
        "refused.json",
        serde_json::to_vec(&json!({
            "componentTypes": [{"componentTypeId": "com.amazon.iottwinmaker.Mine"}],
            "entities": [{
                "entityId": "orphan",
                "entityName": "Orphan",
                "parentEntityId": "nobody",
            }],
        }))
        .expect("the document serializes"),
    )
    .await;
    stage_object(&emulator, "prose.json", b"not a document at all".to_vec()).await;

    // One row of the table below: a job's source and what the run it earns
    // comes to. A named row rather than a tuple, so what each column means is
    // readable at the row and at the assertion.
    struct Row {
        case: &'static str,
        job_id: &'static str,
        source: Value,
        /// The `MetadataTransferJobState` the job settles in.
        state: &'static str,
        /// The counts it reports: total, succeeded, skipped, failed — each `-1`
        /// where the job reports no progress at all.
        counts: (i64, i64, i64, i64),
        /// The modelled `ErrorCode` it carries, and `None` for a run that did
        /// not fail.
        error_code: Option<&'static str>,
    }

    let rows = vec![
        Row {
            case: "a document the destination takes whole",
            job_id: "firstImport",
            source: s3_source("twice.json"),
            state: METADATA_TRANSFER_JOB_STATE_COMPLETED,
            counts: (2, 2, 0, 0),
            error_code: None,
        },
        Row {
            case: "the same document imported again",
            job_id: "secondImport",
            source: s3_source("twice.json"),
            state: METADATA_TRANSFER_JOB_STATE_COMPLETED,
            counts: (2, 0, 2, 0),
            error_code: None,
        },
        Row {
            case: "entries no workspace could be asked to create",
            job_id: "refusedImport",
            source: s3_source("refused.json"),
            state: METADATA_TRANSFER_JOB_STATE_COMPLETED,
            counts: (2, 0, 0, 2),
            error_code: None,
        },
        Row {
            case: "an object that is no metadata document",
            job_id: "proseImport",
            source: s3_source("prose.json"),
            state: METADATA_TRANSFER_JOB_STATE_ERROR,
            counts: (-1, -1, -1, -1),
            error_code: Some(METADATA_TRANSFER_JOB_VALIDATION_ERROR_CODE),
        },
        Row {
            case: "an object nothing staged",
            job_id: "absentImport",
            source: s3_source("missing.json"),
            state: METADATA_TRANSFER_JOB_STATE_ERROR,
            counts: (-1, -1, -1, -1),
            error_code: Some(METADATA_TRANSFER_JOB_ERROR_CODE),
        },
        Row {
            case: "a source this emulator holds no asset models for",
            job_id: "siteWiseImport",
            source: json!({"type": "iotsitewise"}),
            state: METADATA_TRANSFER_JOB_STATE_ERROR,
            counts: (-1, -1, -1, -1),
            error_code: Some(METADATA_TRANSFER_JOB_ERROR_CODE),
        },
    ];

    for row in rows {
        let Row {
            case,
            job_id,
            source,
            state,
            counts,
            error_code,
        } = row;
        let created = emulator
            .create_metadata_transfer_job(json!({
                "metadataTransferJobId": job_id,
                "sources": [source],
                "destination": {
                    "type": "iottwinmaker",
                    "iotTwinMakerConfiguration": {"workspace": workspace_arn(DESTINATION)},
                },
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateMetadataTransferJob, got {created:?}"
        );
        let settled = settled_job(&emulator, job_id).await;
        assert_eq!(
            settled.member("status")["state"].as_str(),
            Some(state),
            "case {case:?}: expected the job to settle in {state}, got {settled:?}"
        );
        let actual = progress_of(&settled);
        assert_eq!(
            actual, counts,
            "case {case:?}: expected the counts {counts:?}, got {actual:?} in {settled:?}"
        );
        assert_eq!(
            settled.member("status")["error"]["code"].as_str(),
            error_code,
            "case {case:?}: expected the error code {error_code:?}, got {settled:?}"
        );
    }

    // The refused entries left the destination exactly as it was: nothing was
    // written for either of them.
    let reserved = emulator
        .get_component_type(DESTINATION, "com.amazon.iottwinmaker.Mine")
        .await;
    let orphan = emulator.get_entity(DESTINATION, "orphan").await;
    assert_eq!(
        (reserved.status, orphan.status),
        (404, 404),
        "a failed entry must write nothing, got {reserved:?} and {orphan:?}"
    );
}

/// RULE: a transferred schema is held to the SAME rules a caller's own request
/// is — a `dataType.type` and a `groupType` the model's enumerations define,
/// and a property group that names only properties the component has — so a
/// document cannot put into a workspace what the control plane would have
/// refused.
///
/// This writer is the one no boundary validator stands in front of: the values
/// come out of a document the transfer read, not off the wire, so the rules are
/// what decides them. A refused entry fails the ENTRY (`failedCount`) and
/// writes nothing, which is how this writer refuses.
///
/// The rows differ along which member misses its value list and where it sits —
/// a component type's definition, its groups, an entity component's definition,
/// its groups, and a group's names — with a control row inside every list to
/// show the rows are refused for the value and not for their shape. The value
/// lists are the model's own (`ops::types::TYPE_VALUES`,
/// `ops::types::GROUP_TYPE_VALUES`).
#[tokio::test]
async fn a_transferred_schema_is_held_to_the_modelled_value_lists() {
    /// One row: what it is about, the document it stages, the counts the run
    /// reports, and the ids the destination must or must not hold afterwards.
    struct Row {
        case: &'static str,
        document: Value,
        /// `(total, succeeded, skipped, failed)`.
        counts: (i64, i64, i64, i64),
        /// The component type id the destination must hold afterwards, if any.
        component_type_id: Option<&'static str>,
        /// The entity id the destination must hold afterwards, if any.
        entity_id: Option<&'static str>,
    }

    let rows = vec![
        Row {
            case: "a component type definition inside every value list",
            document: json!({
                "componentTypes": [{
                    "componentTypeId": "com.example.good",
                    "propertyDefinitions": {"heat": {"dataType": {"type": "DOUBLE"}}},
                    "propertyGroups": {"G": {"groupType": "TABULAR", "propertyNames": ["heat"]}},
                }],
            }),
            counts: (1, 1, 0, 0),
            component_type_id: Some("com.example.good"),
            entity_id: None,
        },
        Row {
            case: "a component type definition whose dataType.type is outside the list",
            document: json!({
                "componentTypes": [{
                    "componentTypeId": "com.example.badType",
                    "propertyDefinitions": {"heat": {"dataType": {"type": "FOO"}}},
                }],
            }),
            counts: (1, 0, 0, 1),
            component_type_id: None,
            entity_id: None,
        },
        Row {
            case: "a component type definition whose dataType names no type at all",
            document: json!({
                "componentTypes": [{
                    "componentTypeId": "com.example.typeless",
                    "propertyDefinitions": {"heat": {"dataType": {"unitOfMeasure": "bar"}}},
                }],
            }),
            counts: (1, 0, 0, 1),
            component_type_id: None,
            entity_id: None,
        },
        Row {
            case: "a component type group whose groupType is outside the list",
            document: json!({
                "componentTypes": [{
                    "componentTypeId": "com.example.badGroup",
                    "propertyDefinitions": {"heat": {"dataType": {"type": "DOUBLE"}}},
                    "propertyGroups": {"G": {"groupType": "NOT_A_GROUP_TYPE", "propertyNames": ["heat"]}},
                }],
            }),
            counts: (1, 0, 0, 1),
            component_type_id: None,
            entity_id: None,
        },
        Row {
            case: "an entity component definition whose dataType.type is outside the list",
            document: json!({
                "entities": [{
                    "entityId": "badTypeEntity",
                    "entityName": "Bad type",
                    "components": {"C": {
                        "componentTypeId": COMPONENT_HOLDER_TYPE,
                        "properties": {"p": {"definition": {"dataType": {"type": "FOO"}}}},
                    }},
                }],
            }),
            counts: (1, 0, 0, 1),
            component_type_id: None,
            entity_id: None,
        },
        Row {
            case: "an entity component group whose groupType is outside the list",
            document: json!({
                "entities": [{
                    "entityId": "badGroupEntity",
                    "entityName": "Bad group",
                    "components": {"C": {
                        "componentTypeId": COMPONENT_HOLDER_TYPE,
                        "properties": {"p": {"definition": {"dataType": {"type": "STRING"}}}},
                        "propertyGroups": {"G": {"groupType": "tabular", "propertyNames": ["p"]}},
                    }},
                }],
            }),
            counts: (1, 0, 0, 1),
            component_type_id: None,
            entity_id: None,
        },
        Row {
            case: "an entity component group naming a property the component does not have",
            document: json!({
                "entities": [{
                    "entityId": "strandedGroupEntity",
                    "entityName": "Stranded group",
                    "components": {"C": {
                        "componentTypeId": COMPONENT_HOLDER_TYPE,
                        "properties": {"p": {"definition": {"dataType": {"type": "STRING"}}}},
                        "propertyGroups": {"G": {"propertyNames": ["notAProperty"]}},
                    }},
                }],
            }),
            counts: (1, 0, 0, 1),
            component_type_id: None,
            entity_id: None,
        },
        Row {
            case: "an entity component group naming the property the same entry declares",
            document: json!({
                "entities": [{
                    "entityId": "goodGroupEntity",
                    "entityName": "Good group",
                    "components": {"C": {
                        "componentTypeId": COMPONENT_HOLDER_TYPE,
                        "properties": {"p": {"definition": {"dataType": {"type": "STRING"}}}},
                        "propertyGroups": {"G": {"groupType": "TABULAR", "propertyNames": ["p"]}},
                    }},
                }],
            }),
            counts: (1, 1, 0, 0),
            component_type_id: None,
            entity_id: Some("goodGroupEntity"),
        },
    ];

    for row in rows {
        let Row {
            case,
            document,
            counts,
            component_type_id,
            entity_id,
        } = row;
        // One emulator per row, so what the destination holds afterwards is
        // what THIS document described.
        let emulator = emulator_with(&[DESTINATION]).await;
        // The component type every entity row's component instantiates: a
        // component names the type it instantiates, and the group rule is
        // decided against the properties that type gives it, so the destination
        // has to hold it before the document lands.
        let holder = emulator
            .create_component_type(DESTINATION, COMPONENT_HOLDER_TYPE, json!({}))
            .await;
        assert_eq!(
            holder.status, 200,
            "case {case:?}: expected 200 from CreateComponentType {COMPONENT_HOLDER_TYPE:?}, got \
             {holder:?}"
        );
        let key = "documents/schema.json";
        stage_object(
            &emulator,
            key,
            serde_json::to_vec(&document).expect("the document serializes"),
        )
        .await;

        let job_id = "schemaJob";
        import_job(&emulator, job_id, key, DESTINATION).await;
        let settled = settled_job(&emulator, job_id).await;
        let actual = progress_of(&settled);
        assert_eq!(
            actual, counts,
            "case {case:?}: expected the counts {counts:?}, got {actual:?} in {settled:?}"
        );

        // What the run reported is what the destination really holds: a refused
        // entry wrote nothing, an accepted one is readable through this
        // service's own operations.
        for described in document["componentTypes"].as_array().into_iter().flatten() {
            let described_id = described["componentTypeId"].as_str().unwrap_or_default();
            let read = emulator.get_component_type(DESTINATION, described_id).await;
            let expected = match component_type_id == Some(described_id) {
                true => 200,
                false => 404,
            };
            assert_eq!(
                read.status, expected,
                "case {case:?}: expected {expected} from GetComponentType {described_id:?}, got \
                 {read:?}"
            );
        }
        for described in document["entities"].as_array().into_iter().flatten() {
            let described_id = described["entityId"].as_str().unwrap_or_default();
            let read = emulator.get_entity(DESTINATION, described_id).await;
            let expected = match entity_id == Some(described_id) {
                true => 200,
                false => 404,
            };
            assert_eq!(
                read.status, expected,
                "case {case:?}: expected {expected} from GetEntity {described_id:?}, got {read:?}"
            );
        }
    }
}

/// An Amazon S3 source naming the object `key` of the suite's bucket.
fn s3_source(key: &str) -> Value {
    json!({
        "type": "s3",
        "s3Configuration": {"location": format!("arn:aws:s3:::{BUCKET}/{key}")},
    })
}
