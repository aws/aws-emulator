// SPDX-License-Identifier: CC0-1.0

//! REFUSED and served Amazon EC2 calls in CloudTrail event history, across two services on
//! one engine.
//!
//! RULE: a service that serves a control-plane API call records it as a management event of
//! that service whether it served the call or REFUSED it, so `cloudtrail:LookupEvents`
//! reports the call under its own `eventName` with the serving service's `eventSource` —
//! and for a refused call with the fault class of the answer plus the error code the caller
//! saw, the message it saw, and no `responseElements` at all, each record naming the
//! request by the id the caller's own response carried. A served call carries no
//! `errorCode`.
//!
//! Only a cross-service suite can pin this: the record is written by the service that
//! SERVES the call (Amazon EC2, ec2Query) and read back through CloudTrail (awsJson1_1) —
//! two services over ONE engine, as `core/router/src/lib.rs` mounts them. The per-record
//! field semantics are pinned over a wider table in the serving crate's own suite
//! (`services/service-com-amazonaws-ec2-amazonec2/tests/all/served_call_audit_records.rs`).
//!
//! Every call goes over the wire, on a router with request authentication off as every
//! suite in this crate runs it. Assembling the whole router costs stack (see
//! `boot_serve.rs`), so it is built — and the requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The Region every call below is signed for, and so the Region the records live in.
const REGION: &str = "us-west-2";

/// The access key id every call carries.
const ACCESS_KEY_ID: &str = "AKIDTEST";

/// The `eventSource` CloudTrail stamps on an Amazon EC2 management event.
const EC2_EVENT_SOURCE: &str = "ec2.amazonaws.com";

/// The EC2 API version an ec2Query body carries.
const EC2_API_VERSION: &str = "2016-11-15";

/// A SHORT-form instance id the account holds no instance for — a form EC2 reads and looks
/// up, so the describe below is refused as a resource that is not there.
const ABSENT_INSTANCE: &str = "i-1a2b3c4d";

/// What one call answered.
struct Answer {
    status: u16,
    body: bytes::Bytes,
}

impl Answer {
    fn json(&self) -> Value {
        if self.body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&self.body).unwrap_or(Value::Null)
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// Dispatch one request signed for `signing_name`.
fn dispatch(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: &str,
    headers: &[(&str, &str)],
    body: bytes::Bytes,
) -> Answer {
    let host = format!("{signing_name}.{REGION}.amazonaws.com");
    let mut builder = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", host.clone())
        .header(
            // The signature is never checked: this router runs with authentication off.
            // What the scope carries is the key id and the Region a handler reads.
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential={ACCESS_KEY_ID}/20260101/{REGION}/{signing_name}/aws4_request, \
                 SignedHeaders=host, Signature=not-verified"
            ),
        );
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder.body(body).expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        body,
    }
}

/// One ec2Query call to Amazon EC2.
fn ec2(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    action: &str,
    params: &[(&str, &str)],
) -> Answer {
    let mut body = format!("Action={action}&Version={EC2_API_VERSION}");
    for (name, value) in params {
        assert!(
            !value.contains(['&', '=', '+', '%']),
            "test parameter {name} would need form escaping: {value:?}"
        );
        body.push_str(&format!("&{name}={value}"));
    }
    dispatch(
        router,
        runtime,
        "ec2",
        &[("content-type", "application/x-www-form-urlencoded")],
        bytes::Bytes::from(body),
    )
}

/// `cloudtrail:LookupEvents` filtered on one attribute, answering each event's
/// `CloudTrailEvent` document.
fn lookup(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    attribute_key: &str,
    attribute_value: &str,
) -> Vec<Value> {
    let answered = dispatch(
        router,
        runtime,
        "cloudtrail",
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("x-amz-target", "CloudTrail_20131101.LookupEvents"),
        ],
        bytes::Bytes::from(
            json!({
                "LookupAttributes": [{
                    "AttributeKey": attribute_key,
                    "AttributeValue": attribute_value,
                }],
            })
            .to_string(),
        ),
    );
    assert_eq!(
        answered.status,
        200,
        "LookupEvents({attribute_key}={attribute_value}) must succeed, got {} with {:?}",
        answered.status,
        answered.text()
    );
    answered.json()["Events"]
        .as_array()
        .unwrap_or_else(|| panic!("LookupEvents reports an Events list, got {}", answered.text()))
        .iter()
        .map(|event| {
            let document = event["CloudTrailEvent"]
                .as_str()
                .unwrap_or_else(|| panic!("an event carries its CloudTrailEvent: {event}"));
            serde_json::from_str(document)
                .unwrap_or_else(|error| panic!("CloudTrailEvent is JSON ({error}): {document}"))
        })
        .collect()
}

/// The text of the first `<name>` element of an XML body, or `""`.
fn xml_element(body: &str, name: &str) -> String {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    match body.find(&open) {
        None => String::new(),
        Some(at) => {
            let start = at + open.len();
            match body[start..].find(&close) {
                None => String::new(),
                Some(end) => body[start..start + end].to_string(),
            }
        }
    }
}

