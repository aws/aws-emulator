// SPDX-License-Identifier: CC0-1.0

//! `UpdateInstanceStorageConfig` — updating the configuration an existing association
//! holds, end to end over HTTP through the same router the emulator serves with.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! * an update REPLACES the configuration the association named by the request holds
//!   for its resource type, so every later read of it reports exactly the
//!   configuration the update carried, under the association id the association
//!   minted — for every `StorageType` the model names, and whether the replacement
//!   adds, drops or keeps the optional member its storage type allows;
//! * the stored configuration carries the association id the PATH names, whatever
//!   `AssociationId` the body's own configuration held;
//! * the operation is idempotent: repeating an update, with or without a
//!   `ClientToken`, leaves the same configuration and no second association;
//! * an instance or an association that does not exist, and an association paired
//!   with another resource type, update nothing and are refused with the modelled
//!   `ResourceNotFoundException` (HTTP 404) — never a 501;
//! * a replacement that does not carry the member its `StorageType` names is refused
//!   with the modelled `InvalidParameterException` (HTTP 400), and leaves the
//!   configuration that was already stored.

use serde_json::{json, Value};
use service_com_amazonaws_connect_amazonconnectservice::ops::types::{
    INSTANCE_STORAGE_RESOURCE_TYPE_VALUES, STORAGE_TYPE_VALUES,
};
use service_com_amazonaws_connect_amazonconnectservice::state::STORAGE_TYPE_CONFIG_MEMBERS;

use super::harness::{connect_managed, Answer, Connect};

/// An identifier of the form this service mints that names nothing.
const ABSENT_ID: &str = "11111111-2222-3333-4444-555555555555";

/// The fixed sentence this service states for a resource it does not hold, written out
/// here rather than read off the implementation so the suite pins the text itself.
const RESOURCE_NOT_FOUND: &str = "Resource not found";

/// Which configuration of a row is meant: the one the association stored, or the one
/// the update replaces it with. Every value a row sends differs between the two, so a
/// read reporting the first after an update is a visible failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Associated,
    Updated,
}

/// One row of the table: a resource type to store, the storage type that stores it,
/// and whether the row's `S3Config` carries the OPTIONAL `EncryptionConfig`
/// (`com.amazonaws.connect#S3Config$EncryptionConfig`) in each phase — so the rows
/// span adding that member, dropping it and keeping it, which is what shows an update
/// REPLACING the stored configuration rather than merging into it.
///
/// Every resource type appears at most once, so the whole table is associated onto one
/// instance. The names come from `com.amazonaws.connect#InstanceStorageResourceType`
/// and `com.amazonaws.connect#StorageType`, which
/// [`the_table_names_only_modelled_resource_and_storage_types`] checks this table
/// against.
#[derive(Debug, Clone, Copy)]
struct Case {
    resource_type: &'static str,
    storage_type: &'static str,
    s3_encrypted: (bool, bool),
}

impl Case {
    /// Whether this row's `S3Config` carries `EncryptionConfig` in `phase`.
    fn s3_encrypted_in(self, phase: Phase) -> bool {
        match phase {
            Phase::Associated => self.s3_encrypted.0,
            Phase::Updated => self.s3_encrypted.1,
        }
    }
}

/// The table every case below is driven over: all four storage types, a spread of the
/// resource types each of them stores, and — for the storage type whose encryption
/// configuration is optional — the update dropping it, adding it and keeping it.
const CASES: &[Case] = &[
    Case {
        resource_type: "CHAT_TRANSCRIPTS",
        storage_type: "S3",
        s3_encrypted: (true, false),
    },
    Case {
        resource_type: "ATTACHMENTS",
        storage_type: "S3",
        s3_encrypted: (false, true),
    },
    Case {
        resource_type: "EMAIL_MESSAGES",
        storage_type: "S3",
        s3_encrypted: (false, false),
    },
    Case {
        resource_type: "SCREEN_RECORDINGS",
        storage_type: "S3",
        s3_encrypted: (true, true),
    },
    Case {
        resource_type: "MEDIA_STREAMS",
        storage_type: "KINESIS_VIDEO_STREAM",
        s3_encrypted: (false, false),
    },
    Case {
        resource_type: "AGENT_EVENTS",
        storage_type: "KINESIS_STREAM",
        s3_encrypted: (false, false),
    },
    Case {
        resource_type: "CONTACT_TRACE_RECORDS",
        storage_type: "KINESIS_FIREHOSE",
        s3_encrypted: (false, false),
    },
];

