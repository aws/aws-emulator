// SPDX-License-Identifier: CC0-1.0

//! What a handler READING a bucket can do with the AWS SDK the runtime supplies: Lambda, IAM,
//! the token service and Amazon S3 on ONE engine, a Python function that walks a listing with
//! the marker the first form of the listing is continued from, reads one object's attributes,
//! and reads every instant those answers carry.
//!
//! Two rules, one per test:
//!
//! * the AWS SDK a managed runtime supplies is generated from the called service's whole
//!   model, so a handler's call on ANY operation that service models and this emulator serves
//!   is built, signed and dispatched to it and answered with that operation's own output
//!   members — including the form of a listing that is continued from a key marker and the
//!   read that reports an object's metadata without its bytes;
//! * a member the called operation's model types as a `timestamp` reaches the handler as its
//!   own language's instant — a timezone-aware `datetime` — whatever the wire carried it as
//!   and wherever the protocol bound it, so a handler can compare it, subtract it and render
//!   it the way it renders any other instant.
//!
//! The rows of the first differ along the page size the walk asks for and the prefix it
//! selects, and along WHICH attributes of the object the read asks for; the rows of the second
//! differ along which operation's answer the instant came from and where in that answer it
//! sits — a member of the top-level answer, and one inside a listing's own entries.
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

/// The bucket the handler reads.
const BUCKET: &str = "handler-listing-bucket";

/// The objects the bucket holds, in the order their keys sort — which is the order a listing
/// reports them in, and so the order a walk must collect them in.
const OBJECTS: [(&str, &str); 5] = [
    ("alpha/one.txt", "one"),
    ("alpha/two.txt", "two"),
    ("beta/three.txt", "three"),
    ("reports/five.txt", "five"),
    ("reports/four.txt", "four"),
];

/// The object whose attributes the handler reads, and the bytes it holds.
const ATTRIBUTED: (&str, &str) = OBJECTS[4];

/// One walk of the listing: what the row is called, the prefix it selects, the page size it
/// asks for, the keys it must collect and how many pages that takes.
struct Walk {
    /// What the row is called in a failure.
    case: &'static str,
    /// The prefix the listing selects, or the empty text for the whole bucket.
    prefix: &'static str,
    /// The page size the walk asks each listing for (`ListObjectsRequest$MaxKeys`).
    page: u64,
    /// The keys the walk must collect, in the order a listing reports them.
    keys: Vec<&'static str>,
    /// How many calls walking them takes at that page size.
    pages: u64,
}

/// The walks this suite drives, one row per page size and prefix the rule generalises over.
fn walks() -> Vec<Walk> {
    let every: Vec<&'static str> = OBJECTS.iter().map(|(key, _)| *key).collect();
    vec![
        Walk {
            case: "a page larger than the bucket, so one call serves the whole listing",
            prefix: "",
            page: 1000,
            keys: every.clone(),
            pages: 1,
        },
        Walk {
            case: "a page of two keys, walked to the end with the marker",
            prefix: "",
            page: 2,
            keys: every.clone(),
            pages: 3,
        },
        Walk {
            case: "a page of one key, which is a call per object",
            prefix: "",
            page: 1,
            keys: every,
            pages: 5,
        },
        Walk {
            case: "a prefix selecting some of the keys, paged",
            prefix: "reports/",
            page: 1,
            keys: vec![OBJECTS[3].0, OBJECTS[4].0],
            pages: 2,
        },
        Walk {
            case: "a prefix the bucket holds no key under",
            prefix: "nothing/",
            page: 2,
            keys: Vec::new(),
            pages: 1,
        },
    ]
}

/// One read of an object's attributes: what the row is called, the attributes it asks for, and
/// which members the answer must then carry and must not.
struct Attributes {
    /// What the row is called in a failure.
    case: &'static str,
    /// The `ObjectAttributes` the read asks for — each a value of that enum
    /// (`smithy_query(query="shape",shape="ObjectAttributes",
    /// model_path="services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json")`:
    /// `ETag`, `Checksum`, `ObjectParts`, `StorageClass`, `ObjectSize`).
    asked: Vec<&'static str>,
    /// The members the answer must carry.
    present: Vec<&'static str>,
    /// The members it must not, because the read did not ask for them.
    absent: Vec<&'static str>,
}

