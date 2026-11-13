// SPDX-License-Identifier: CC0-1.0

//! The wire harness the suites in this target share.
//!
//! Every call goes through the same `MultiServiceRouter::dispatch` the emulator's front
//! door uses: `POST /` with `X-Amz-Target: <Prefix>.<Operation>` and an `awsJson` body.
//! Nothing here reaches into the state struct, so what a suite built on it proves is what a
//! client sees — and one definition of the dispatch shape means no two suites can drift
//! apart on the header set or the routing credential.
//!
//! Compute Optimizer does not stand alone: an Amazon ECS service recommendation is computed
//! over the ECS service the ECS emulator holds and the `AWS/ECS` utilization the CloudWatch
//! emulator holds, so [`emulator_with_analysed_services`] mounts all three on ONE engine —
//! exactly what `core/router` assembles. An Amazon Aurora or RDS database recommendation is
//! computed the same way over the DB instances the Amazon RDS emulator holds and the
//! `AWS/RDS` utilization published for them, which is what
//! [`emulator_with_analysed_databases`] mounts. Amazon S3 is mounted by every emulator here
//! for the same reason: a recommendations export is written into a bucket of the one object
//! store this emulator holds, so an export suite CREATES its destination bucket over S3's own
//! wire ([`create_bucket`]) and reads the exported objects back over it ([`get_object`]).

use std::collections::BTreeMap;
use std::sync::Arc;

use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::ops::types::RESOURCE_TYPE_VALUES;
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    service_state, METRIC_STATISTIC_MAXIMUM, RESOURCE_TYPE_AUTO_SCALING_GROUP, SCOPE_ACCOUNT_ID,
    SCOPE_RESOURCE_ARN, ANALYSIS_DELAY,
};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The one account this service answers for, and so the account that owns every resource
/// the suites name: this emulator presents exactly ONE standalone account
/// (`engine::emulator_account_id`) and records no organization, so a request naming another
/// account's resource — in `accountIds`, in a resource ARN, or in an `AccountId` scope — is
/// refused rather than answered. A `Scope` of `AccountId` must be a 12-digit account id
/// (the model's `Scope$value` doc), and an account-level preference applies to the resources
/// of that account.
pub(crate) const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// An EC2 instance ARN — one of the two ARN forms the preference API accepts.
pub(crate) fn instance_arn(instance_id: &str) -> String {
    format!("arn:aws:ec2:us-east-1:{ACCOUNT}:instance/{instance_id}")
}

/// An Auto Scaling group ARN — the other accepted form.
pub(crate) fn auto_scaling_group_arn(name: &str) -> String {
    format!(
        "arn:aws:autoscaling:us-east-1:{ACCOUNT}:autoScalingGroup:\
         12345678-1234-1234-1234-123456789012:autoScalingGroupName/{name}"
    )
}

/// The region every request in these suites signs for; every ARN the ECS emulator mints
/// carries it, because the SigV4 credential scope is what names a request's region.
pub(crate) const REGION: &str = "us-east-1";

/// One emulator: one engine, one shared store, reached only over HTTP.
pub(crate) fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// The same emulator, with the engine it was built on — for a suite that has to RECORD
/// analysis output (an idle resource recommendation, say) that no operation of this API
/// creates, and then read it back over the wire like any client.
pub(crate) fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// Compute Optimizer, Amazon ECS and CloudWatch over ONE engine: the emulator a suite needs
/// to drive a recommendation that is computed over another service's resources and the
/// utilization published for them.
pub(crate) fn emulator_with_analysed_services() -> MultiServiceRouter {
    emulator_with_analysed_services_and_engine().0
}