/// The one record among `events` that names the request answered under `request_id` — how a
/// client holding its own response finds the record of that very call.
fn by_request_id<'a>(case: &str, events: &'a [Value], request_id: &str) -> &'a Value {
    let found: Vec<&Value> = events
        .iter()
        .filter(|event| event.get("requestID") == Some(&json!(request_id)))
        .collect();
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one record naming the request {request_id:?}, \
         got {events:?}"
    );
    found[0]
}

#[test]
fn refused_ec2_calls_are_in_cloudtrail_event_history() {
    std::thread::Builder::new()
        .name("ec2-management-events".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a current-thread runtime");

            // An AMI of the published catalog, resolved the way a client does, so the dry
            // run below names an image this account really has.
            let images = ec2(&router, &runtime, "DescribeImages", &[]);
            assert_eq!(
                images.status,
                200,
                "DescribeImages must be served, got {} with {:?}",
                images.status,
                images.text()
            );
            let image_id = xml_element(&images.text(), "imageId");
            assert!(
                image_id.starts_with("ami-"),
                "the catalog must publish an image, got {image_id:?}"
            );

            // A read REFUSED for an instance nobody owns, a write REFUSED as a dry run, and
            // a read that is SERVED — the three calls the audit below is about.
            let refused_read = ec2(
                &router,
                &runtime,
                "DescribeInstances",
                &[("InstanceId.1", ABSENT_INSTANCE)],
            );
            assert_eq!(
                refused_read.status,
                400,
                "a describe of an absent instance is a 400, got {} with {:?}",
                refused_read.status,
                refused_read.text()
            );
            let refused_read_body = refused_read.text();
            let read_code = xml_element(&refused_read_body, "Code");
            let read_request_id = xml_element(&refused_read_body, "RequestID");
            assert_eq!(
                read_code, "InvalidInstanceID.NotFound",
                "the caller sees EC2's own code, got {read_code:?} in {refused_read_body}"
            );

            let dry_run = ec2(
                &router,
                &runtime,
                "RunInstances",
                &[
                    ("ImageId", image_id.as_str()),
                    ("InstanceType", "t3.micro"),
                    ("MinCount", "1"),
                    ("MaxCount", "1"),
                    ("DryRun", "true"),
                ],
            );
            assert_eq!(
                dry_run.status,
                412,
                "an authorized dry run is a 412, got {} with {:?}",
                dry_run.status,
                dry_run.text()
            );
            let dry_run_body = dry_run.text();
            let dry_run_request_id = xml_element(&dry_run_body, "RequestID");

            let served_read = ec2(&router, &runtime, "DescribeInstances", &[]);
            assert_eq!(
                served_read.status,
                200,
                "a describe of the account's instances must be served, got {} with {:?}",
                served_read.status,
                served_read.text()
            );
            let served_read_request_id = xml_element(&served_read.text(), "requestId");

            // All three are in event history, found the way the check that filed this issue
            // finds them: a lookup by the operation's name, then the record naming the
            // caller's own request id.
            let describes = lookup(&router, &runtime, "EventName", "DescribeInstances");
            let refused = by_request_id("the refused read", &describes, &read_request_id);
            assert_eq!(
                refused.get("eventSource"),
                Some(&json!(EC2_EVENT_SOURCE)),
                "the record names the serving service, got {refused}"
            );
            assert_eq!(
                refused.get("errorCode"),
                Some(&json!("Client.InvalidInstanceID.NotFound")),
                "the trail records the fault class plus the code the caller saw, got {refused}"
            );
            assert_eq!(
                refused.get("errorMessage"),
                Some(&json!(format!(
                    "The instance ID '{ABSENT_INSTANCE}' does not exist"
                ))),
                "the trail records the message the caller saw, got {refused}"
            );
            assert_eq!(
                refused.get("responseElements"),
                Some(&Value::Null),
                "a refused call has no response to record, got {refused}"
            );
            assert_eq!(
                refused.get("readOnly"),
                Some(&json!(true)),
                "a describe only reports state, got {refused}"
            );

            let served = by_request_id("the served read", &describes, &served_read_request_id);
            assert_eq!(
                served.get("errorCode"),
                None,
                "a served call carries no error code at all, got {served}"
            );

            let launches = lookup(&router, &runtime, "EventName", "RunInstances");
            let refused_launch = by_request_id("the dry run", &launches, &dry_run_request_id);
            assert_eq!(
                refused_launch.get("errorCode"),
                Some(&json!("Client.DryRunOperation")),
                "the trail records the dry run's own class and code, got {refused_launch}"
            );
            assert_eq!(
                refused_launch.get("errorMessage"),
                Some(&json!("Request would have succeeded, but DryRun flag is set.")),
                "the trail records EC2's dry-run sentence, got {refused_launch}"
            );
            assert_eq!(
                refused_launch.get("responseElements"),
                Some(&Value::Null),
                "a refused launch started nothing to report, got {refused_launch}"
            );
        })
        .expect("spawn the EC2 management-event thread")
        .join()
        .expect("refused EC2 calls must be in CloudTrail event history");
}