/// `AssociateInstanceStorageConfig`: `PUT /instance/{InstanceId}/storage-config`, the
/// resource type and the configuration in the body.
async fn associate(connect: &Connect, instance_id: &str, body: Value) -> Answer {
    connect
        .send_answer(
            "PUT",
            &format!("/instance/{instance_id}/storage-config"),
            Some(body),
        )
        .await
}

/// `UpdateInstanceStorageConfig`:
/// `POST /instance/{InstanceId}/storage-config/{AssociationId}?resourceType=…`, the
/// replacement configuration in the body.
async fn update(
    connect: &Connect,
    instance_id: &str,
    association_id: &str,
    resource_type: &str,
    body: Value,
) -> Answer {
    connect
        .send_answer(
            "POST",
            &format!(
                "/instance/{instance_id}/storage-config/{association_id}?resourceType={resource_type}"
            ),
            Some(body),
        )
        .await
}

/// `DescribeInstanceStorageConfig`:
/// `GET /instance/{InstanceId}/storage-config/{AssociationId}?resourceType=…`.
async fn describe(
    connect: &Connect,
    instance_id: &str,
    association_id: &str,
    resource_type: &str,
) -> Answer {
    connect
        .send_answer(
            "GET",
            &format!(
                "/instance/{instance_id}/storage-config/{association_id}?resourceType={resource_type}"
            ),
            None,
        )
        .await
}

/// `ListInstanceStorageConfigs`:
/// `GET /instance/{InstanceId}/storage-configs?resourceType=…`.
async fn list(connect: &Connect, instance_id: &str, resource_type: &str) -> Answer {
    connect
        .send_answer(
            "GET",
            &format!("/instance/{instance_id}/storage-configs?resourceType={resource_type}"),
            None,
        )
        .await
}

/// The `InstanceStorageConfig` a row sends in `phase`: its `StorageType` and the one
/// member that storage type names, with values derived from the row's resource type
/// and its phase, so no two rows and no two phases send the same configuration.
fn storage_config(case: Case, phase: Phase) -> Value {
    let slug = case.resource_type.to_lowercase().replace('_', "-");
    let (tag, retention_period_hours) = match phase {
        Phase::Associated => ("associated", 48),
        Phase::Updated => ("updated", 72),
    };
    let encryption = json!({
        "EncryptionType": "KMS",
        "KeyId": format!("arn:aws:kms:us-east-1:000000000000:key/{slug}-{tag}"),
    });
    match case.storage_type {
        "S3" => {
            let mut s3 = json!({
                "BucketName": format!("connect-{slug}-{tag}"),
                "BucketPrefix": format!("{slug}/{tag}"),
            });
            if case.s3_encrypted_in(phase) {
                s3["EncryptionConfig"] = encryption;
            }
            json!({ "StorageType": "S3", "S3Config": s3 })
        }
        "KINESIS_VIDEO_STREAM" => json!({
            "StorageType": "KINESIS_VIDEO_STREAM",
            "KinesisVideoStreamConfig": {
                "Prefix": format!("{slug}-{tag}"),
                "RetentionPeriodHours": retention_period_hours,
                "EncryptionConfig": encryption,
            },
        }),
        "KINESIS_STREAM" => json!({
            "StorageType": "KINESIS_STREAM",
            "KinesisStreamConfig": {
                "StreamArn": format!("arn:aws:kinesis:us-east-1:000000000000:stream/{slug}-{tag}"),
            },
        }),
        "KINESIS_FIREHOSE" => json!({
            "StorageType": "KINESIS_FIREHOSE",
            "KinesisFirehoseConfig": {
                "FirehoseArn":
                    format!("arn:aws:firehose:us-east-1:000000000000:deliverystream/{slug}-{tag}"),
            },
        }),
        other => panic!("case {case:?}: the table names an unhandled storage type {other}"),
    }
}