/// The subnet the analysed ECS services are placed in ([`ANALYSED_SUBNET`]), and the VPC it
/// is in, in the account's own Amazon EC2 store.
///
/// An ECS service of the `awsvpc` network mode — which a Fargate service is — is placed in
/// subnets Amazon ECS RESOLVES against Amazon VPC's records before it creates the service,
/// so a suite that creates one has to hold the subnet its placement names. The records are
/// the modelled shapes that service reports.
fn register_analysed_network(engine: &engine::InMemoryEngine) {
    use service_com_amazonaws_ec2_amazonec2 as ec2;

    let ec2_state = ec2::state::service_state(engine)
        .expect("the in-memory engine carries the account's Amazon EC2 store");
    let mut store = ec2_state.lock();
    store.vpcs.insert(
        ANALYSED_VPC.to_string(),
        ec2::ops::types::Vpc {
            vpc_id: Some(ANALYSED_VPC.to_string()),
            cidr_block: Some("10.0.0.0/16".to_string()),
            state: Some(ec2::state::VPC_STATE_AVAILABLE.to_string()),
            ..Default::default()
        },
    );
    store.subnets.insert(
        ANALYSED_SUBNET.to_string(),
        ec2::ops::types::Subnet {
            subnet_id: Some(ANALYSED_SUBNET.to_string()),
            vpc_id: Some(ANALYSED_VPC.to_string()),
            cidr_block: Some("10.0.0.0/24".to_string()),
            availability_zone: Some(format!("{REGION}a")),
            state: Some(ec2::state::SUBNET_STATE_AVAILABLE.to_string()),
            ..Default::default()
        },
    );
}

/// The subnet every analysed ECS service below is placed in.
pub(crate) const ANALYSED_SUBNET: &str = "subnet-0123456789abcdef0";

/// The VPC [`ANALYSED_SUBNET`] is in.
const ANALYSED_VPC: &str = "vpc-0123456789abcdef0";

/// The same emulator, with the engine it was built on — for a suite that has to seed a
/// CloudWatch series of the AWS-reserved namespace space ([`seed_cloudwatch_series`]), which
/// no client call can write.
pub(crate) fn emulator_with_analysed_services_and_engine(
) -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    register_analysed_network(&engine);
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// Compute Optimizer, Amazon RDS and CloudWatch over ONE engine: the emulator a suite needs
/// to drive an Amazon Aurora or RDS database recommendation, which is computed over the DB
/// instances the Amazon RDS emulator holds and the `AWS/RDS` utilization published for them.
pub(crate) fn emulator_with_analysed_databases() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>)
{
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_rds_amazonrdsv19::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// One Amazon RDS call at the endpoint of `region`: an `awsQuery` `POST /` whose body carries
/// the `Action`, the API `Version` and the request's parameters, as an SDK sends one.
pub(crate) async fn call_rds_in(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    params: &[(&str, String)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2014-10-31");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("https://rds.amazonaws.com/")
        .header("content-type", "application/x-www-form-urlencoded")
        // The SigV4 credential scope is what routes a request to its service, and the region
        // it names is the regional endpoint the request addressed.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{region}/rds/aws4_request, \
                 SignedHeaders=host, Signature=not-verified"
            ),
        )
        .body(Bytes::from(body))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8(response.body().to_vec()).expect("a UTF-8 response body");
    (status, body)
}

/// One Amazon RDS call at the endpoint every suite here signs for ([`REGION`]).
pub(crate) async fn call_rds(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, String)],
) -> (u16, String) {
    call_rds_in(router, REGION, action, params).await
}

/// One form value as a client puts it in an `application/x-www-form-urlencoded` body: every
/// byte outside the unreserved set (`A-Z`, `a-z`, `0-9`, `-`, `_`, `.`, `~`) is
/// percent-escaped.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(char::from(byte));
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// The endpoint every Amazon S3 request below is addressed to (its `Host`), which is what
/// makes it a PATH-STYLE request: `PUT /<bucket>`, `GET /<bucket>/<key>`.
const OBJECT_ENDPOINT_HOST: &str = "127.0.0.1:9297";

/// Create `bucket` in the one object store this emulator holds, over Amazon S3's own
/// `CreateBucket`.
///
/// "You must create the destination Amazon S3 bucket for your recommendations export before
/// you create the export job. Compute Optimizer does not create the S3 bucket for you"
/// (`S3DestinationConfig`), so an export suite creates its destination the way a client does
/// — through S3 — rather than reaching into the object store.
pub(crate) async fn create_bucket(router: &MultiServiceRouter, bucket: &str) {
    let (status, body) = object_request(router, "PUT", &format!("/{bucket}")).await;
    assert!(
        (200..300).contains(&status),
        "creating bucket {bucket}: expected 2xx, got {status}: {}",
        String::from_utf8_lossy(&body)
    );
}

/// `GET /<bucket>/<key>` — Amazon S3's `GetObject`, answering the status and the object's
/// bytes, which is how a client downloads an exported recommendations file.
pub(crate) async fn get_object(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
) -> (u16, Vec<u8>) {
    object_request(router, "GET", &format!("/{bucket}/{key}")).await
}

