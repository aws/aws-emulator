// SPDX-License-Identifier: CC0-1.0

//! What a handler written against the SDK's RESOURCE interface can do with what the runtime
//! supplies: Lambda, IAM, the token service and Amazon S3 on ONE engine, a Python function
//! that reaches a bucket through `boto3.resource("s3")`, walks and filters its objects, and
//! stores and reads one object through the `Object` resource.
//!
//! RULE: the AWS SDK a managed runtime supplies carries the RESOURCE interface of the services
//! it has resource classes for, so `boto3.resource(<service>)` answers that service's resource
//! and a collection hanging off one is an ITERABLE the SDK pages through by calling the
//! listing the collection is over — every call being the client's own, so what a resource
//! reports is the service's own answer.
//!
//! The rows differ along what the collection is asked for: every object, the objects one of
//! several prefixes selects, a bound on how many objects the iteration yields, and a page size
//! that makes the walk take several calls — and along the keys each of those must yield.
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

/// The bucket the handler reaches through the resource interface.
const BUCKET: &str = "handler-resource-bucket";

/// The objects the bucket holds, in the order their keys sort.
const OBJECTS: [(&str, &str); 6] = [
    ("logs/april.txt", "april"),
    ("logs/june.txt", "june"),
    ("logs/march.txt", "march"),
    ("photos/one.jpg", "one"),
    ("photos/two.jpg", "two"),
    ("readme.txt", "readme"),
];

/// The key the handler writes through the `Object` resource, and the bytes it writes.
const WRITTEN: (&str, &str) = ("written/by-the-resource.txt", "written by the resource");

/// One ask of the bucket's object collection: what the row is called, the prefix it filters
/// by, the bounds it carries, and the keys it must yield.
struct Asked {
    /// What the row is called in a failure.
    case: &'static str,
    /// The prefix the collection filters by, or the empty text for no filter at all.
    prefix: &'static str,
    /// The bound on how many objects the iteration yields (`limit`), where the row carries
    /// one.
    limit: Option<u64>,
    /// The page size the listing behind it is asked for (`page_size`), where the row carries
    /// one.
    page_size: Option<u64>,
    /// The keys the iteration must yield, in the order the listing reports them.
    keys: Vec<&'static str>,
}

