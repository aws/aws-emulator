// SPDX-License-Identifier: CC0-1.0

//! What a read of an SSE-KMS object answers while the object's own KMS key runs no
//! cryptographic operation.
//!
//! RULE: the bytes of an object stored with server-side encryption under a KMS key are
//! readable only THROUGH that key, so every read that serves such an object's BYTES performs
//! `kms:Decrypt` on it — and a key whose `KeyState` is outside the states that run
//! cryptographic operations fails such a call inside KMS, whoever asks. So the read has no
//! plaintext to serve and S3 relays KMS's own failure: HTTP 400, the KMS exception's own name
//! qualified by the service that raised it, and KMS's own sentence (the key's ARN and the
//! state it is in). WHICH exception is the key state's own, the split KMS's `Decrypt` itself
//! makes: a `Disabled` key raises `DisabledException` ("the request was rejected because the
//! specified KMS key is not enabled",
//! `smithy_query(query="shape",shape="DisabledException",model_path="services/service-com-amazonaws-kms-trentservice/smithy-model/com-amazonaws-kms-trentservice.json")`),
//! and a key in any other unusable state `KMSInvalidStateException`. The refusal is the read's
//! half of the relay a WRITE already makes for the same key state
//! ([`super::sse_kms_relay_error_class`]), it is scoped to the key the object names, and it
//! lifts the moment the key's state runs cryptographic operations again.
//!
//! A read that serves NO bytes asks the key nothing: "a HEAD request has the same options as a
//! GET operation on an object" (`HeadObject`'s own documentation) except that it answers no
//! body, and what it reports is S3's own record of the object — so `HeadObject` keeps
//! reporting the object's size and its `SSEKMSKeyId`, and a LISTING keeps listing it, while
//! the key can no longer open it.
//!
//! The tables span what the rule generalises over rather than one recorded read:
//!
//! * the KEY STATE the object's key was moved into after the write — `Disabled` through
//!   KMS's `DisableKey` and `PendingDeletion` through its `ScheduleKeyDeletion`, two
//!   `smithy_query(query="shape",shape="KeyState")` values that raise the two different
//!   exceptions;
//! * the OPERATION doing the reading — `GetObject` whole, `GetObject` of a byte RANGE (a span
//!   of the bytes is still bytes), and the two copies that read a source object's bytes
//!   (`CopyObject`, `UploadPartCopy`);
//! * the stored object's own SIZE, which differs per row, since a key state is not about what
//!   is in the object;
//! * what must stay unaffected beside it — `HeadObject`, `ListObjectsV2`, an object in the
//!   same bucket under a DIFFERENT key that is still enabled, and one stored under SSE-S3,
//!   which names no KMS key at all;
//! * and that the refusal LIFTS after `EnableKey`, with the exact original bytes coming back.
//!
//! Every assertion drives real HTTP requests through a router carrying S3 and KMS on ONE
//! engine ([`super::kms_key_references::router_with_kms`]), and every key state is reached
//! through KMS's own operations: nothing here plants a key or a state in a store.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use super::kms_key_references::{
    copy_object, create_multipart_upload, created_key, described_key_member, kms, router_with_kms,
};
use super::multipart_part_copy::upload_part_copy;
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};
use super::object_encryption::head_object;
use super::object_storage::list_objects_v2;

/// The `ServerSideEncryption` value a KMS key id belongs to, and the one that is SSE-S3 and
/// names no KMS key (`smithy_query(query="shape",shape="ServerSideEncryption")` lists both).
const SSE_KMS: &str = "aws:kms";
const SSE_S3: &str = "AES256";

/// The bound headers a write states its server-side encryption and its KMS key in, and the one
/// a read reports the key in.
const ENCRYPTION_HEADER: &str = "x-amz-server-side-encryption";
const KEY_ID_HEADER: &str = "x-amz-server-side-encryption-aws-kms-key-id";

/// The status every one of these relays carries. Recorded behaviour: the s3 model binds no
/// error shape for the relay, so the status cannot come from it.
const KMS_RELAY_STATUS: u16 = 400;

/// The relayed code for a key that is merely DISABLED, and the one for every other key state
/// that runs no cryptographic operation.
const KMS_DISABLED_CODE: &str = "KMS.DisabledException";
const KMS_INVALID_STATE_CODE: &str = "KMS.KMSInvalidStateException";

/// `length` bytes that differ from every other row's, without anything random: a payload is
/// not what a key state is about, so each row only needs its own.
fn payload(label: u8, length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| label.wrapping_add(index as u8))
        .collect()
}