/// `DELETE /<bucket>` — Amazon S3's `DeleteBucket`, for a suite that has to take the
/// destination of an export away again.
pub(crate) async fn delete_bucket(router: &MultiServiceRouter, bucket: &str) -> u16 {
    object_request(router, "DELETE", &format!("/{bucket}")).await.0
}

/// One path-style Amazon S3 request, answering its status and body.
async fn object_request(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
) -> (u16, Vec<u8>) {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{OBJECT_ENDPOINT_HOST}{path}"))
        .header("host", OBJECT_ENDPOINT_HOST)
        // The SigV4 credential scope's service field is what routes a request to S3.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{REGION}/s3/aws4_request, \
                 SignedHeaders=host, Signature=not-verified"
            ),
        )
        .body(Bytes::new())
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, response.body().to_vec())
}

/// Write datapoints straight into the CloudWatch metric store, the way the AWS service that
/// OWNS the metric writes them.
///
/// A client cannot publish into the AWS-reserved namespace space: `PutMetricData` refuses a
/// `Namespace` beginning with `AWS/` ("To avoid conflicts with Amazon Web Services service
/// namespaces, you should not specify a namespace that begins with AWS/"), and the
/// utilization Compute Optimizer analyses is exactly such a series (`AWS/ECS`), authored by
/// the service that owns the resource. So a suite seeds it here and reads the recommendation
/// computed from it over the wire, like any client.
pub(crate) fn seed_cloudwatch_series(
    engine: &Arc<engine::InMemoryEngine>,
    namespace: &str,
    metric_name: &str,
    dimensions: &[(&str, &str)],
    unit: Option<&str>,
    at_secs: f64,
    values: &[f64],
) {
    use service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state as cloudwatch;

    let key = cloudwatch::MetricKey {
        namespace: namespace.to_string(),
        metric_name: metric_name.to_string(),
        dimensions: dimensions
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect(),
    };
    let data: Vec<cloudwatch::StoredDatum> = values
        .iter()
        .map(|value| cloudwatch::StoredDatum {
            timestamp_secs: at_secs,
            sample_count: 1.0,
            sum: *value,
            minimum: *value,
            maximum: *value,
            unit: unit.map(ToString::to_string),
            // The default `StorageResolution` of a published datum.
            storage_resolution: 60,
        })
        .collect();
    engine
        .state::<cloudwatch::ComAmazonawsCloudwatchGraniteserviceversion20100801State>()
        .lock()
        .append_datapoints(key, data)
        .expect("the metric store has room for the seeded series");
}

/// Call one Compute Optimizer operation and return the HTTP status and the parsed JSON body.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    call_in_region(router, REGION, operation, body).await
}

/// The same call at the endpoint of `region` — the region a client's SigV4 credential scope
/// names — for a suite that asserts what a regional endpoint reports.
pub(crate) async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    dispatch(
        router,
        region,
        "compute-optimizer",
        "application/x-amz-json-1.0",
        &format!("ComputeOptimizerService.{operation}"),
        body,
    )
    .await
}

/// Call one Amazon ECS operation (`awsJson1_1`).
pub(crate) async fn call_ecs(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    dispatch(
        router,
        REGION,
        "ecs",
        "application/x-amz-json-1.1",
        &format!("AmazonEC2ContainerServiceV20141113.{operation}"),
        body,
    )
    .await
}

/// Dispatch one request, returning the HTTP status and the parsed JSON body.
async fn dispatch(
    router: &MultiServiceRouter,
    region: &str,
    signing_name: &str,
    content_type: &str,
    target: &str,
    body: Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", content_type)
        .header("x-amz-target", target)
        // The SigV4 credential scope is what routes a request to its service, and the
        // region it names is the regional endpoint the request addressed.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{region}/{signing_name}/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=not-verified"
            ),
        )
        .body(Bytes::from(body.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code (`__type`) of an error response.
pub(crate) fn error_code(body: &Value) -> String {
    body["__type"].as_str().unwrap_or_default().to_string()
}

/// The `s3Destination` members an accepted `Export*Recommendations` call answered:
/// (bucket, key, metadataKey). Shared so no export suite reads the destination differently.
pub(crate) fn export_destination(case: &str, body: &Value) -> (String, String, String) {
    let destination = &body["s3Destination"];
    let member = |name: &str| {
        destination[name]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: an accepted export answers s3Destination.{name}, got {body}")
            })
            .to_string()
    };
    (member("bucket"), member("key"), member("metadataKey"))
}