/// The body an ASSOCIATION of a row sends: the configuration, the resource type the
/// association binds in its body, and a `ClientToken` where the case drives one.
fn associate_body(case: Case, phase: Phase) -> Value {
    json!({
        "ResourceType": case.resource_type,
        "StorageConfig": storage_config(case, phase),
    })
}

/// The body an UPDATE of a row sends. `ResourceType` is NOT in it: the update binds
/// that member to the `resourceType` query parameter, so the request names the
/// resource type in the URI and carries only the replacement configuration and, where
/// the case drives one, a `ClientToken`.
fn update_body(case: Case, phase: Phase, client_token: Option<&str>) -> Value {
    let mut body = json!({ "StorageConfig": storage_config(case, phase) });
    if let Some(token) = client_token {
        body["ClientToken"] = json!(token);
    }
    body
}

/// The configuration a read reports once `case` has been stored in `phase` under
/// `association_id`: what was sent, carrying the association's own identifier.
fn stored(case: Case, phase: Phase, association_id: &str) -> Value {
    let mut expected = storage_config(case, phase);
    expected["AssociationId"] = json!(association_id);
    expected
}

/// Associate every row onto one instance, handing back each row's association id.
async fn associate_every_row(connect: &Connect, instance_id: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for case in CASES {
        let answer = associate(connect, instance_id, associate_body(*case, Phase::Associated)).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected the association to answer HTTP 200, got {} {}",
            answer.status, answer.body
        );
        let id = answer.body["AssociationId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected an AssociationId, got {}",
                    answer.body
                )
            })
            .to_string();
        ids.push(id);
    }
    ids
}

/// The `Message` member of an answered error — the member name the model gives it.
fn message_of(body: &Value) -> String {
    body["Message"].as_str().unwrap_or_default().to_string()
}

/// The table is written by hand, so it is pinned to the model's own value lists: a row
/// naming anything else is a stale table, not a failing emulator.
#[test]
fn the_table_names_only_modelled_resource_and_storage_types() {
    for case in CASES {
        assert!(
            INSTANCE_STORAGE_RESOURCE_TYPE_VALUES.contains(&case.resource_type),
            "case {case:?}: InstanceStorageResourceType does not name {}",
            case.resource_type
        );
        assert!(
            STORAGE_TYPE_VALUES.contains(&case.storage_type),
            "case {case:?}: StorageType does not name {}",
            case.storage_type
        );
    }
    for (storage_type, _, _) in STORAGE_TYPE_CONFIG_MEMBERS {
        assert!(
            CASES.iter().any(|case| case.storage_type == storage_type),
            "the table leaves the storage type {storage_type} unexercised"
        );
    }
}

/// An update replaces the stored configuration: every row reads back as the
/// replacement — not the configuration it was associated with, and not a merge of the
/// two — under the association id the association minted, through both reads, and the
/// rows that were not updated yet are untouched.
#[tokio::test]
async fn an_update_replaces_the_configuration_every_read_reports() {
    let connect = Connect::new();
    let (instance_id, _) = connect
        .create_instance(connect_managed("storage-update"))
        .await;
    let ids = associate_every_row(&connect, &instance_id).await;

    for (index, case) in CASES.iter().enumerate() {
        let association_id = &ids[index];
        let answer = update(
            &connect,
            &instance_id,
            association_id,
            case.resource_type,
            update_body(*case, Phase::Updated, None),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} {}",
            answer.status, answer.body
        );

        let read = describe(&connect, &instance_id, association_id, case.resource_type).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the updated association to stay readable, got {} {}",
            read.status, read.body
        );
        let expected = stored(*case, Phase::Updated, association_id);
        assert_eq!(
            read.body["StorageConfig"], expected,
            "case {case:?}: expected the stored config {expected}, got {}",
            read.body["StorageConfig"]
        );

        let listed = list(&connect, &instance_id, case.resource_type).await;
        assert_eq!(
            listed.body["StorageConfigs"],
            json!([expected]),
            "case {case:?}: expected the list to report {expected}, got {}",
            listed.body["StorageConfigs"]
        );

        // The rows further down the table are still as they were associated: an update
        // reaches one association, not the instance's other resource types.
        for (other_index, other) in CASES.iter().enumerate().skip(index + 1) {
            let untouched =
                describe(&connect, &instance_id, &ids[other_index], other.resource_type).await;
            let expected = stored(*other, Phase::Associated, &ids[other_index]);
            assert_eq!(
                untouched.body["StorageConfig"], expected,
                "case {case:?}: expected {other:?} to be untouched as {expected}, got {}",
                untouched.body["StorageConfig"]
            );
        }
    }
}