/// The asks this suite drives, one row per thing a collection may be asked for.
fn asked() -> Vec<Asked> {
    let every: Vec<&'static str> = OBJECTS.iter().map(|(key, _)| *key).collect();
    vec![
        Asked {
            case: "every object the bucket holds",
            prefix: "",
            limit: None,
            page_size: None,
            keys: every.clone(),
        },
        Asked {
            case: "the objects one prefix selects",
            prefix: "logs/",
            limit: None,
            page_size: None,
            keys: vec![OBJECTS[0].0, OBJECTS[1].0, OBJECTS[2].0],
        },
        Asked {
            case: "the objects another prefix selects",
            prefix: "photos/",
            limit: None,
            page_size: None,
            keys: vec![OBJECTS[3].0, OBJECTS[4].0],
        },
        Asked {
            case: "a prefix selecting one object alone",
            prefix: "readme",
            limit: None,
            page_size: None,
            keys: vec![OBJECTS[5].0],
        },
        Asked {
            case: "a prefix the bucket holds no key under",
            prefix: "nothing/",
            limit: None,
            page_size: None,
            keys: Vec::new(),
        },
        Asked {
            case: "a bound on how many objects the iteration yields",
            prefix: "",
            limit: Some(2),
            page_size: None,
            keys: vec![OBJECTS[0].0, OBJECTS[1].0],
        },
        Asked {
            case: "a page size that makes the walk take several calls",
            prefix: "",
            limit: None,
            page_size: Some(2),
            keys: every.clone(),
        },
        Asked {
            case: "a page size and a bound together",
            prefix: "",
            limit: Some(3),
            page_size: Some(2),
            keys: vec![OBJECTS[0].0, OBJECTS[1].0, OBJECTS[2].0],
        },
        Asked {
            case: "a prefix with a page size smaller than what it selects",
            prefix: "logs/",
            limit: None,
            page_size: Some(1),
            keys: vec![OBJECTS[0].0, OBJECTS[1].0, OBJECTS[2].0],
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

/// Create the bucket and the objects the handler walks.
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

/// The handler this suite runs: the collection asks the event names, and one object stored and
/// read back through the `Object` resource.
const HANDLER_SOURCE: &str = r#"
import boto3


def _collection(bucket, row):
    """The bucket's objects as one row asks for them."""
    held = bucket.objects
    if row["prefix"]:
        held = held.filter(Prefix=row["prefix"])
    else:
        held = held.all()
    if row.get("page_size") is not None:
        held = held.page_size(row["page_size"])
    if row.get("limit") is not None:
        held = held.limit(row["limit"])
    return held


def _walked(bucket, row):
    """What one ask of the collection yields, as values the answer carries."""
    summaries = list(_collection(bucket, row))
    return {
        "keys": [one.key for one in summaries],
        "buckets": sorted(set([one.bucket_name for one in summaries])),
        "sizes": [one.size for one in summaries],
        "tagged": [bool(one.e_tag) for one in summaries],
        "types": sorted(set([type(one).__name__ for one in summaries])),
        "instants": sorted(
            set([type(one.last_modified).__name__ for one in summaries])
        ),
    }


def lambda_handler(event, context):
    resource = boto3.resource("s3")
    bucket = resource.Bucket(event["bucket"])
    walked = [
        {"case": row["case"], "read": _walked(bucket, row)} for row in event["rows"]
    ]

    # The object resource itself: written through the bucket, read back through the object,
    # and removed — the resource-style round trip a handler stores through.
    written = bucket.put_object(Key=event["written_key"], Body=event["written_body"])
    read = written.get()
    object_resource = resource.Object(event["bucket"], event["written_key"])
    summary = resource.ObjectSummary(event["bucket"], event["written_key"])
    stored = {
        "type": type(written).__name__,
        "bucket_name": written.bucket_name,
        "key": written.key,
        "body": read["Body"].read().decode("utf-8"),
        "content_length": object_resource.content_length,
        "summary_size": summary.size,
        "summary_type": type(summary).__name__,
        "same_object": object_resource == resource.Object(
            event["bucket"], event["written_key"]
        ),
    }
    object_resource.delete()
    stored["keys_after_delete"] = [
        one.key for one in bucket.objects.filter(Prefix=event["written_key"])
    ]

    # The buckets of the account, as the service resource's own collection reports them.
    return {
        "walked": walked,
        "stored": stored,
        "buckets": sorted([one.name for one in resource.buckets.all()]),
        "bucket_types": sorted(
            set([type(one).__name__ for one in resource.buckets.all()])
        ),
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
            "Timeout": 60,
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

/// A handler reaching a bucket through the SDK's resource interface: every ask of the
/// collection yields the objects the store holds, and the object it writes through the
/// resource is the object the store then serves.
#[tokio::test]
async fn a_handler_walks_a_bucket_through_the_sdks_resource_interface() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    let case = "a handler reaching a bucket through the resource interface";
    let router = emulator();
    given_objects(&router, case).await;
    let role = given_role(
        &router,
        case,
        "handler-resource-role",
        Some(json!([{ "Effect": "Allow", "Action": "s3:*", "Resource": "*" }])),
    )
    .await;
    let function = "handler-resource-walker";
    given_function(&router, case, function, &role).await;
    let rows = asked();
    let answered = call(
        &router,
        "POST",
        &format!("{FUNCTIONS_URI}/{function}/invocations"),
        &json!({
            "bucket": BUCKET,
            "written_key": WRITTEN.0,
            "written_body": WRITTEN.1,
            "rows": rows
                .iter()
                .map(|row| json!({
                    "case": row.case,
                    "prefix": row.prefix,
                    "limit": row.limit,
                    "page_size": row.page_size,
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

    let walked = report["walked"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected one report per row, got {report}"));
    assert_eq!(
        walked.len(),
        rows.len(),
        "case {case:?}: expected one report per row, got {report}"
    );
    for (row, answered) in rows.iter().zip(walked) {
        let read = &answered["read"];
        assert_eq!(
            read["keys"],
            json!(row.keys),
            "case {:?}: expected the keys the store holds, got {read}",
            row.case
        );
        let sizes: Vec<Value> = row
            .keys
            .iter()
            .map(|key| {
                let (_, body) = OBJECTS
                    .iter()
                    .find(|(held, _)| held == key)
                    .unwrap_or_else(|| panic!("case {:?}: {key} is one of the objects", row.case));
                json!(body.len())
            })
            .collect();
        assert_eq!(
            read["sizes"],
            json!(sizes),
            "case {:?}: expected the size the listing reports for each object, got {read}",
            row.case
        );
        assert!(
            read["tagged"]
                .as_array()
                .is_some_and(|held| held.iter().all(|carried| carried == &json!(true))),
            "case {:?}: expected the listing's own tag for each object, got {read}",
            row.case
        );
        if !row.keys.is_empty() {
            assert_eq!(
                read["buckets"],
                json!([BUCKET]),
                "case {:?}: expected every object to name the bucket it is held in, got {read}",
                row.case
            );
            assert_eq!(
                read["types"],
                json!(["ObjectSummary"]),
                "case {:?}: expected the collection to yield object summaries, got {read}",
                row.case
            );
            // The listing's own instant reaches the handler as an instant, which is what a
            // summary reports for the object it stands for.
            assert_eq!(
                read["instants"],
                json!(["datetime"]),
                "case {:?}: expected the summary's own instant, got {read}",
                row.case
            );
        }
    }

    // The object resource: the write reached the service, the read gave back the bytes, and
    // the removal left the bucket holding none of it.
    let stored = &report["stored"];
    assert_eq!(
        stored["type"],
        json!("Object"),
        "case {case:?}: expected the write to answer the object as a resource, got {stored}"
    );
    assert_eq!(
        stored["bucket_name"],
        json!(BUCKET),
        "case {case:?}: expected the object to name its bucket, got {stored}"
    );
    assert_eq!(
        stored["key"],
        json!(WRITTEN.0),
        "case {case:?}: expected the object to name its key, got {stored}"
    );
    assert_eq!(
        stored["body"],
        json!(WRITTEN.1),
        "case {case:?}: expected the bytes the handler wrote, got {stored}"
    );
    for member in ["content_length", "summary_size"] {
        assert_eq!(
            stored[member],
            json!(WRITTEN.1.len()),
            "case {case:?}: expected {member} to be the size of the bytes written, got {stored}"
        );
    }
    assert_eq!(
        stored["summary_type"],
        json!("ObjectSummary"),
        "case {case:?}: expected the service resource to answer a summary, got {stored}"
    );
    assert_eq!(
        stored["same_object"],
        json!(true),
        "case {case:?}: expected two resources for one object to be equal, got {stored}"
    );
    assert_eq!(
        stored["keys_after_delete"],
        json!([] as [Value; 0]),
        "case {case:?}: expected the removal to leave the bucket holding none of it, got \
         {stored}"
    );
    let (status, document) = s3(&router, "HEAD", &format!("/{BUCKET}/{}", WRITTEN.0), b"").await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected the store to hold no object the resource removed, got \
         {status}: {document}"
    );

    // The account's buckets, as the service resource's own collection reports them.
    assert_eq!(
        report["buckets"],
        json!([BUCKET]),
        "case {case:?}: expected the buckets the account holds, got {report}"
    );
    assert_eq!(
        report["bucket_types"],
        json!(["Bucket"]),
        "case {case:?}: expected the collection to yield buckets, got {report}"
    );
}
