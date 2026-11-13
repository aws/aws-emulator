// SPDX-License-Identifier: CC0-1.0

//! The sentence a lookup that resolved no stream answers with.
//!
//! One rule, asserted over a SET of inputs: every operation that resolves a stream by
//! name refuses a name no stream has with `ResourceNotFoundException` and one sentence
//! naming the account the lookup was scoped to and nothing of the name it was given —
//! the same sentence for every name, whichever operation did the resolving. Which
//! sentence is a property of the PATH the lookup is on, and the three paths word that
//! one fact differently ([`NotFoundWording`]), so no path's sentence stands in for
//! another's. The recorded cases (a describe, and a stop-encryption, of one absent name)
//! are two rows of the tables below.
//!
//! Every request goes through the real router, so what is asserted is the body a client
//! reads. The harness is the lifecycle suite's.

use serde_json::{json, Value};

use super::delivery_stream_lifecycle::{error_code, error_message, Emulator};

/// Which of this service's not-found sentences a path answers with. One fact, three
/// wordings: the sentence belongs to the path, not to the name that was probed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NotFoundWording {
    /// The control-plane reads, the delete, the tag changes and the destination update.
    ControlPlaneRead,
    /// The two encryption changes: the noun is `accountId`, and there is no terminal
    /// period.
    EncryptionChange,
    /// The data plane's puts.
    DataPlane,
}

impl NotFoundWording {
    /// The sentence this path states, with the account the lookup was scoped to in its
    /// one variable slot.
    pub(crate) fn sentence(self, account: &str) -> String {
        match self {
            Self::ControlPlaneRead => format!("Firehose under account {account} not found."),
            Self::EncryptionChange => format!("Firehose under accountId {account} not found"),
            Self::DataPlane => format!("Firehose not found under account {account}."),
        }
    }
}

/// One operation that resolves a stream by name, and the request that probes `name` with
/// every OTHER member of that request valid, so the lookup is what refuses it.
///
/// `pub(crate)` with the probe tables below so a sibling suite probes the same operations
/// rather than restating them.
pub(crate) struct Probe {
    pub(crate) case: &'static str,
    pub(crate) operation: &'static str,
    pub(crate) request: fn(&str) -> Value,
    /// The sentence this operation's path words the miss in.
    pub(crate) wording: NotFoundWording,
}

/// Every control-plane operation that takes a `DeliveryStreamName` and resolves it
/// (`smithy_query(query="digest")`): the reads, the delete, the tag changes, the two
/// encryption changes and the destination update.
pub(crate) fn control_plane_probes() -> Vec<Probe> {
    vec![
        Probe {
            case: "describe",
            operation: "DescribeDeliveryStream",
            request: |name| json!({ "DeliveryStreamName": name }),
            wording: NotFoundWording::ControlPlaneRead,
        },
        Probe {
            case: "describe from a start id",
            operation: "DescribeDeliveryStream",
            request: |name| json!({
                "DeliveryStreamName": name,
                "Limit": 1,
                "ExclusiveStartDestinationId": "destinationId-000000000001",
            }),
            wording: NotFoundWording::ControlPlaneRead,
        },
        Probe {
            case: "delete",
            operation: "DeleteDeliveryStream",
            request: |name| json!({ "DeliveryStreamName": name }),
            wording: NotFoundWording::ControlPlaneRead,
        },
        Probe {
            case: "delete without allowing a force",
            operation: "DeleteDeliveryStream",
            request: |name| json!({ "DeliveryStreamName": name, "AllowForceDelete": false }),
            wording: NotFoundWording::ControlPlaneRead,
        },
        Probe {
            case: "list tags",
            operation: "ListTagsForDeliveryStream",
            request: |name| json!({ "DeliveryStreamName": name }),
            wording: NotFoundWording::ControlPlaneRead,
        },
        Probe {
            case: "add a tag",
            operation: "TagDeliveryStream",
            request: |name| json!({
                "DeliveryStreamName": name,
                "Tags": [{ "Key": "owner", "Value": "team" }],
            }),
            wording: NotFoundWording::ControlPlaneRead,
        },
        Probe {
            case: "remove a tag",
            operation: "UntagDeliveryStream",
            request: |name| json!({ "DeliveryStreamName": name, "TagKeys": ["owner"] }),
            wording: NotFoundWording::ControlPlaneRead,
        },
        Probe {
            case: "start encryption",
            operation: "StartDeliveryStreamEncryption",
            request: |name| json!({ "DeliveryStreamName": name }),
            wording: NotFoundWording::EncryptionChange,
        },
        Probe {
            case: "start encryption naming the AWS-owned CMK explicitly",
            operation: "StartDeliveryStreamEncryption",
            request: |name| json!({
                "DeliveryStreamName": name,
                "DeliveryStreamEncryptionConfigurationInput": {
                    "KeyType": "AWS_OWNED_CMK",
                },
            }),
            wording: NotFoundWording::EncryptionChange,
        },
        Probe {
            case: "stop encryption",
            operation: "StopDeliveryStreamEncryption",
            request: |name| json!({ "DeliveryStreamName": name }),
            wording: NotFoundWording::EncryptionChange,
        },
        Probe {
            case: "update a destination",
            operation: "UpdateDestination",
            request: |name| json!({
                "DeliveryStreamName": name,
                "CurrentDeliveryStreamVersionId": "1",
                "DestinationId": "destinationId-000000000001",
                "S3DestinationUpdate": { "Prefix": "x/" },
            }),
            wording: NotFoundWording::ControlPlaneRead,
        },
    ]
}

