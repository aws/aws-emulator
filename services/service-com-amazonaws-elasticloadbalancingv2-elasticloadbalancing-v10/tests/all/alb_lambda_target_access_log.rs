// SPDX-License-Identifier: CC0-1.0

//! The ACCESS LOG a load balancer with a `lambda` target delivers, and the per-request
//! evaluation of the permission that admits the invocation.
//!
//! The rule under test: **a load balancer whose `access_logs.s3.*` attributes name a bucket
//! delivers, for every request it answered, one record into that bucket — carrying the status
//! the CLIENT was answered with, the status the TARGET answered where a target answered one,
//! and, for an answer the load balancer generated itself, the CLASS of the failure it
//! generated it for; and the grant that admits the invocation is the function's own resource
//! policy, read on EVERY request, so removing it makes the load balancer answer the request
//! itself without invoking the function and restoring it serves the function's own response
//! again with no re-registration of the target.**
//!
//! `LoadBalancerAttribute$Key` states the destination — "`access_logs.s3.enabled` - Indicates
//! whether access logs are enabled. The value is true or false. The default is false",
//! "`access_logs.s3.bucket` - The name of the S3 bucket for the access logs" and
//! "`access_logs.s3.prefix` - The prefix for the location in the S3 bucket for the access
//! logs"; `AddPermission` / `RemovePermission` are what write and withdraw the grant a load
//! balancer's invocation is admitted by ("for Amazon Web Services services, the principal is
//! a domain-style identifier that the service defines", narrowed by the target group's ARN as
//! `aws:SourceArn`). The record's own field order, and the `error_reason` token each failure
//! class carries, are Elastic Load Balancing's documented access-log format.
//!
//! The rows differ along what the rule generalises over: the state of the GRANT when the
//! request is made (in place, withdrawn, restored) and the CLASS of answer the load balancer
//! generated (a response naming no status code, a payload that is no response object, a run
//! that raised, a response past the size such a target may answer with), beside the
//! well-formed answer the function's own run produced.
//!
//! That the function ran — or did not — is asserted where nothing of this service can fake
//! it: the handler reports every invocation to an Amazon SQS queue through its own SDK before
//! it decides what to return, so a row whose grant was withdrawn leaves the queue empty and a
//! row that was served leaves exactly one report. Every request is made over a real
//! connection to the address the load balancer publishes, and every record is read back where
//! a client reads it: out of the bucket, through Amazon S3's own API.

use std::io::Read as _;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use crate::alb_data_plane::{create_load_balancer, settle};
use crate::alb_lambda_target_published_address::{
    element, exchange, execution_role, lambda, package, reported, sqs, HANDLER, RUNTIME,
    TARGET_BODY,
};
use crate::provisioning_loop::{action, call, p, REGION};

/// The function every row invokes, the role it runs as, the queue it reports to, and the
/// statement id the grant is written under.
const FUNCTION: &str = "alb-access-log-target";
const ROLE_NAME: &str = "alb-access-log-exec";
const SIDE_CHANNEL: &str = "alb-access-log-side-channel";
const STATEMENT: &str = "elbv2-invoke";

/// The load balancer, its target group, and the bucket and prefix its attributes send the
/// access log to.
const LOAD_BALANCER: &str = "alb-access-log";
const TARGET_GROUP: &str = "alb-access-log-tg";
const LOG_BUCKET: &str = "elbv2-access-logs";
const LOG_PREFIX: &str = "edge";

/// The three attributes that turn access logging on and say where it goes, from
/// `LoadBalancerAttribute$Key`'s own catalog.
const ENABLED_ATTRIBUTE: &str = "access_logs.s3.enabled";
const BUCKET_ATTRIBUTE: &str = "access_logs.s3.bucket";
const PREFIX_ATTRIBUTE: &str = "access_logs.s3.prefix";

/// The status a load balancer answers for a Lambda-target response its contract refuses, and
/// for an invocation the function's policy admits no load balancer to make.
const BAD_GATEWAY: u16 = 502;

/// What a record's field carries where it does not apply.
const ABSENT: &str = "-";

/// How long a row waits for work the clock owes — a report on the side channel, a record in
/// the bucket — and how long between two looks. Generous on purpose: everything waited for
/// happens in this process, so a loaded host does not decide a verdict.
const BUDGET: Duration = Duration::from_secs(30);
const LOOK_AGAIN: Duration = Duration::from_millis(25);