/// The attribute reads this suite drives, one row per set of attributes a caller may ask for.
fn attributes() -> Vec<Attributes> {
    vec![
        Attributes {
            case: "the tag alone",
            asked: vec!["ETag"],
            present: vec!["ETag"],
            absent: vec!["ObjectSize", "StorageClass"],
        },
        Attributes {
            case: "the size alone",
            asked: vec!["ObjectSize"],
            present: vec!["ObjectSize"],
            absent: vec!["ETag", "StorageClass"],
        },
        Attributes {
            case: "the storage class alone",
            asked: vec!["StorageClass"],
            present: vec!["StorageClass"],
            absent: vec!["ETag", "ObjectSize"],
        },
        Attributes {
            case: "every attribute of an object stored in one call",
            asked: vec!["ETag", "ObjectSize", "StorageClass", "Checksum", "ObjectParts"],
            present: vec!["ETag", "ObjectSize", "StorageClass"],
            absent: Vec::new(),
        },
    ]
}

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

/// One request to Amazon S3, made as a CLIENT makes one.
async fn s3(router: &MultiServiceRouter, method: &str, path: &str, body: &[u8]) -> (u16, String) {
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
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// Create the bucket and the objects the handler reads.
async fn given_objects(router: &MultiServiceRouter, case: &str) {
    let (status, document) = s3(router, "PUT", &format!("/{BUCKET}"), b"").await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateBucket to answer 200, got {status}: {document}"
    );
    for (key, body) in OBJECTS {
        let (status, document) =
            s3(router, "PUT", &format!("/{BUCKET}/{key}"), body.as_bytes()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutObject {key} to answer 200, got {status}: {document}"
        );
    }
}