/// The encryption headers a write naming `key_arn` for SSE-KMS carries.
fn sse_kms_headers(key_arn: &str) -> Vec<(&'static str, &str)> {
    vec![(ENCRYPTION_HEADER, SSE_KMS), (KEY_ID_HEADER, key_arn)]
}

/// One object stored under its own KMS key, whose key was then moved into a state that runs no
/// cryptographic operation.
struct Row {
    /// The case this row stands for, named in every assertion.
    case: &'static str,
    /// The `KeyState` KMS must report for the key once the row is stood up.
    key_state: &'static str,
    /// The key the object is stored under, by ARN — the value the relay's sentence names.
    key_arn: String,
    /// The object's key in the bucket.
    object_key: String,
    /// The exact bytes the write stored, which a served read answers.
    body: Vec<u8>,
    /// The code S3 must relay for this key state, and KMS's own sentence for it.
    code: &'static str,
    message: String,
}

/// What one row asks for: the object to write, and the KMS operation that then moves its key
/// into the state the row is about, with what that state must then answer.
struct Spec {
    /// The case this row stands for, named in every assertion.
    case: &'static str,
    /// The object's key in the bucket, and the exact bytes written at it.
    object_key: &'static str,
    body: Vec<u8>,
    /// The KMS operation that moves the key into the state, and the members it needs beside
    /// `KeyId`.
    operation: &'static str,
    extra: serde_json::Value,
    /// The `KeyState` KMS must then report, the code S3 must relay for it, and the state as
    /// KMS's own sentence words it.
    key_state: &'static str,
    code: &'static str,
    prose: &'static str,
}

/// Write one object under a key of its own, confirm it reads back, then move that key into the
/// state the row is about through KMS's own operation and confirm KMS reports it.
async fn row(router: &MultiServiceRouter, name: &str, spec: Spec) -> Row {
    let Spec {
        case,
        object_key,
        body,
        operation,
        extra,
        key_state,
        code,
        prose,
    } = spec;
    let (key_id, key_arn) = created_key(router, case).await;
    let written = put_object(router, name, object_key, &body, &sse_kms_headers(&key_arn)).await;
    assert_eq!(
        written.status, 200,
        "case {case:?}: the object must be stored under its key first, got {} with body {:?}",
        written.status,
        text(&written)
    );
    assert_eq!(
        written.header(KEY_ID_HEADER).as_deref(),
        Some(key_arn.as_str()),
        "case {case:?}: the write must report the key ARN {key_arn:?}, got {:?}",
        written.header(KEY_ID_HEADER)
    );

    // The row's control, taken while the key is still enabled: the read serves the bytes.
    let served = get_object(router, name, object_key, "", &[]).await;
    assert_eq!(
        (served.status, served.body.as_ref()),
        (200, body.as_slice()),
        "case {case:?}: the read must serve the stored bytes while the key is enabled, got {} \
         with body {:?}",
        served.status,
        text(&served)
    );

    // The key is moved into the row's state through KMS's OWN operation, named here with
    // whatever member that operation needs beside the key.
    let mut request = json!({"KeyId": key_id});
    for (member, value) in extra.as_object().into_iter().flatten() {
        request[member] = value.clone();
    }
    let (status, payload) = kms(router, operation, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} must succeed, got {status} with {payload}"
    );
    let reported = described_key_member(router, &key_arn, "KeyState").await;
    assert_eq!(
        reported, key_state,
        "case {case:?}: KMS must report the key state {key_state:?}, got {reported:?}"
    );

    let message = format!("{key_arn} is {prose}.");
    Row {
        case,
        key_state,
        key_arn,
        object_key: object_key.to_string(),
        body,
        code,
        message,
    }
}

/// The rows: one per key state a key an object is already stored under can be moved into that
/// runs no cryptographic operation, each with its own object and its own payload size.
async fn rows(router: &MultiServiceRouter, name: &str) -> Vec<Row> {
    vec![
        row(
            router,
            name,
            Spec {
                case: "a key that was disabled after the write",
                object_key: "disabled/ledger.bin",
                body: payload(7, 24),
                operation: "DisableKey",
                extra: json!({}),
                key_state: "Disabled",
                code: KMS_DISABLED_CODE,
                prose: "disabled",
            },
        )
        .await,
        row(
            router,
            name,
            Spec {
                case: "a key whose deletion was scheduled after the write",
                object_key: "pending/ledger.bin",
                body: payload(131, 4096),
                operation: "ScheduleKeyDeletion",
                extra: json!({"PendingWindowInDays": 7_i32}),
                key_state: "PendingDeletion",
                code: KMS_INVALID_STATE_CODE,
                prose: "pending deletion",
            },
        )
        .await,
    ]
}