/// The association id is the association's own, never the caller's: an update whose
/// body configuration carries some other `AssociationId` stores the one the PATH names.
#[tokio::test]
async fn an_update_keeps_the_association_id_the_path_names() {
    let connect = Connect::new();
    let (instance_id, _) = connect
        .create_instance(connect_managed("storage-update-id"))
        .await;
    let ids = associate_every_row(&connect, &instance_id).await;

    for (index, case) in CASES.iter().enumerate() {
        let association_id = &ids[index];
        let mut body = update_body(*case, Phase::Updated, None);
        body["StorageConfig"]["AssociationId"] = json!(ABSENT_ID);
        let answer = update(
            &connect,
            &instance_id,
            association_id,
            case.resource_type,
            body,
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} {}",
            answer.status, answer.body
        );

        let read = describe(&connect, &instance_id, association_id, case.resource_type).await;
        let expected = stored(*case, Phase::Updated, association_id);
        assert_eq!(
            read.body["StorageConfig"], expected,
            "case {case:?}: expected the path's association id in {expected}, got {}",
            read.body["StorageConfig"]
        );
        // And nothing answers under the identifier the body asked for.
        let decoy = describe(&connect, &instance_id, ABSENT_ID, case.resource_type).await;
        assert_eq!(
            decoy.status, 404,
            "case {case:?}: expected the body's association id to name nothing, got {} {}",
            decoy.status, decoy.body
        );
    }
}

/// The operation is idempotent: repeating an update — with a `ClientToken` and without
/// one — leaves the same configuration, and the instance still holds exactly one
/// association for the resource type.
#[tokio::test]
async fn repeating_an_update_leaves_the_same_configuration() {
    let connect = Connect::new();
    let (instance_id, _) = connect
        .create_instance(connect_managed("storage-update-idempotent"))
        .await;
    let ids = associate_every_row(&connect, &instance_id).await;

    for (index, case) in CASES.iter().enumerate() {
        let association_id = &ids[index];
        let token = format!("token-{}", case.resource_type);
        let expected = stored(*case, Phase::Updated, association_id);
        for attempt in ["first", "repeat", "repeat with no token"] {
            let client_token = (attempt != "repeat with no token").then_some(token.as_str());
            let answer = update(
                &connect,
                &instance_id,
                association_id,
                case.resource_type,
                update_body(*case, Phase::Updated, client_token),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?} {attempt}: expected HTTP 200, got {} {}",
                answer.status, answer.body
            );

            let read = describe(&connect, &instance_id, association_id, case.resource_type).await;
            assert_eq!(
                read.body["StorageConfig"], expected,
                "case {case:?} {attempt}: expected the stored config {expected}, got {}",
                read.body["StorageConfig"]
            );
            let listed = list(&connect, &instance_id, case.resource_type).await;
            assert_eq!(
                listed.body["StorageConfigs"],
                json!([expected]),
                "case {case:?} {attempt}: expected one association for the resource type, got {}",
                listed.body["StorageConfigs"]
            );
        }
    }
}

