// SPDX-License-Identifier: CC0-1.0

//! What a handler MOVING BYTES can do with the AWS SDK the runtime supplies: Lambda, IAM, the
//! token service and Amazon S3 on ONE engine, a Python function that assembles an object from
//! parts at the size Amazon S3 accepts for one and round-trips objects through the SDK's own
//! managed transfer, with the bytes the store holds checked against the bytes the handler
//! sent.
//!
//! RULE: an object's bytes are what a call of this seam carries, in both directions, so a call
//! carrying any payload the called operation accepts is dispatched and answered — the parts of
//! a multipart upload at the smallest size S3 accepts for one ("each part must be at least 5
//! MB in size, except the last part", `UploadPart`) included — and a MANAGED transfer is the
//! calls the transfer library makes for the size it is handed: one `PutObject` or `GetObject`
//! for an object inside the configured threshold, and a multipart upload for one over it,
//! round-tripping the bytes exactly either way.
//!
//! The rows differ along the SIZE of the payload: nothing at all, a handful of bytes, just
//! past the smallest document bound a call could have, well past it, and past the transfer's
//! own threshold — the configured one and the SDK's own default — and, for the multipart half,
//! parts at the floor S3 judges a part by and a last part below it, which is the part that
//! floor exempts.
//!
//! The suite RUNS a handler, so it takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]).

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mapping_role_access::given_role;
use crate::event_source_mappings::{await_function_active, call, router_on};

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The Region the function runs in and the bucket is created in.
const REGION: &str = "us-east-1";

/// The bucket the handler stores through.
const BUCKET: &str = "handler-payload-bucket";

/// The smallest part Amazon S3 accepts for any but the last part of a multipart upload: "Part
/// size: 5 MiB to 5 GiB. There is no minimum size limit on the last part of your multipart
/// upload" (Amazon S3 multipart upload limits).
const MIN_PART_BYTES: usize = 5 * 1024 * 1024;

/// The sizes the parts of the object this suite assembles have: the floor above, and a last
/// part far below it — the one the floor exempts.
const PART_SIZES: [usize; 2] = [MIN_PART_BYTES, 11];

/// One transfer: what the row is called, the key it writes, the size of the object, the
/// configuration the transfer is run under and how many parts the stored object is assembled
/// from.
struct Transferred {
    /// What the row is called in a failure.
    case: &'static str,
    /// The key the object is written under.
    key: &'static str,
    /// How many bytes the object holds.
    size: usize,
    /// The transfer configuration the row states, as `(multipart_threshold,
    /// multipart_chunksize)`, or `None` for the SDK's own defaults — 8 MiB for each
    /// (`boto3.s3.transfer.TransferConfig`).
    config: Option<(usize, usize)>,
    /// How many parts the stored object was assembled from, which the tag of a multipart
    /// object names (`"<digest>-<parts>"`), or `None` for an object one call stored whole.
    parts: Option<u64>,
}

/// The transfers this suite drives, one row per size the rule generalises over.
fn transferred() -> Vec<Transferred> {
    vec![
        Transferred {
            case: "an object holding no bytes at all",
            key: "transfers/empty.bin",
            size: 0,
            config: None,
            parts: None,
        },
        Transferred {
            case: "a handful of bytes",
            key: "transfers/small.bin",
            size: 11,
            config: None,
            parts: None,
        },
        Transferred {
            case: "one byte past a mebibyte, which no bound of this seam may stop",
            key: "transfers/past-a-mebibyte.bin",
            size: 1024 * 1024 + 1,
            config: None,
            parts: None,
        },
        Transferred {
            case: "several mebibytes, still inside the transfer's own threshold",
            key: "transfers/several-mebibytes.bin",
            size: 3 * 1024 * 1024,
            config: None,
            parts: None,
        },
        Transferred {
            case: "past a threshold the caller stated, so the transfer is a multipart upload",
            key: "transfers/past-a-stated-threshold.bin",
            size: 6 * 1024 * 1024,
            config: Some((MIN_PART_BYTES, MIN_PART_BYTES)),
            parts: Some(2),
        },
        Transferred {
            case: "past the SDK's own default threshold, under no configuration at all",
            key: "transfers/past-the-default-threshold.bin",
            size: 9 * 1024 * 1024,
            config: None,
            parts: Some(2),
        },
    ]
}

/// The key the assembled object is written under.
const ASSEMBLED_KEY: &str = "assembled/from-parts.bin";

/// Lambda, IAM, the token service and Amazon S3 over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = router_on(&engine);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router
}

/// One request to Amazon S3, made as a CLIENT makes one, answering the bytes it served.
async fn s3(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: &[u8],
) -> (u16, Vec<u8>) {
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/s3/\
                 aws4_request, SignedHeaders=host, Signature=not-verified"
            ),
        )
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    (response.status().as_u16(), response.body().to_vec())
}

/// Create the bucket the handler stores through.
async fn given_bucket(router: &MultiServiceRouter, case: &str) {
    let (status, document) = s3(router, "PUT", &format!("/{BUCKET}"), b"").await;
    assert_eq!(
        status,
        200,
        "case {case:?}: expected CreateBucket to answer 200, got {status}: {}",
        String::from_utf8_lossy(&document)
    );
}