/// Every read that serves the row's object's BYTES, as `(operation, answer)`: the whole
/// object, a byte range of it, and the two copies that read a source object.
async fn byte_reads<'a>(
    router: &'a MultiServiceRouter,
    name: &'a str,
    row: &'a Row,
) -> Vec<(&'static str, super::bucket_lifecycle::Answer)> {
    let copy_source = format!("/{name}/{}", row.object_key);
    let upload = create_multipart_upload(
        router,
        name,
        &format!("copies/{}-part.bin", row.key_state),
        &[],
    )
    .await;
    assert_eq!(
        upload.status, 200,
        "case {:?}: the upload a part copy copies into must be initiated, got {} with body {:?}",
        row.case,
        upload.status,
        text(&upload)
    );
    let upload_id = upload.element("UploadId").unwrap_or_else(|| {
        panic!(
            "case {:?}: an initiation reports an UploadId, got {:?}",
            row.case,
            text(&upload)
        )
    });

    vec![
        (
            "GetObject",
            get_object(router, name, &row.object_key, "", &[]).await,
        ),
        (
            "GetObject of a byte range",
            get_object(
                router,
                name,
                &row.object_key,
                "",
                &[("range", "bytes=0-3")],
            )
            .await,
        ),
        (
            "CopyObject",
            copy_object(
                router,
                name,
                &format!("copies/{}-copy.bin", row.key_state),
                &copy_source,
                &[],
            )
            .await,
        ),
        (
            "UploadPartCopy",
            upload_part_copy(
                router,
                name,
                &format!("copies/{}-part.bin", row.key_state),
                &upload_id,
                1,
                &copy_source,
                &[],
            )
            .await,
        ),
    ]
}

#[tokio::test]
async fn a_read_of_an_sse_kms_object_is_refused_while_its_key_runs_no_cryptographic_operation() {
    let router = router_with_kms();
    let name = "sse-kms-read-key-state";
    bucket(&router, name).await;

    // The two objects that must stay readable throughout: one under ANOTHER key, which stays
    // enabled, and one under SSE-S3, which names no KMS key at all.
    let (_, sibling_key_arn) = created_key(&router, "the sibling key that stays enabled").await;
    let sibling_body = payload(200, 48);
    let sibling = put_object(
        &router,
        name,
        "sibling.bin",
        &sibling_body,
        &sse_kms_headers(&sibling_key_arn),
    )
    .await;
    assert_eq!(
        sibling.status, 200,
        "the sibling object must be stored, got {} with body {:?}",
        sibling.status,
        text(&sibling)
    );
    let managed_body = payload(3, 16);
    let managed = put_object(
        &router,
        name,
        "sse-s3.bin",
        &managed_body,
        &[(ENCRYPTION_HEADER, SSE_S3)],
    )
    .await;
    assert_eq!(
        managed.status, 200,
        "the SSE-S3 object must be stored, got {} with body {:?}",
        managed.status,
        text(&managed)
    );

    for row in rows(&router, name).await {
        let case = row.case;
        for (operation, refused) in byte_reads(&router, name, &row).await {
            assert_eq!(
                refused.status, KMS_RELAY_STATUS,
                "case {case:?}/{operation}: expected HTTP {KMS_RELAY_STATUS}, got {} with body \
                 {:?}",
                refused.status,
                text(&refused)
            );
            assert_eq!(
                refused.error_code(),
                row.code,
                "case {case:?}/{operation}: expected the code {:?}, got {:?} ({:?})",
                row.code,
                refused.error_code(),
                text(&refused)
            );
            assert_eq!(
                refused.error_message(),
                row.message,
                "case {case:?}/{operation}: expected KMS's own sentence {:?}, got {:?}",
                row.message,
                refused.error_message()
            );
            assert!(
                !refused.body.as_ref().ends_with(&row.body),
                "case {case:?}/{operation}: a refused read must carry no plaintext, got {:?}",
                text(&refused)
            );
        }

        // A refused copy wrote nothing at its destination.
        let destination = format!("copies/{}-copy.bin", row.key_state);
        let absent = get_object(&router, name, &destination, "", &[]).await;
        assert_eq!(
            absent.status, 404,
            "case {case:?}: the refused copy must leave nothing at {destination:?}, got {} with \
             body {:?}",
            absent.status,
            text(&absent)
        );

        // A read that serves NO bytes asks the key nothing: the HEAD keeps reporting the
        // object's size and the key it is stored under.
        let headed = head_object(&router, name, &row.object_key).await;
        assert_eq!(
            headed.status, 200,
            "case {case:?}: HeadObject must keep succeeding, got {} with body {:?}",
            headed.status,
            text(&headed)
        );
        assert_eq!(
            headed.header(KEY_ID_HEADER).as_deref(),
            Some(row.key_arn.as_str()),
            "case {case:?}: HeadObject must keep reporting the key ARN {:?}, got {:?}",
            row.key_arn,
            headed.header(KEY_ID_HEADER)
        );
        assert_eq!(
            headed.header("content-length"),
            Some(row.body.len().to_string()),
            "case {case:?}: HeadObject must keep reporting the object's length {}, got {:?}",
            row.body.len(),
            headed.header("content-length")
        );

        // And the object stays listable.
        let listed = list_objects_v2(&router, name, "").await;
        assert!(
            listed.elements("Key").contains(&row.object_key),
            "case {case:?}: the listing must keep listing {:?}, got {:?}",
            row.object_key,
            listed.elements("Key")
        );

        // The refusal is scoped to the row's key: the object under the still-enabled key and
        // the SSE-S3 object keep answering their exact bytes.
        for (unaffected, expected) in [
            ("sibling.bin", &sibling_body),
            ("sse-s3.bin", &managed_body),
        ] {
            let read = get_object(&router, name, unaffected, "", &[]).await;
            assert_eq!(
                (read.status, read.body.as_ref()),
                (200, expected.as_slice()),
                "case {case:?}: {unaffected:?} must keep answering its exact bytes, got {} with \
                 body {:?}",
                read.status,
                text(&read)
            );
        }
    }
}