/// This service, Lambda, Amazon SQS, IAM, the token service and Amazon S3 over ONE shared
/// engine, as `core/router/src/lib.rs` mounts them: the function the load balancer forwards to
/// is run by that Lambda as a session of the role IAM holds, the handler's own SDK call is
/// dispatched back into this router, and the bucket the access log is delivered into is the
/// bucket this suite reads.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
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
    (router, engine)
}

/// The Amazon S3 client this suite reads the delivered records with, built by the one shared
/// constructor so it reads the store the delivery wrote into.
fn s3(router: &MultiServiceRouter) -> aws_sdk_s3::Client {
    service_common::s3_client::s3_client(&EmbeddedEmulator::from_router(router.clone()), REGION)
}

/// Create the bucket the access log is delivered into, in the Region every request here is
/// signed for.
async fn log_bucket(router: &MultiServiceRouter) {
    s3(router)
        .create_bucket()
        .bucket(LOG_BUCKET)
        .create_bucket_configuration(
            aws_sdk_s3::types::CreateBucketConfiguration::builder()
                .location_constraint(aws_sdk_s3::types::BucketLocationConstraint::from(REGION))
                .build(),
        )
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup: creating {LOG_BUCKET} must succeed, got {error:?}"));
}

/// Every object the log bucket holds, by key, with the record each one carries — the gzip
/// stream decompressed, as a reader of the log decompresses it.
async fn delivered(router: &MultiServiceRouter) -> Vec<(String, String)> {
    let listed = s3(router)
        .list_objects_v2()
        .bucket(LOG_BUCKET)
        .send()
        .await
        .unwrap_or_else(|error| panic!("listing {LOG_BUCKET} must succeed, got {error:?}"));
    let mut records = Vec::new();
    for object in listed.contents() {
        let key = object.key().unwrap_or_default().to_string();
        let fetched = s3(router)
            .get_object()
            .bucket(LOG_BUCKET)
            .key(&key)
            .send()
            .await
            .unwrap_or_else(|error| panic!("fetching {key} must succeed, got {error:?}"));
        let bytes = fetched
            .body
            .collect()
            .await
            .unwrap_or_else(|error| panic!("reading {key} must succeed, got {error:?}"))
            .into_bytes();
        let mut text = String::new();
        flate2::read::GzDecoder::new(bytes.as_ref())
            .read_to_string(&mut text)
            .unwrap_or_else(|error| panic!("{key} must be a gzip stream, got {error}"));
        records.push((key, text.trim_end().to_string()));
    }
    records
}