/// The bytes one part of the assembled object holds: the part's own number, repeated.
///
/// The same rule the handler fills a part with, so the bytes the store ends up holding can be
/// compared with the bytes the handler sent without carrying them through the invocation's own
/// answer.
fn part_bytes(number: usize, size: usize) -> Vec<u8> {
    vec![u8::try_from(number % 256).unwrap_or_default(); size]
}

/// The bytes one transferred object holds: every byte value in turn, cut to the size.
///
/// The same rule the handler fills the object with, for the same reason.
fn transfer_bytes(size: usize) -> Vec<u8> {
    (0..size)
        .map(|at| u8::try_from(at % 256).unwrap_or_default())
        .collect()
}

/// The handler this suite runs: the multipart assembly the event describes, and one transfer
/// round trip per row it names.
const HANDLER_SOURCE: &str = r#"
import hashlib
import io

import boto3


def _part_bytes(number, size):
    """The bytes one part holds: the part's own number, repeated."""
    return bytes([number % 256]) * size


def _object_bytes(size):
    """The bytes one transferred object holds: every byte value in turn, cut to the size.

    Built by repeating one pattern rather than by walking the range: what is being measured
    is the transfer, not how fast the handler can fill a buffer.
    """
    if size <= 0:
        return b""
    pattern = bytes(range(256))
    return (pattern * (size // 256 + 1))[:size]


def _assembled(client, bucket, key, sizes):
    """One object stored as several parts, and what the service says about the result."""
    started = client.create_multipart_upload(Bucket=bucket, Key=key)
    upload = started["UploadId"]
    parts = []
    for number, size in enumerate(sizes, start=1):
        stored = client.upload_part(
            Bucket=bucket,
            Key=key,
            UploadId=upload,
            PartNumber=number,
            Body=_part_bytes(number, size),
        )
        parts.append({"PartNumber": number, "ETag": stored["ETag"]})
    listed = client.list_parts(Bucket=bucket, Key=key, UploadId=upload)
    completed = client.complete_multipart_upload(
        Bucket=bucket,
        Key=key,
        UploadId=upload,
        MultipartUpload={"Parts": parts},
    )
    read = client.head_object(Bucket=bucket, Key=key)
    return {
        "upload_id_carried": bool(upload),
        "parts_sent": len(parts),
        "parts_listed": [
            [one.get("PartNumber"), one.get("Size")] for one in listed.get("Parts", [])
        ],
        "listed_key": listed.get("Key"),
        "etag": completed.get("ETag"),
        "length": read.get("ContentLength"),
        "digest": hashlib.sha256(
            b"".join(_part_bytes(number, size) for number, size in enumerate(sizes, start=1))
        ).hexdigest(),
    }


def _transferred(client, bucket, key, size, config):
    """One object written and read back through the SDK's own managed transfer."""
    transfer = None
    if config is not None:
        from boto3.s3.transfer import TransferConfig

        transfer = TransferConfig(
            multipart_threshold=config["threshold"],
            multipart_chunksize=config["chunksize"],
        )
    payload = _object_bytes(size)
    client.upload_fileobj(io.BytesIO(payload), bucket, key, Config=transfer)
    buffered = io.BytesIO()
    client.download_fileobj(bucket, key, buffered, Config=transfer)
    read = buffered.getvalue()
    head = client.head_object(Bucket=bucket, Key=key)
    return {
        "round_tripped": read == payload,
        "length": len(read),
        "stored_length": head.get("ContentLength"),
        "etag": head.get("ETag"),
        "digest": hashlib.sha256(read).hexdigest(),
        "expected_digest": hashlib.sha256(payload).hexdigest(),
    }


def lambda_handler(event, context):
    client = boto3.client("s3")
    bucket = event["bucket"]
    return {
        "assembled": _assembled(
            client, bucket, event["assembled_key"], event["part_sizes"]
        ),
        "transfers": [
            {
                "case": row["case"],
                "read": _transferred(
                    client, bucket, row["key"], row["size"], row.get("config")
                ),
            }
            for row in event["transfers"]
        ],
    }
"#;

/// Create the function `name`, running as `role_arn`.
async fn given_function(router: &MultiServiceRouter, case: &str, name: &str, role_arn: &str) {
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": role_arn,
            "Runtime": "python3.12",
            "Handler": "lambda_function.lambda_handler",
            "Timeout": 120,
            "MemorySize": 512,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(crate::synchronous_invoke_run::package(HANDLER_SOURCE)),
            },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {case:?}: expected CreateFunction to answer 201, got {} {}",
        created.status,
        created.text()
    );
    await_function_active(router, case, name).await;
}