/// One export file as rows of cells, read with the same `csv` codec it is written with.
pub(crate) fn csv_rows(document: &str) -> Vec<Vec<String>> {
    ::csv::ReaderBuilder::new()
        .has_headers(false)
        .from_reader(document.as_bytes())
        .records()
        .map(|record| {
            let record = record
                .unwrap_or_else(|error| panic!("the export file is comma-separated values: {error}"));
            record.iter().map(ToString::to_string).collect()
        })
        .collect()
}

/// The cell of `row` under `column`, by the header the document carries.
pub(crate) fn cell<'a>(header: &[String], row: &'a [String], column: &str) -> &'a str {
    header
        .iter()
        .position(|held| held == column)
        .and_then(|at| row.get(at))
        .map_or("", String::as_str)
}

/// A list-valued member of a reported recommendation as the strings it holds; the empty list
/// for a member reporting none, so a suite reads a member the response omits the same way it
/// reads an empty one.
pub(crate) fn strings(reported: &Value) -> Vec<String> {
    reported
        .as_array()
        .map(|values| {
            values
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The utilization metrics a reported member carries, by metric name, under the ONE statistic
/// this API reports them with ([`METRIC_STATISTIC_MAXIMUM`]): "the Compute Optimizer API,
/// Command Line Interface (CLI), and SDKs return utilization metrics using only the `Maximum`
/// statistic".
pub(crate) fn metrics(reported: &Value) -> BTreeMap<String, f64> {
    reported
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter(|metric| metric["statistic"].as_str() == Some(METRIC_STATISTIC_MAXIMUM))
                .filter_map(|metric| {
                    let name = metric["name"].as_str()?.to_string();
                    Some((name, metric["value"].as_f64().unwrap_or_default()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Assert the cell of `row` under `column` holds `expected`, as the figure every export cell is
/// rounded to — so an export suite compares a cell against a figure a recommendation reader
/// reported rather than against its text.
pub(crate) fn assert_figure(
    case: &str,
    header: &[String],
    row: &[String],
    column: &str,
    expected: f64,
) {
    let held = cell(header, row, column);
    let actual: f64 = held.parse().unwrap_or_else(|_| {
        panic!("case {case:?}: {column} holds a figure, got {held:?} in {row:?}")
    });
    assert!(
        (actual - expected).abs() < 1e-6,
        "case {case:?}: {column} must hold the figure the reader reports ({expected}), got \
         {actual} in {row:?}"
    );
}

/// Opt the account in, which every preference operation requires.
pub(crate) async fn opt_in(router: &MultiServiceRouter) {
    let (status, body) = call(
        router,
        "UpdateEnrollmentStatus",
        json!({"status": "Active", "includeMemberAccounts": false}),
    )
    .await;
    assert_eq!(status, 200, "opting in: expected 200, got {status}: {body}");
}

/// Make the analysis the account's enrollment started DUE, without waiting for the clock to
/// reach the instant it becomes due.
///
/// The service derives that instant from the stored enrollment record — `ANALYSIS_DELAY`
/// after `Enrollment::last_updated_timestamp`, which is what every reader gates its records
/// on (`Enrollment::analysis_visible`) — so a suite that needs the analysis READABLE moves
/// the stored instant back a span instead of sleeping for one: the condition is then true
/// however fast or loaded the host is, and nothing in the suite depends on elapsed time.
pub(crate) fn make_analysis_due(engine: &Arc<engine::InMemoryEngine>) {
    let state = service_state(&**engine).expect("the engine bears this service's state");
    let mut store = state.lock();
    let written = store
        .enrollment
        .last_updated_timestamp
        .as_ref()
        .map(DateTime::as_secs_f64)
        .expect("the account must be opted in before its analysis can be made due");
    store.enrollment.last_updated_timestamp =
        Some(DateTime::from_secs_f64(written - ANALYSIS_DELAY.as_secs_f64()));
}

/// Read the recommendation summaries, asserting the call succeeded. The summary set is
/// answered whole, so ONE call is the whole set.
pub(crate) async fn summary_set(
    router: &MultiServiceRouter,
    case: &str,
    request: Value,
) -> Value {
    let (status, body) = call(router, "GetRecommendationSummaries", request.clone()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the summaries for {request} must be served, got {status}: {body}"
    );
    body
}

/// The resource types a summary answer reports, in the order it reports them.
pub(crate) fn summary_resource_types(case: &str, body: &Value) -> Vec<String> {
    body["recommendationSummaries"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: recommendationSummaries must be a list, got {body}")
        })
        .iter()
        .map(|summary| {
            summary["recommendationResourceType"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The summary of one resource type in an answer.
pub(crate) fn summary_of<'a>(case: &str, body: &'a Value, resource_type: &str) -> &'a Value {
    body["recommendationSummaries"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .find(|summary| summary["recommendationResourceType"] == json!(resource_type))
        .unwrap_or_else(|| panic!("case {case:?}: {resource_type} must be summarised, got {body}"))
}

/// Write one preference, asserting it was accepted.
pub(crate) async fn put_preference(router: &MultiServiceRouter, request: &Value) {
    let (status, body) = call(router, "PutRecommendationPreferences", request.clone()).await;
    assert_eq!(
        status, 200,
        "put {request}: expected 200, got {status}: {body}"
    );
    assert_eq!(
        body,
        json!({}),
        "put {request}: PutRecommendationPreferences has an empty output, got {body}"
    );
}

/// Every value of the model's `ResourceType` enum — the type a preference request's
/// `resourceType` member carries — taken from the crate's generated value set rather than
/// copied, so a resource type the model adds is covered the day it appears
/// (`RESOURCE_TYPE_VALUES`: AuroraDBClusterStorage | AutoScalingGroup | EbsVolume |
/// Ec2Instance | EcsService | Idle | LambdaFunction | License | NotApplicable |
/// RdsDBInstance).
pub(crate) const RESOURCE_TYPES: &[&str] = RESOURCE_TYPE_VALUES;

/// The `ResourceType` values whose recommendations this service analyses per resource, which
/// `PutRecommendationPreferences$lookBackPeriod` enumerates: "You can only set this preference
/// for the Amazon EC2 instance, Auto Scaling group, Amazon EBS volume, Amazon ECS service on
/// Fargate, Amazon RDS DB instance, and Aurora DB cluster storage resource types" —
/// `preferredResources` documents the same six, in the order the service's refusal lists them.
/// Stated once here so no suite that asserts a preference's resource types can drift from
/// another on them.
pub(crate) const ANALYSED_PER_RESOURCE_TYPES: &[&str] = &[
    "Ec2Instance",
    "AutoScalingGroup",
    "EbsVolume",
    "EcsService",
    "RdsDBInstance",
    "AuroraDBClusterStorage",
];

/// The scope a preference for `resource_type` may be written at.
///
/// An Auto Scaling group preference "can only be set at the resource level by specifying a
/// scope name of `ResourceArn`" (`PutRecommendationPreferences$scope`); every other
/// resource type takes the account-level scope, whose "value must be a 12-digit Amazon Web
/// Services account ID" (`Scope$value`). Stated once here so no suite that walks the whole
/// enum can drift from another on it.
pub(crate) fn scope_for(resource_type: &str) -> Value {
    if resource_type == RESOURCE_TYPE_AUTO_SCALING_GROUP {
        json!({"name": SCOPE_RESOURCE_ARN, "value": auto_scaling_group_arn("batch-tier")})
    } else {
        json!({"name": SCOPE_ACCOUNT_ID, "value": ACCOUNT})
    }
}

/// Read the preference records stored for `resource_type`, optionally at one scope.
pub(crate) async fn get_preferences(
    router: &MultiServiceRouter,
    resource_type: &str,
    scope: Option<&Value>,
) -> Value {
    let mut request = json!({"resourceType": resource_type});
    if let Some(scope) = scope {
        request["scope"] = scope.clone();
    }
    let (status, body) = call(router, "GetRecommendationPreferences", request.clone()).await;
    assert_eq!(
        status, 200,
        "get {request}: expected 200, got {status}: {body}"
    );
    body
}

/// The effective preferences in force for one resource.
pub(crate) async fn effective_for(router: &MultiServiceRouter, resource_arn: &str) -> Value {
    let (status, body) = call(
        router,
        "GetEffectiveRecommendationPreferences",
        json!({"resourceArn": resource_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "effective preferences for {resource_arn}: expected 200, got {status}: {body}"
    );
    body
}
