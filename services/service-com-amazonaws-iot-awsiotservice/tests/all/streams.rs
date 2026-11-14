// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the AWS IoT STREAM store: `CreateStream`, the
//! `DescribeStream` reader that proves what it persisted, and `DeleteStream`.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `iot` signing name — so what is asserted is what
//! an AWS SDK client would see, not an internal function's return value.
//!
//! The rules under test:
//!
//! * a stream is created under its id with the files it serves, and a later read
//!   reports that record member for member at version 1;
//! * a stream is deleted on its OWN terms: an OTA update that names it holds nothing
//!   back, and the update keeps the `streamId` it was created with, because the
//!   reference lives on the update's record and not on the stream's;
//! * an id the account holds no stream under is `ResourceNotFoundException` (404), so
//!   a second delete of the same stream is refused;
//! * a file entry naming no S3 object names no file at all, which is
//!   `InvalidRequestException` (400) with nothing created.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region this suite signs for; the ARN assertions read it back.
const REGION: &str = "ca-central-1";
/// The role IoT reads a stream's S3 files under: its form is the caller's, so one
/// well-formed ARN is used throughout.
const STREAM_ROLE: &str = "arn:aws:iam::000000000000:role/IoTStreamRole";

/// One emulator: the shared engine plus a router with this service mounted.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn dispatch(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let host = format!("iot.{REGION}.amazonaws.com");
        let payload = body.map_or_else(Bytes::new, |value| Bytes::from(value.to_string()));
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/iot/aws4_request, \
                     SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(payload)
            .expect("the stream request is well formed");
        Reply::of(self.router.dispatch(request).await)
    }

    async fn create_stream(&self, stream_id: &str, body: &Value) -> Reply {
        self.dispatch("POST", &format!("/streams/{stream_id}"), Some(body))
            .await
    }

    async fn describe_stream(&self, stream_id: &str) -> Reply {
        self.dispatch("GET", &format!("/streams/{stream_id}"), None)
            .await
    }

    async fn delete_stream(&self, stream_id: &str) -> Reply {
        self.dispatch("DELETE", &format!("/streams/{stream_id}"), None)
            .await
    }

    async fn create_thing(&self, thing_name: &str) -> Reply {
        self.dispatch("POST", &format!("/things/{thing_name}"), Some(&json!({})))
            .await
    }

    async fn create_ota_update(&self, ota_update_id: &str, body: &Value) -> Reply {
        self.dispatch("POST", &format!("/otaUpdates/{ota_update_id}"), Some(body))
            .await
    }

    async fn get_ota_update(&self, ota_update_id: &str) -> Reply {
        self.dispatch("GET", &format!("/otaUpdates/{ota_update_id}"), None)
            .await
    }
}

/// A dispatched response, reduced to what the assertions read.
struct Reply {
    status: u16,
    body: Bytes,
}

impl Reply {
    fn of(response: http::Response<Bytes>) -> Self {
        Self {
            status: response.status().as_u16(),
            body: response.into_body(),
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("response body is not JSON ({e}): {}", self.text()))
    }

    fn error_code(&self) -> String {
        self.json()["__type"]
            .as_str()
            .unwrap_or_else(|| panic!("no __type in {}", self.text()))
            .to_string()
    }

    fn error_message(&self) -> String {
        self.json()["message"]
            .as_str()
            .unwrap_or_else(|| panic!("no message in {}", self.text()))
            .to_string()
    }
}

/// One file of a stream: the id a device asks for it by and the S3 object it is.
fn stream_file(file_id: i64, key: &str) -> Value {
    json!({
        "fileId": file_id,
        "s3Location": { "bucket": "firmware-images", "key": key, "version": "1" },
    })
}