#[tokio::test]
async fn a_read_refused_for_the_key_state_is_served_again_once_the_state_runs_operations() {
    // The refusal is about the key STATE, not about the object: the one object is refused while
    // its key is disabled and answers its exact original bytes once `EnableKey` restores it.
    let router = router_with_kms();
    let name = "sse-kms-read-key-state-restored";
    bucket(&router, name).await;
    let (key_id, key_arn) = created_key(&router, "the key disabled and then enabled again").await;
    let body = payload(61, 64);
    let written = put_object(
        &router,
        name,
        "restored.bin",
        &body,
        &sse_kms_headers(&key_arn),
    )
    .await;
    assert_eq!(
        written.status, 200,
        "the object must be stored first, got {} with body {:?}",
        written.status,
        text(&written)
    );

    let (turned_off, payload_off) = kms(&router, "DisableKey", &json!({"KeyId": key_id})).await;
    assert_eq!(
        turned_off, 200,
        "case disabled: disabling the key must succeed, got {turned_off} with {payload_off}"
    );
    let refused = get_object(&router, name, "restored.bin", "", &[]).await;
    assert_eq!(
        refused.status, KMS_RELAY_STATUS,
        "case disabled: expected HTTP {KMS_RELAY_STATUS}, got {} with body {:?}",
        refused.status,
        text(&refused)
    );
    assert_eq!(
        refused.error_code(),
        KMS_DISABLED_CODE,
        "case disabled: expected the code {KMS_DISABLED_CODE:?}, got {:?} ({:?})",
        refused.error_code(),
        text(&refused)
    );
    assert_eq!(
        refused.error_message(),
        format!("{key_arn} is disabled."),
        "case disabled: expected KMS's own sentence for the state, got {:?}",
        refused.error_message()
    );

    let (turned_on, payload_on) = kms(&router, "EnableKey", &json!({"KeyId": key_id})).await;
    assert_eq!(
        turned_on, 200,
        "case enabled: enabling the key must succeed, got {turned_on} with {payload_on}"
    );
    let served = get_object(&router, name, "restored.bin", "", &[]).await;
    assert_eq!(
        (served.status, served.body.as_ref()),
        (200, body.as_slice()),
        "case enabled: the identical read must answer the exact original bytes, got {} with body \
         {:?}",
        served.status,
        text(&served)
    );
    assert_eq!(
        served.header(KEY_ID_HEADER).as_deref(),
        Some(key_arn.as_str()),
        "case enabled: the served read must report the key ARN {key_arn:?}, got {:?}",
        served.header(KEY_ID_HEADER)
    );
}