/// The record delivered for the request this row made: the one object the bucket holds that
/// `seen` does not, waited for until the bucket holds it.
///
/// Every key read is remembered, so the next row reads only its own record.
async fn record_of(
    router: &MultiServiceRouter,
    case: &str,
    seen: &mut Vec<String>,
) -> (String, String) {
    let deadline = Instant::now() + BUDGET;
    loop {
        let held = delivered(router).await;
        let fresh: Vec<(String, String)> = held
            .into_iter()
            .filter(|(key, _)| !seen.contains(key))
            .collect();
        match fresh.as_slice() {
            [one] => {
                seen.push(one.0.clone());
                return one.clone();
            }
            [] => assert!(
                Instant::now() < deadline,
                "case {case:?}: the load balancer delivered no access-log record"
            ),
            several => panic!(
                "case {case:?}: expected one new access-log record, got {}: {several:?}",
                several.len()
            ),
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// One record line read the way a reader of the log reads it: the fields are
/// space-separated, and a quoted field carries its own spaces.
fn fields(line: &str) -> Vec<String> {
    let mut fields: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    for character in line.chars() {
        match character {
            '"' => quoted = !quoted,
            ' ' if !quoted => fields.push(std::mem::take(&mut field)),
            other => field.push(other),
        }
    }
    fields.push(field);
    fields
}

/// The positions of the fields this suite reads, in the documented order Elastic Load
/// Balancing publishes for an HTTP entry (1-based in the documentation, 0-based here): the
/// entry type, the load balancer's resource id, the status the client was answered with, the
/// status the target answered, the request line, the target group, the actions performed and
/// the error reason.
const TYPE_FIELD: usize = 0;
const ELB_FIELD: usize = 2;
const ELB_STATUS_FIELD: usize = 8;
const TARGET_STATUS_FIELD: usize = 9;
const REQUEST_FIELD: usize = 12;
const TARGET_GROUP_FIELD: usize = 16;
const ACTIONS_FIELD: usize = 22;
const ERROR_REASON_FIELD: usize = 24;

/// One row: the state of the grant when the request is made, the path the client asks for,
/// what the load balancer must answer, what the record must carry, and how many invocations
/// the side channel must show.
struct Row {
    case: &'static str,
    /// Whether the function's resource policy carries the grant when this request is made.
    granted: bool,
    target: &'static str,
    status: u16,
    /// The `error_reason` the record carries: the absent marker for a request a target
    /// answered.
    error_reason: &'static str,
    /// The `target_status_code` the record carries.
    target_status: &'static str,
    /// How many invocations the side channel must report for this request.
    invocations: usize,
    /// Whether the answer carries the function's own body, which no answer the load balancer
    /// generated itself does.
    serves_target_body: bool,
}

/// Every row's answer and every row's record come from what the records hold.
///
/// The grant is withdrawn and restored in the middle of the table — with no re-registration
/// of the target — which is what pins the evaluation to the REQUEST rather than to
/// `RegisterTargets`; the classes of generated answer follow it, each with the reason its own
/// failure carries.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a well-formed response, with the grant in place",
            granted: true,
            target: "/behind/ok",
            status: 200,
            error_reason: ABSENT,
            target_status: "200",
            invocations: 1,
            serves_target_body: true,
        },
        Row {
            case: "the grant withdrawn: the load balancer answers and nothing is invoked",
            granted: false,
            target: "/behind/ok",
            status: BAD_GATEWAY,
            error_reason: "LambdaAccessDenied",
            target_status: ABSENT,
            invocations: 0,
            serves_target_body: false,
        },
        Row {
            case: "the grant restored, with no re-registration of the target",
            granted: true,
            target: "/behind/ok",
            status: 200,
            error_reason: ABSENT,
            target_status: "200",
            invocations: 1,
            serves_target_body: true,
        },
        Row {
            case: "a response naming no statusCode",
            granted: true,
            target: "/behind/nostatus",
            status: BAD_GATEWAY,
            error_reason: "LambdaInvalidResponse",
            target_status: ABSENT,
            invocations: 1,
            serves_target_body: false,
        },
        Row {
            case: "a payload that is no response object at all",
            granted: true,
            target: "/behind/nonjson",
            status: BAD_GATEWAY,
            error_reason: "LambdaInvalidResponse",
            target_status: ABSENT,
            invocations: 1,
            serves_target_body: false,
        },
        Row {
            case: "a run that raised",
            granted: true,
            target: "/behind/raise",
            status: BAD_GATEWAY,
            error_reason: "LambdaUnhandled",
            target_status: ABSENT,
            invocations: 1,
            serves_target_body: false,
        },
        Row {
            case: "a response past the size a Lambda target may answer with",
            granted: true,
            target: "/behind/toolarge",
            status: BAD_GATEWAY,
            error_reason: "LambdaResponseTooLarge",
            target_status: ABSENT,
            invocations: 1,
            serves_target_body: false,
        },
        Row {
            case: "a well-formed response after every refused one: the target still serves",
            granted: true,
            target: "/behind/ok",
            status: 200,
            error_reason: ABSENT,
            target_status: "200",
            invocations: 1,
            serves_target_body: true,
        },
    ]
}