/// RULE: a created stream is read back member for member, at version 1.
#[tokio::test]
async fn a_created_stream_is_read_back_member_for_member() {
    let emulator = Emulator::new();

    // The rows span the form and length of a stream id (modelled
    // `length 1..128, pattern ^[a-zA-Z0-9_-]+$`), how many files the stream serves
    // (the modelled `files` length is 1..50), and which optional members the request
    // carries.
    let rows: Vec<(&str, String, Value)> = vec![
        (
            "one file, nothing optional",
            "firmware".to_string(),
            json!({
                "files": [stream_file(0, "firmware.bin")],
                "roleArn": STREAM_ROLE,
            }),
        ),
        (
            "three files, a description and tags",
            "firmware_v2-RC".to_string(),
            json!({
                "files": [
                    stream_file(0, "boot.bin"),
                    stream_file(1, "kernel.bin"),
                    stream_file(255, "rootfs.img"),
                ],
                "roleArn": STREAM_ROLE,
                "description": "the second firmware release",
                "tags": [{ "Key": "release", "Value": "rc" }],
            }),
        ),
        (
            "the longest id the model admits",
            "s".repeat(128),
            json!({
                "files": [stream_file(7, "firmware.bin")],
                "roleArn": STREAM_ROLE,
            }),
        ),
    ];

    for (case, stream_id, body) in &rows {
        let created = emulator.create_stream(stream_id, body).await;
        assert_eq!(
            created.status,
            200,
            "case {case}: expected 200, got {} body {}",
            created.status,
            created.text()
        );
        let expected_arn = format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:stream/{stream_id}");
        assert_eq!(
            created.json()["streamArn"],
            json!(expected_arn),
            "case {case}: expected the minted ARN, got {}",
            created.text()
        );
        assert_eq!(
            created.json()["streamVersion"],
            json!(1),
            "case {case}: a stream that has had one write is at version 1, got {}",
            created.text()
        );

        let read = emulator.describe_stream(stream_id).await;
        assert_eq!(
            read.status,
            200,
            "case {case}: expected the read to answer 200, got {} body {}",
            read.status,
            read.text()
        );
        let info = read.json()["streamInfo"].clone();
        assert_eq!(
            info["streamId"],
            json!(stream_id),
            "case {case}: the read must name the stream, got {info}"
        );
        assert_eq!(
            info["streamArn"],
            json!(expected_arn),
            "case {case}: the read must report the ARN the create minted, got {info}"
        );
        assert_eq!(
            info["files"], body["files"],
            "case {case}: the read must report the files the writer sent, got {info}"
        );
        assert_eq!(
            info["roleArn"], body["roleArn"],
            "case {case}: the read must report the stored role, got {info}"
        );
        assert_eq!(
            info["description"],
            body.get("description").cloned().unwrap_or(Value::Null),
            "case {case}: the read must report the stored description, got {info}"
        );
        assert!(
            info["createdAt"].is_number() && info["lastUpdatedAt"].is_number(),
            "case {case}: the read must report when the stream was created, got {info}"
        );
    }

    // The id is the stream's identity, so a second create under it is a conflict and
    // the record the first create wrote is left alone.
    let conflict = emulator
        .create_stream(
            "firmware",
            &json!({ "files": [stream_file(9, "other.bin")], "roleArn": STREAM_ROLE }),
        )
        .await;
    assert_eq!(
        conflict.status,
        409,
        "case an id already taken: expected 409, got {} body {}",
        conflict.status,
        conflict.text()
    );
    let unchanged = emulator.describe_stream("firmware").await.json();
    assert_eq!(
        unchanged["streamInfo"]["files"],
        json!([stream_file(0, "firmware.bin")]),
        "case an id already taken: the stored stream must be untouched, got {unchanged}"
    );
}