/// Create the function `name` out of `source`, running as `role_arn`.
async fn given_function(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    role_arn: &str,
    source: &str,
) {
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": role_arn,
            "Runtime": "python3.12",
            "Handler": "lambda_function.lambda_handler",
            "Timeout": 60,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(crate::synchronous_invoke_run::package(source)),
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

/// Invoke `name` with `payload`, and answer what the handler returned.
async fn invoked(router: &MultiServiceRouter, case: &str, name: &str, payload: &Value) -> Value {
    let answered = call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/invocations"),
        payload,
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
    answered.json()
}

/// The role every function of this suite runs as: reading a bucket's objects and their
/// attributes, and listing the account's buckets.
fn reader() -> Value {
    json!([{ "Effect": "Allow", "Action": "s3:*", "Resource": "*" }])
}

/// The handler of the first test: the walks and the attribute reads the event names.
const LISTING_SOURCE: &str = r#"
import boto3


def _walk(client, bucket, prefix, page):
    """Every key the FIRST form of the listing reports, walked with its own marker.

    `ListObjects` is continued from a key: the answer names the key to resume at where it has
    one, and otherwise the last key of the page is the marker ("Marker ... Specifies the key
    to start with").
    """
    keys = []
    pages = 0
    marker = None
    while True:
        parameters = {"Bucket": bucket, "MaxKeys": page}
        if prefix:
            parameters["Prefix"] = prefix
        if marker is not None:
            parameters["Marker"] = marker
        read = client.list_objects(**parameters)
        pages += 1
        held = [one["Key"] for one in read.get("Contents", [])]
        keys.extend(held)
        if not read.get("IsTruncated"):
            return {
                "keys": keys,
                "pages": pages,
                "name": read.get("Name"),
                "max_keys": read.get("MaxKeys"),
                "truncated": bool(read.get("IsTruncated")),
            }
        marker = read.get("NextMarker") or (held[-1] if held else None)
        if marker is None:
            return {"keys": keys, "pages": pages, "stalled": True}


def _attributes(client, bucket, key, asked):
    """What one object's attributes say about it, for the attributes asked for."""
    read = client.get_object_attributes(
        Bucket=bucket, Key=key, ObjectAttributes=asked
    )
    return {
        "members": sorted(
            [name for name in read if name != "ResponseMetadata"]
        ),
        "etag": read.get("ETag"),
        "size": read.get("ObjectSize"),
        "storage_class": read.get("StorageClass"),
        "status": read["ResponseMetadata"]["HTTPStatusCode"],
    }


def lambda_handler(event, context):
    client = boto3.client("s3")
    bucket = event["bucket"]
    head = client.head_object(Bucket=bucket, Key=event["key"])
    return {
        "walks": [
            {
                "case": row["case"],
                "read": _walk(client, bucket, row["prefix"], row["page"]),
            }
            for row in event["walks"]
        ],
        "attributes": [
            {
                "case": row["case"],
                "read": _attributes(client, bucket, event["key"], row["asked"]),
            }
            for row in event["attributes"]
        ],
        "head": {"etag": head.get("ETag"), "length": head.get("ContentLength")},
    }
"#;

/// The handler of the second test: the instants the answers of several operations carry, each
/// reported as what the SDK handed the handler for it.
const INSTANT_SOURCE: &str = r#"
import datetime

import boto3


def _instant(value):
    """What the SDK handed the handler for one timestamp member.

    An SDK hands a caller its own language's instant for a timestamp, so what is reported is
    the TYPE of the value, whether it carries a zone, and the things only an instant can do:
    render itself, compare with another instant and be subtracted from one.
    """
    report = {"type": type(value).__name__}
    try:
        offset = value.utcoffset()
        report["zone_aware"] = offset is not None
        report["offset_seconds"] = None if offset is None else offset.total_seconds()
        report["iso"] = value.isoformat()
        report["year"] = value.year
        report["age_seconds"] = (
            datetime.datetime.now(datetime.timezone.utc) - value
        ).total_seconds()
    except (AttributeError, TypeError) as error:
        report["error"] = "%s: %s" % (type(error).__name__, error)
    return report


def _carried(answered, path):
    """The value `path` reaches in one answer: member names and list indices."""
    held = answered
    for step in path:
        held = held[step]
    return held


def lambda_handler(event, context):
    client = boto3.client("s3")
    bucket = event["bucket"]
    key = event["key"]
    answers = {
        "head_object": client.head_object(Bucket=bucket, Key=key),
        "get_object": client.get_object(Bucket=bucket, Key=key),
        "list_objects_v2": client.list_objects_v2(Bucket=bucket),
        "list_objects": client.list_objects(Bucket=bucket),
        "list_buckets": client.list_buckets(),
        "get_object_attributes": client.get_object_attributes(
            Bucket=bucket, Key=key, ObjectAttributes=["ETag", "ObjectSize"]
        ),
    }
    return {
        row["case"]: _instant(_carried(answers[row["operation"]], row["path"]))
        for row in event["rows"]
    }
"#;

/// One instant an answer carries: what the row is called, the operation whose answer carries
/// it, and the path that reaches it in that answer.
struct Instant {
    /// What the row is called in a failure.
    case: &'static str,
    /// The operation whose answer carries it, as the handler names it.
    operation: &'static str,
    /// The member names and list indices that reach it.
    path: Value,
}

/// The instants this suite reads, one row per place an answer of this service carries one.
fn instants() -> Vec<Instant> {
    vec![
        Instant {
            case: "the instant a read of one object reports (HeadObject$LastModified)",
            operation: "head_object",
            path: json!(["LastModified"]),
        },
        Instant {
            case: "the same on the read that carries the bytes (GetObject$LastModified)",
            operation: "get_object",
            path: json!(["LastModified"]),
        },
        Instant {
            case: "an instant inside a listing's own entry (Object$LastModified)",
            operation: "list_objects_v2",
            path: json!(["Contents", 0, "LastModified"]),
        },
        Instant {
            case: "the same in the first form of the listing",
            operation: "list_objects",
            path: json!(["Contents", 0, "LastModified"]),
        },
        Instant {
            case: "when a bucket came into being (Bucket$CreationDate)",
            operation: "list_buckets",
            path: json!(["Buckets", 0, "CreationDate"]),
        },
        Instant {
            case: "the instant an attribute read reports (…Output$LastModified)",
            operation: "get_object_attributes",
            path: json!(["LastModified"]),
        },
    ]
}

/// A handler walking a listing with its marker and reading an object's attributes: every call
/// reaches Amazon S3, and the keys and the attributes it reports are the ones the store holds.
#[tokio::test]
async fn a_handler_walks_a_marker_listing_and_reads_an_objects_attributes() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    let case = "a handler reading a bucket through the runtime's SDK";
    let router = emulator();
    given_objects(&router, case).await;
    let role = given_role(&router, case, "handler-listing-role", Some(reader())).await;
    let function = "handler-listing-reader";
    given_function(&router, case, function, &role, LISTING_SOURCE).await;
    let rows = walks();
    let asked = attributes();
    let report = invoked(
        &router,
        case,
        function,
        &json!({
            "bucket": BUCKET,
            "key": ATTRIBUTED.0,
            "walks": rows
                .iter()
                .map(|row| json!({
                    "case": row.case,
                    "prefix": row.prefix,
                    "page": row.page,
                }))
                .collect::<Vec<Value>>(),
            "attributes": asked
                .iter()
                .map(|row| json!({ "case": row.case, "asked": row.asked }))
                .collect::<Vec<Value>>(),
        }),
    )
    .await;

    let walked = report["walks"].as_array().unwrap_or_else(|| {
        panic!("case {case:?}: expected one report per walk, got {report}")
    });
    assert_eq!(
        walked.len(),
        rows.len(),
        "case {case:?}: expected one report per walk, got {report}"
    );
    for (row, answered) in rows.iter().zip(walked) {
        let read = &answered["read"];
        assert_eq!(
            read["keys"],
            json!(row.keys),
            "case {:?}: expected the keys the store holds, got {read}",
            row.case
        );
        assert_eq!(
            read["pages"],
            json!(row.pages),
            "case {:?}: expected {} call(s) at a page of {}, got {read}",
            row.case,
            row.pages,
            row.page
        );
        assert_eq!(
            read["name"],
            json!(BUCKET),
            "case {:?}: expected the listing to name the bucket, got {read}",
            row.case
        );
        assert_eq!(
            read["max_keys"],
            json!(row.page),
            "case {:?}: expected the listing to echo the page size asked for, got {read}",
            row.case
        );
    }

    // The attributes: each read carries what it asked for and nothing it did not, and the two
    // the object itself decides are the ones a read of the object reports — the tag without
    // the quotes the header's own syntax requires, and the size of the bytes stored.
    let head = &report["head"];
    let quoted = head["etag"].as_str().unwrap_or_default().to_string();
    let bare = quoted.trim_matches('"').to_string();
    let reads = report["attributes"].as_array().unwrap_or_else(|| {
        panic!("case {case:?}: expected one report per attribute read, got {report}")
    });
    for (row, answered) in asked.iter().zip(reads) {
        let read = &answered["read"];
        let members: Vec<String> = read["members"]
            .as_array()
            .map(|held| {
                held.iter()
                    .map(|name| name.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        for member in &row.present {
            assert!(
                members.iter().any(|held| held == member),
                "case {:?}: expected the answer to carry {member}, got {read}",
                row.case
            );
        }
        for member in &row.absent {
            assert!(
                !members.iter().any(|held| held == member),
                "case {:?}: expected the answer to carry no {member}, got {read}",
                row.case
            );
        }
        if row.present.contains(&"ETag") {
            assert_eq!(
                read["etag"],
                json!(bare),
                "case {:?}: expected the tag the object carries ({quoted}), got {read}",
                row.case
            );
        }
        if row.present.contains(&"ObjectSize") {
            assert_eq!(
                read["size"],
                json!(ATTRIBUTED.1.len()),
                "case {:?}: expected the size of the bytes stored, got {read}",
                row.case
            );
            assert_eq!(
                read["size"], head["length"],
                "case {:?}: expected the length a read of the object reports ({}), got {read}",
                row.case, head["length"]
            );
        }
    }
}

/// Every instant one of this service's answers carries reaches the handler as a
/// timezone-aware instant, wherever in the answer it sits and whichever binding the wire
/// carried it in.
#[tokio::test]
async fn every_instant_an_answer_carries_reaches_the_handler_as_a_zone_aware_instant() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    let case = "a handler reading the instants of its answers";
    let router = emulator();
    given_objects(&router, case).await;
    let role = given_role(&router, case, "handler-instant-role", Some(reader())).await;
    let function = "handler-instant-reader";
    given_function(&router, case, function, &role, INSTANT_SOURCE).await;
    let rows = instants();
    let report = invoked(
        &router,
        case,
        function,
        &json!({
            "bucket": BUCKET,
            "key": ATTRIBUTED.0,
            "rows": rows
                .iter()
                .map(|row| json!({
                    "case": row.case,
                    "operation": row.operation,
                    "path": row.path,
                }))
                .collect::<Vec<Value>>(),
        }),
    )
    .await;

    for row in &rows {
        let read = &report[row.case];
        assert_eq!(
            read["type"],
            json!("datetime"),
            "case {:?}: expected the SDK to hand the handler an instant, got {read}",
            row.case
        );
        assert_eq!(
            read["zone_aware"],
            json!(true),
            "case {:?}: expected the instant to carry a zone, got {read}",
            row.case
        );
        assert_eq!(
            read["offset_seconds"],
            json!(0.0),
            "case {:?}: expected the instant to be in UTC, the zone a function runs in, got \
             {read}",
            row.case
        );
        // The things only an instant can do: render itself, and be subtracted from another
        // instant — which is what a handler doing date arithmetic on the value needs.
        assert!(
            read["iso"]
                .as_str()
                .is_some_and(|rendered| rendered.contains('T')),
            "case {:?}: expected the instant to render itself, got {read}",
            row.case
        );
        assert!(
            read["age_seconds"].as_f64().is_some(),
            "case {:?}: expected the instant to be subtractable from another, got {read}",
            row.case
        );
        // The objects and the bucket this suite created, so the instant is this run's rather
        // than a value the wire's own form happened to parse into something else.
        assert!(
            read["year"].as_i64().is_some_and(|year| year >= 2020),
            "case {:?}: expected the instant of a resource this run created, got {read}",
            row.case
        );
    }
}