/// Write the grant that admits this load balancer's invocation, scoped to the target group.
async fn grant(router: &MultiServiceRouter, case: &str, target_group: &str) {
    let (status, granted) = lambda(
        router,
        "POST",
        &format!("/2015-03-31/functions/{FUNCTION}/policy"),
        json!({
            "StatementId": STATEMENT,
            "Action": "lambda:InvokeFunction",
            "Principal": "elasticloadbalancing.amazonaws.com",
            "SourceArn": target_group,
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected AddPermission to answer 201, got {status} {granted}"
    );
}

/// Withdraw that grant.
async fn withdraw(router: &MultiServiceRouter, case: &str) {
    let (status, removed) = lambda(
        router,
        "DELETE",
        &format!("/2015-03-31/functions/{FUNCTION}/policy/{STATEMENT}"),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 204,
        "case {case:?}: expected RemovePermission to answer 204, got {status} {removed}"
    );
}

/// RULE: every request a load balancer answers is recorded in the bucket its own attributes
/// name — with the status the client got, the status the target gave and the failure class of
/// an answer the load balancer generated itself — and the grant that admits the invocation is
/// read on every request.
#[tokio::test]
async fn an_access_log_records_what_answered_and_the_grant_is_read_on_every_request() {
    let (router, engine) = emulator();
    log_bucket(&router).await;

    // The side channel the handler reports every invocation to, and the role whose one grant
    // admits that report.
    let (status, queue) = sqs(&router, "CreateQueue", &json!({ "QueueName": SIDE_CHANNEL })).await;
    assert_eq!(
        status, 200,
        "setup CreateQueue: expected 200, got {status} {queue}"
    );
    let queue_url = queue["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("setup CreateQueue: no QueueUrl in {queue}"))
        .to_string();
    let (status, attributes) = sqs(
        &router,
        "GetQueueAttributes",
        &json!({ "QueueUrl": queue_url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup GetQueueAttributes: expected 200, got {status} {attributes}"
    );
    let queue_arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("setup GetQueueAttributes: no QueueArn in {attributes}"))
        .to_string();
    let role = execution_role(&router, ROLE_NAME, &queue_arn).await;

    // The function the load balancer forwards to, created through Lambda's own API.
    let (status, created) = lambda(
        &router,
        "POST",
        "/2015-03-31/functions",
        json!({
            "FunctionName": FUNCTION,
            "Role": role,
            "Runtime": RUNTIME,
            "Handler": HANDLER,
            "Timeout": 30,
            "Environment": { "Variables": { "QUEUE_URL": queue_url } },
            "Code": { "ZipFile": aws_smithy_types::base64::encode(package()) },
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup CreateFunction: expected 201, got {status} {created}"
    );
    let function_arn = created["FunctionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("setup CreateFunction: no FunctionArn in {created}"))
        .to_string();
    // The activation span Lambda owes the function, placed rather than slept for — the same
    // reading every suite that invokes a freshly created function uses.
    {
        let state = engine
            .state::<service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState>();
        let mut store = state.lock();
        let past = SystemTime::now() - Duration::from_secs(1);
        for record in store.functions.values_mut() {
            record.active_due = past;
        }
    }

    // The load balancer and the address it publishes, its `lambda` target group, the
    // registration of the function, the grant, and the HTTP:80 listener that forwards to the
    // group — every one of them through the owning API.
    let (lb_arn, published) = create_load_balancer(&router, LOAD_BALANCER).await;
    // The host of the published address, which is the name the client dialled and so the one
    // the record's request line carries; the PORT in that line is the listener's own.
    let node_address = published
        .split(':')
        .next()
        .unwrap_or(&published)
        .to_string();
    let load_balancer_id = lb_arn
        .split_once("loadbalancer/")
        .map(|(_, id)| id.to_string())
        .unwrap_or_else(|| panic!("setup: {lb_arn} names no load balancer resource"));

    let mut form = action("CreateTargetGroup");
    form.push(p("Name", TARGET_GROUP));
    form.push(p("TargetType", "lambda"));
    let reply = call(&router, &form).await;
    assert_eq!(
        reply.status, 200,
        "setup CreateTargetGroup: expected 200, got {} body {}",
        reply.status, reply.body
    );
    let tg_arn = reply
        .text("TargetGroupArn")
        .unwrap_or_else(|| panic!("setup CreateTargetGroup: no TargetGroupArn in {}", reply.body));

    let mut form = action("RegisterTargets");
    form.push(p("TargetGroupArn", &tg_arn));
    form.push(p("Targets.member.1.Id", &function_arn));
    let reply = call(&router, &form).await;
    assert_eq!(
        reply.status, 200,
        "setup RegisterTargets: expected 200, got {} body {}",
        reply.status, reply.body
    );

    grant(&router, "setup", &tg_arn).await;

    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", &lb_arn));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", "80"));
    form.push(p("DefaultActions.member.1.Type", "forward"));
    form.push(p("DefaultActions.member.1.TargetGroupArn", &tg_arn));
    let reply = call(&router, &form).await;
    assert_eq!(
        reply.status, 200,
        "setup CreateListener: expected 200, got {} body {}",
        reply.status, reply.body
    );

    // Access logging on, into the bucket and under the prefix the attributes name.
    let mut form = action("ModifyLoadBalancerAttributes");
    form.push(p("LoadBalancerArn", &lb_arn));
    form.push(p("Attributes.member.1.Key", ENABLED_ATTRIBUTE));
    form.push(p("Attributes.member.1.Value", "true"));
    form.push(p("Attributes.member.2.Key", BUCKET_ATTRIBUTE));
    form.push(p("Attributes.member.2.Value", LOG_BUCKET));
    form.push(p("Attributes.member.3.Key", PREFIX_ATTRIBUTE));
    form.push(p("Attributes.member.3.Value", LOG_PREFIX));
    let reply = call(&router, &form).await;
    assert_eq!(
        reply.status, 200,
        "setup ModifyLoadBalancerAttributes: expected 200, got {} body {}",
        reply.status, reply.body
    );

    // The move to `active` the clock owes the load balancer, placed rather than slept for.
    settle(&engine, &lb_arn, SystemTime::now() - Duration::from_secs(1));

    let mut granted = true;
    let mut seen: Vec<String> = Vec::new();
    for row in rows() {
        if row.granted != granted {
            if row.granted {
                grant(&router, row.case, &tg_arn).await;
            } else {
                withdraw(&router, row.case).await;
            }
            granted = row.granted;
        }

        let answered = exchange(row.case, &published, row.target).await;
        assert_eq!(
            answered.status, row.status,
            "case {:?}: expected HTTP {}, got {} in {:?}",
            row.case, row.status, answered.status, answered.raw
        );
        assert_eq!(
            answered.raw.contains(TARGET_BODY),
            row.serves_target_body,
            "case {:?}: expected the function's own body to be served={}, got {:?}",
            row.case,
            row.serves_target_body,
            answered.raw
        );

        // The function ran exactly as often as the grant admitted. An invocation is made
        // BEFORE the client is answered and the handler reports it before it returns, so a
        // queue that is empty once the answer is in hand is proof that nothing was invoked:
        // the row that expects none takes one look rather than waiting out a span.
        let budget = if row.invocations == 0 {
            Duration::ZERO
        } else {
            BUDGET
        };
        let reports = reported(&router, row.case, &queue_url, budget).await;
        assert_eq!(
            reports.len(),
            row.invocations,
            "case {:?}: expected {} invocation(s) reported on the side channel, got {reports:?}",
            row.case,
            row.invocations
        );

        // ... and the record the load balancer delivered for that very request.
        let (key, line) = record_of(&router, row.case, &mut seen).await;
        assert!(
            key.starts_with(&format!("{LOG_PREFIX}/AWSLogs/")),
            "case {:?}: expected the record under the configured prefix, got {key:?}",
            row.case
        );
        assert!(
            key.contains("/elasticloadbalancing/") && key.ends_with(".log.gz"),
            "case {:?}: expected the documented layout and suffix, got {key:?}",
            row.case
        );
        let fields = fields(&line);
        let field = |index: usize| fields.get(index).cloned().unwrap_or_default();
        assert_eq!(
            field(TYPE_FIELD),
            "http",
            "case {:?}: expected the entry type of the listener's protocol, got {line:?}",
            row.case
        );
        assert_eq!(
            field(ELB_FIELD),
            load_balancer_id,
            "case {:?}: expected the load balancer's own resource id, got {line:?}",
            row.case
        );
        assert_eq!(
            field(ELB_STATUS_FIELD),
            row.status.to_string(),
            "case {:?}: expected elb_status_code {}, got {line:?}",
            row.case,
            row.status
        );
        assert_eq!(
            field(TARGET_STATUS_FIELD),
            row.target_status,
            "case {:?}: expected target_status_code {:?}, got {line:?}",
            row.case,
            row.target_status
        );
        assert_eq!(
            field(REQUEST_FIELD),
            format!("GET http://{node_address}:80{} HTTP/1.1", row.target),
            "case {:?}: expected the request the client sent, got {line:?}",
            row.case
        );
        assert_eq!(
            field(TARGET_GROUP_FIELD),
            tg_arn,
            "case {:?}: expected the target group the request was forwarded to, got {line:?}",
            row.case
        );
        assert_eq!(
            field(ACTIONS_FIELD),
            "forward",
            "case {:?}: expected the action performed, got {line:?}",
            row.case
        );
        assert_eq!(
            field(ERROR_REASON_FIELD),
            row.error_reason,
            "case {:?}: expected error_reason {:?}, got {line:?}",
            row.case,
            row.error_reason
        );
    }

    // The target is still the one registration it was: a withdrawn grant and a refused
    // response cost their own request and nothing else.
    let mut form = action("DescribeTargetHealth");
    form.push(p("TargetGroupArn", &tg_arn));
    let reply = call(&router, &form).await;
    assert_eq!(
        reply.status, 200,
        "expected DescribeTargetHealth to answer 200, got {} body {}",
        reply.status, reply.body
    );
    assert_eq!(
        element(&reply.body, "Id"),
        function_arn,
        "expected the function to still be the group's one registered target, got {}",
        reply.body
    );
}

/// One row of the logging-switch table: the attributes written before the request, and how
/// many records the bucket holds once it has been answered.
struct LoggingRow {
    case: &'static str,
    attributes: &'static [(&'static str, &'static str)],
    records: usize,
}

/// RULE: a load balancer whose attributes do not name a bucket delivers nothing — the
/// attribute's documented default is off, and switching it off again stops the delivery.
#[tokio::test]
async fn a_load_balancer_with_logging_off_delivers_no_record() {
    let (router, engine) = emulator();
    log_bucket(&router).await;
    let (lb_arn, published) = create_load_balancer(&router, "alb-access-log-off").await;
    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", &lb_arn));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", "80"));
    form.push(p("DefaultActions.member.1.Type", "fixed-response"));
    form.push(p(
        "DefaultActions.member.1.FixedResponseConfig.StatusCode",
        "200",
    ));
    form.push(p(
        "DefaultActions.member.1.FixedResponseConfig.MessageBody",
        "SERVED",
    ));
    let reply = call(&router, &form).await;
    assert_eq!(
        reply.status, 200,
        "setup CreateListener: expected 200, got {} body {}",
        reply.status, reply.body
    );
    settle(&engine, &lb_arn, SystemTime::now() - Duration::from_secs(1));

    // The rows differ along the attribute collection the load balancer carries when the
    // request is made: its documented default, logging on with the bucket named, and logging
    // switched off again with the bucket still named.
    let rows: &[LoggingRow] = &[
        LoggingRow {
            case: "the attribute's documented default",
            attributes: &[],
            records: 0,
        },
        LoggingRow {
            case: "logging on, naming the bucket",
            attributes: &[(ENABLED_ATTRIBUTE, "true"), (BUCKET_ATTRIBUTE, LOG_BUCKET)],
            records: 1,
        },
        LoggingRow {
            case: "logging switched off again",
            attributes: &[(ENABLED_ATTRIBUTE, "false")],
            records: 1,
        },
    ];
    for row in rows {
        let LoggingRow {
            case,
            attributes,
            records,
        } = row;
        if !attributes.is_empty() {
            let mut form = action("ModifyLoadBalancerAttributes");
            form.push(p("LoadBalancerArn", &lb_arn));
            for (index, (key, value)) in attributes.iter().enumerate() {
                let position = index + 1;
                form.push(p(&format!("Attributes.member.{position}.Key"), key));
                form.push(p(&format!("Attributes.member.{position}.Value"), value));
            }
            let reply = call(&router, &form).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected ModifyLoadBalancerAttributes to answer 200, got {} \
                 body {}",
                reply.status, reply.body
            );
        }
        let answered = exchange(case, &published, "/served").await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: expected the listener's own fixed response, got {:?}",
            answered.raw
        );
        let held = delivered(&router).await;
        assert_eq!(
            held.len(),
            *records,
            "case {case:?}: expected {records} record(s) in the bucket, got {}: {:?}",
            held.len(),
            held.iter().map(|(key, _)| key).collect::<Vec<_>>()
        );
    }
}