/// RULE: a stream is deleted on its own terms — an OTA update that names it holds
/// nothing back and keeps the id it was created with — and a second delete is refused.
#[tokio::test]
async fn a_stream_is_deleted_even_while_an_ota_update_names_it() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator
            .create_stream(
                "firmware",
                &json!({ "files": [stream_file(0, "firmware.bin")], "roleArn": STREAM_ROLE })
            )
            .await
            .status,
        200,
        "the stream the update names must be creatable"
    );
    assert_eq!(emulator.create_thing("sensor-a").await.status, 200);

    let deployed = emulator
        .create_ota_update(
            "firmware-rollout",
            &json!({
                "targets": [format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:thing/sensor-a")],
                "files": [{
                    "fileName": "firmware.bin",
                    "fileLocation": { "stream": { "streamId": "firmware", "fileId": 0 } },
                }],
                "roleArn": STREAM_ROLE,
            }),
        )
        .await;
    assert_eq!(
        deployed.status,
        200,
        "case an update naming the stream: expected 200, got {} body {}",
        deployed.status,
        deployed.text()
    );

    // The delete is no reference count: the stream goes, while the update keeps the id.
    let deleted = emulator.delete_stream("firmware").await;
    assert_eq!(
        deleted.status,
        200,
        "case deleting a stream an update names: expected 200, got {} body {}",
        deleted.status,
        deleted.text()
    );
    let update = emulator.get_ota_update("firmware-rollout").await;
    assert_eq!(
        update.status,
        200,
        "case after the delete: the update must still be readable, got {} body {}",
        update.status,
        update.text()
    );
    let described = update.json()["otaUpdateInfo"].clone();
    assert_eq!(
        described["otaUpdateFiles"][0]["fileLocation"]["stream"]["streamId"],
        json!("firmware"),
        "case after the delete: the update keeps the streamId it was created with, got {described}"
    );

    // The stream itself is gone, so reading or deleting it again is the same 404.
    for (case, reply) in [
        ("a read of the deleted stream", emulator.describe_stream("firmware").await),
        ("a second delete", emulator.delete_stream("firmware").await),
        ("a stream that never existed", emulator.delete_stream("never-existed").await),
    ] {
        assert_eq!(
            reply.status,
            404,
            "case {case}: expected 404, got {} body {}",
            reply.status,
            reply.text()
        );
        assert_eq!(
            reply.error_code(),
            "ResourceNotFoundException",
            "case {case}: got {}",
            reply.text()
        );
    }
    assert_eq!(
        emulator.describe_stream("firmware").await.error_message(),
        format!("Requested stream firmware does not exist in given account {ACCOUNT_ID}"),
        "case a read of the deleted stream: the refusal must state the miss"
    );
}

/// RULE: a file entry that names no S3 object names no file, so the stream would serve
/// nothing and the create is refused.
#[tokio::test]
async fn a_file_that_names_no_object_is_refused() {
    let emulator = Emulator::new();

    // The rows leave out a different part of the object's name each: the location
    // altogether, the bucket, the key, and each of them empty.
    let rows: Vec<(&str, Value)> = vec![
        ("no s3Location at all", json!([{ "fileId": 0 }])),
        (
            "no bucket",
            json!([{ "fileId": 0, "s3Location": { "key": "firmware.bin" } }]),
        ),
        (
            "no key",
            json!([{ "fileId": 0, "s3Location": { "bucket": "firmware-images" } }]),
        ),
        (
            "a later file naming no object",
            json!([
                stream_file(0, "firmware.bin"),
                { "fileId": 1, "s3Location": {} },
            ]),
        ),
    ];

    for (case, files) in &rows {
        let refused = emulator
            .create_stream("refused", &json!({ "files": files, "roleArn": STREAM_ROLE }))
            .await;
        assert_eq!(
            refused.status,
            400,
            "case {case}: expected 400, got {} body {}",
            refused.status,
            refused.text()
        );
        assert_eq!(
            refused.error_code(),
            "InvalidRequestException",
            "case {case}: got {}",
            refused.text()
        );
        assert_eq!(
            emulator.describe_stream("refused").await.status,
            404,
            "case {case}: a refused create must leave no stream behind"
        );
    }
}