/// Every DATA-plane operation that takes a `DeliveryStreamName` and resolves it
/// (`smithy_query(query="digest")`): the single put and the batch put.
pub(crate) fn data_plane_probes() -> Vec<Probe> {
    vec![
        Probe {
            case: "put a record",
            operation: "PutRecord",
            request: |name| json!({ "DeliveryStreamName": name, "Record": { "Data": "eA==" } }),
            wording: NotFoundWording::DataPlane,
        },
        Probe {
            case: "put a batch",
            operation: "PutRecordBatch",
            request: |name| json!({
                "DeliveryStreamName": name,
                "Records": [{ "Data": "eA==" }, { "Data": "eQ==" }],
            }),
            wording: NotFoundWording::DataPlane,
        },
    ]
}

/// Names differing in every way a `DeliveryStreamName` can — length (its 1-character
/// minimum and its 64-character maximum), the characters it is spelled with, and how
/// closely it resembles a name that DOES exist. None of them occurs inside the expected
/// sentence, so the "never echoed" assertion below means what it says.
fn probed_names() -> Vec<String> {
    vec![
        "q".to_string(),
        "ghost-stream".to_string(),
        "ghost.stream".to_string(),
        "GHOST_1".to_string(),
        "present-stream-2".to_string(),
        "z".repeat(64),
    ]
}

#[tokio::test]
async fn every_control_plane_lookup_of_an_absent_stream_states_the_account_and_not_the_name() {
    let emulator = Emulator::new();
    let account = engine::emulator_account_id();

    // One stream exists and has settled, so each refusal below is a lookup that resolved
    // nothing rather than an empty register, and a probe that resembles that stream's
    // name still resolves nothing.
    emulator.create_stream("present-stream", None).await;
    emulator.await_active("present-stream").await;

    for probe in control_plane_probes() {
        let expected = probe.wording.sentence(account);
        for name in probed_names() {
            let case = format!("{} of {name:?}", probe.case);
            let (status, refused) = emulator.call(probe.operation, (probe.request)(&name)).await;
            assert_eq!(status, 400, "case {case:?}: got {refused:?}");
            let code = error_code(&refused);
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case:?}: expected \"ResourceNotFoundException\", got {code:?}"
            );
            let message = error_message(&refused);
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
            assert!(
                !message.contains(&name),
                "case {case:?}: the probed name must not be echoed, got {message:?}"
            );
        }
    }

    // The stream that DOES exist is untouched by all of that: it still resolves.
    let (status, described) = emulator
        .call(
            "DescribeDeliveryStream",
            json!({ "DeliveryStreamName": "present-stream" }),
        )
        .await;
    assert_eq!(status, 200, "the present stream still resolves: {described:?}");
    let name = described["DeliveryStreamDescription"]["DeliveryStreamName"].as_str();
    assert_eq!(
        name,
        Some("present-stream"),
        "the present stream still resolves: got {described:?}"
    );
}

#[tokio::test]
async fn a_put_to_an_absent_stream_states_the_account_in_the_data_plane_wording() {
    let emulator = Emulator::new();
    let account = engine::emulator_account_id();
    let expected = NotFoundWording::DataPlane.sentence(account);

    let puts: Vec<Probe> = data_plane_probes();

    emulator.create_stream("present-stream", None).await;
    emulator.await_active("present-stream").await;

    for put in puts {
        for name in probed_names() {
            let case = format!("{} of {name:?}", put.case);
            let (status, refused) = emulator.call(put.operation, (put.request)(&name)).await;
            assert_eq!(status, 400, "case {case:?}: got {refused:?}");
            let code = error_code(&refused);
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case:?}: expected \"ResourceNotFoundException\", got {code:?}"
            );
            let message = error_message(&refused);
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
            assert!(
                !message.contains(&name),
                "case {case:?}: the probed name must not be echoed, got {message:?}"
            );
        }
    }

    // A put to the stream that exists is accepted, so the refusals above are the lookup
    // and not the put path itself.
    let (status, accepted) = emulator
        .call(
            "PutRecord",
            json!({
                "DeliveryStreamName": "present-stream",
                "Record": { "Data": "eA==" },
            }),
        )
        .await;
    assert_eq!(status, 200, "a put to the present stream: got {accepted:?}");
    assert!(
        accepted["RecordId"].as_str().is_some_and(|id| !id.is_empty()),
        "a put to the present stream answers a record id, got {accepted:?}"
    );
}