/// Nothing is updated that was never stored: an unknown instance, an unknown
/// association, and an association addressed under another resource type are all the
/// modelled not-found error, and each leaves the stored configuration alone.
#[tokio::test]
async fn an_unknown_instance_or_association_updates_nothing() {
    let connect = Connect::new();
    let (instance_id, _) = connect
        .create_instance(connect_managed("storage-update-not-found"))
        .await;
    assert_ne!(instance_id, ABSENT_ID, "the absent id must name no instance");
    let ids = associate_every_row(&connect, &instance_id).await;

    for (index, case) in CASES.iter().enumerate() {
        let association_id = &ids[index];
        // The resource type of another row, which this association does not store.
        let other_resource_type = CASES[(index + 1) % CASES.len()].resource_type;
        let refusals = [
            (
                "an unknown instance",
                update(
                    &connect,
                    ABSENT_ID,
                    association_id,
                    case.resource_type,
                    update_body(*case, Phase::Updated, None),
                )
                .await,
            ),
            (
                "an unknown association",
                update(
                    &connect,
                    &instance_id,
                    ABSENT_ID,
                    case.resource_type,
                    update_body(*case, Phase::Updated, None),
                )
                .await,
            ),
            (
                "an association addressed under another resource type",
                update(
                    &connect,
                    &instance_id,
                    association_id,
                    other_resource_type,
                    update_body(*case, Phase::Updated, None),
                )
                .await,
            ),
        ];

        for (label, answer) in refusals {
            assert_eq!(
                answer.status, 404,
                "case {case:?} with {label}: expected HTTP 404, got {} {}",
                answer.status, answer.body
            );
            assert!(
                answer.error_code.contains("ResourceNotFoundException"),
                "case {case:?} with {label}: expected ResourceNotFoundException, got {:?} {}",
                answer.error_code,
                answer.body
            );
            let actual = message_of(&answer.body);
            assert_eq!(
                actual, RESOURCE_NOT_FOUND,
                "case {case:?} with {label}: expected message {RESOURCE_NOT_FOUND:?}, got \
                 {actual:?}"
            );
        }

        // Every refusal left the configuration the association was given.
        let read = describe(&connect, &instance_id, association_id, case.resource_type).await;
        let expected = stored(*case, Phase::Associated, association_id);
        assert_eq!(
            read.body["StorageConfig"], expected,
            "case {case:?}: expected the refused updates to leave {expected}, got {}",
            read.body["StorageConfig"]
        );
    }
}

/// A replacement has to carry the member its `StorageType` names: one that carries
/// none of them, and one that carries another storage type's member, are both refused
/// with the modelled bad-parameter error, and the stored configuration stays as it was.
#[tokio::test]
async fn a_replacement_without_its_storage_type_member_is_refused() {
    let connect = Connect::new();
    let (instance_id, _) = connect
        .create_instance(connect_managed("storage-update-bad-config"))
        .await;
    let ids = associate_every_row(&connect, &instance_id).await;

    for (index, case) in CASES.iter().enumerate() {
        let association_id = &ids[index];
        let (_, member, _) = STORAGE_TYPE_CONFIG_MEMBERS
            .iter()
            .copied()
            .find(|(storage_type, _, _)| *storage_type == case.storage_type)
            .expect("the table names only storage types the service pairs with a member");

        // A configuration of a DIFFERENT storage type, as a replacement that carries a
        // member — just not the one this row's storage type names.
        let decoy = CASES
            .iter()
            .copied()
            .find(|other| other.storage_type != case.storage_type)
            .expect("the table spans more than one storage type");
        let decoy_config = storage_config(decoy, Phase::Updated);
        let mut wrong_member = json!({ "StorageType": case.storage_type });
        for (key, value) in decoy_config.as_object().expect("an object") {
            if key != "StorageType" {
                wrong_member[key] = value.clone();
            }
        }

        for (label, config) in [
            (
                "no configuration member",
                json!({ "StorageType": case.storage_type }),
            ),
            ("another storage type's member", wrong_member),
        ] {
            let answer = update(
                &connect,
                &instance_id,
                association_id,
                case.resource_type,
                json!({ "StorageConfig": config }),
            )
            .await;
            assert_eq!(
                answer.status, 400,
                "case {case:?} with {label}: expected HTTP 400, got {} {}",
                answer.status, answer.body
            );
            assert!(
                answer.error_code.contains("InvalidParameterException"),
                "case {case:?} with {label}: expected InvalidParameterException, got {:?} {}",
                answer.error_code,
                answer.body
            );
        }

        // And the refusals left the configuration the association was given.
        let read = describe(&connect, &instance_id, association_id, case.resource_type).await;
        let expected = stored(*case, Phase::Associated, association_id);
        assert_eq!(
            read.body["StorageConfig"], expected,
            "case {case:?} (needs {member}): expected the refused updates to leave \
             {expected}, got {}",
            read.body["StorageConfig"]
        );
    }
}