/// A handler assembling an object from parts and round-tripping objects through the managed
/// transfer: every call reaches Amazon S3, and the bytes the store holds are the bytes the
/// handler sent.
#[tokio::test]
async fn a_handler_moves_an_objects_bytes_whatever_their_size() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    let case = "a handler moving an object's bytes";
    let router = emulator();
    given_bucket(&router, case).await;
    let role = given_role(
        &router,
        case,
        "handler-payload-role",
        Some(json!([{ "Effect": "Allow", "Action": "s3:*", "Resource": "*" }])),
    )
    .await;
    let function = "handler-payload-mover";
    given_function(&router, case, function, &role).await;
    let rows = transferred();
    let answered = call(
        &router,
        "POST",
        &format!("{FUNCTIONS_URI}/{function}/invocations"),
        &json!({
            "bucket": BUCKET,
            "assembled_key": ASSEMBLED_KEY,
            "part_sizes": PART_SIZES,
            "transfers": rows
                .iter()
                .map(|row| json!({
                    "case": row.case,
                    "key": row.key,
                    "size": row.size,
                    "config": row.config.map(|(threshold, chunksize)| json!({
                        "threshold": threshold,
                        "chunksize": chunksize,
                    })),
                }))
                .collect::<Vec<Value>>(),
        }),
    )
    .await;
    assert_eq!(
        answered.status,
        200,
        "case {case:?}: expected HTTP 200 from Invoke, got {} {}",
        answered.status,
        answered.text()
    );
    assert_eq!(
        answered.header("X-Amz-Function-Error"),
        None,
        "case {case:?}: expected the handler to run without a function error, got {}",
        answered.text()
    );
    let report = answered.json();

    // The assembled object: every part was stored, the upload reported them back at the sizes
    // they were sent at, and the object the completion made carries their bytes end to end.
    let assembled = &report["assembled"];
    let expected: Vec<Vec<u8>> = PART_SIZES
        .iter()
        .enumerate()
        .map(|(at, size)| part_bytes(at + 1, *size))
        .collect();
    let whole: Vec<u8> = expected.concat();
    assert_eq!(
        assembled["parts_sent"],
        json!(PART_SIZES.len()),
        "case {case:?}: expected every part to be sent, got {assembled}"
    );
    assert_eq!(
        assembled["parts_listed"],
        json!(PART_SIZES
            .iter()
            .enumerate()
            .map(|(at, size)| json!([at + 1, size]))
            .collect::<Vec<Value>>()),
        "case {case:?}: expected the upload to report every part at the size it was sent at, \
         got {assembled}"
    );
    assert_eq!(
        assembled["listed_key"],
        json!(ASSEMBLED_KEY),
        "case {case:?}: expected the parts listing to name the object, got {assembled}"
    );
    assert_eq!(
        assembled["length"],
        json!(whole.len()),
        "case {case:?}: expected the assembled object to hold every part's bytes, got \
         {assembled}"
    );
    let (status, served) = s3(&router, "GET", &format!("/{BUCKET}/{ASSEMBLED_KEY}"), b"").await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the store to serve the assembled object, got {status}"
    );
    assert_eq!(
        served.len(),
        whole.len(),
        "case {case:?}: expected the store to hold {} bytes, got {}",
        whole.len(),
        served.len()
    );
    assert!(
        served == whole,
        "case {case:?}: expected the store to hold the parts' own bytes in order"
    );

    // The transfers: each round trip gave the handler back exactly what it sent, and the
    // object the store holds is that long.
    let reports = report["transfers"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected one report per row, got {report}"));
    assert_eq!(
        reports.len(),
        rows.len(),
        "case {case:?}: expected one report per row, got {report}"
    );
    for (row, answered) in rows.iter().zip(reports) {
        let read = &answered["read"];
        assert_eq!(
            read["round_tripped"],
            json!(true),
            "case {:?}: expected the transfer to give back the bytes it was given, got {read}",
            row.case
        );
        assert_eq!(
            read["digest"], read["expected_digest"],
            "case {:?}: expected the digest of the bytes sent, got {read}",
            row.case
        );
        assert_eq!(
            read["length"],
            json!(row.size),
            "case {:?}: expected {} bytes back, got {read}",
            row.case,
            row.size
        );
        assert_eq!(
            read["stored_length"],
            json!(row.size),
            "case {:?}: expected the store to hold {} bytes, got {read}",
            row.case,
            row.size
        );
        // WHICH calls the transfer made, as the stored object's own tag reports it: the tag of
        // an object assembled from parts names how many it was assembled from, and the tag of
        // one stored whole names none.
        let etag = read["etag"].as_str().unwrap_or_default();
        let bare = etag.trim_matches('"');
        match row.parts {
            Some(parts) => assert!(
                bare.ends_with(&format!("-{parts}")),
                "case {:?}: expected the tag of an object assembled from {parts} parts, got \
                 {etag:?}",
                row.case
            ),
            None => assert!(
                !bare.contains('-'),
                "case {:?}: expected the tag of an object one call stored whole, got {etag:?}",
                row.case
            ),
        }
        let (status, served) = s3(&router, "GET", &format!("/{BUCKET}/{}", row.key), b"").await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the store to serve the object, got {status}",
            row.case
        );
        assert!(
            served == transfer_bytes(row.size),
            "case {:?}: expected the store to hold the bytes the handler sent, got {} bytes",
            row.case,
            served.len()
        );
    }
}
