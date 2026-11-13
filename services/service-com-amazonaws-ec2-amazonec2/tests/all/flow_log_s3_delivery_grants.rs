// SPDX-License-Identifier: CC0-1.0

//! The permissions an Amazon S3 flow log leaves on its DESTINATION BUCKET, where Amazon S3
//! owns them.
//!
//! "When publishing to Amazon S3, flow log records for all of the monitored network
//! interfaces are published to a single log file object that is stored in the specified
//! bucket" (`CreateFlowLogs`) — written by the log delivery service, not by the caller, so
//! the destination bucket's own policy has to permit that service before any record could
//! land. The rules under test, each over a SET of inputs rather than on one request:
//!
//! 1. A CREATE WITH AN S3 DESTINATION LEAVES THE BUCKET GRANTING THE LOG DELIVERY SERVICE:
//!    its policy carries one `s3:PutObject` statement over the path the records land under
//!    and one `s3:GetBucketAcl` statement over the bucket itself, both to
//!    `delivery.logs.amazonaws.com`, both scoped to this account's own logging.
//! 2. THE GRANTED PATH FOLLOWS THE DESTINATION: the optional subfolder of the destination
//!    ARN, then the segment the delivery writes under, then the account — written as a Hive
//!    partition when the flow log asked for Hive-compatible prefixes.
//! 3. THE GRANTS ARE MERGED, NOT SUBSTITUTED: a policy the bucket's owner wrote keeps its
//!    statements verbatim, and a second flow log delivering to the same bucket is covered
//!    BESIDE the first rather than instead of it.
//! 4. A FLOW LOG THAT PUBLISHES ANYWHERE ELSE TOUCHES NO BUCKET POLICY: a CloudWatch Logs or
//!    Firehose destination leaves the bucket exactly as it was.
//!
//! Both services are mounted on ONE shared engine, exactly as the assembled emulator mounts
//! them, so the bucket is created through Amazon S3's own wire and its policy is read back
//! from the state Amazon S3 serves its own `GetBucketPolicy` from.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements};

/// The Region most of this suite signs for, and the Region an Amazon S3 `CreateBucket` with
/// no `CreateBucketConfiguration` places a bucket in.
const REGION: &str = "us-east-1";

/// A second Region, so the logging the grant is scoped to is pinned as a function of the
/// Region the flow log was created in rather than as one fixed string.
const OTHER_REGION: &str = "eu-west-1";

/// The service principal AWS delivers a flow log's records as — the identity the destination
/// bucket's policy has to grant. Written out here rather than read from the crate so this
/// suite cannot drift from the implementation's own spelling.
const LOG_DELIVERY_PRINCIPAL: &str = "delivery.logs.amazonaws.com";

/// The action the records are written with.
const PUT_OBJECT: &str = "s3:PutObject";

/// The action the delivery reads the destination bucket's ACL with.
const GET_BUCKET_ACL: &str = "s3:GetBucketAcl";

/// The key segment every delivered flow log object sits under: "Note that you can't use
/// `AWSLogs` as a subfolder name" (`CreateFlowLogsRequest$LogDestination` —
/// `smithy_query(query="doc",shape="CreateFlowLogsRequest",member="LogDestination")`).
const DELIVERED_SEGMENT: &str = "AWSLogs";

/// One `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One row of the destination table: the case, the Region the flow log is created in, the
/// destination bucket, the subfolder of the destination ARN, whether that ARN carries the
/// trailing slash, and the Hive-compatible prefixes the request asks for (`None` for a
/// request carrying no destination options at all).
type DestinationRow = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    bool,
    Option<bool>,
);

/// EC2 and Amazon S3 over ONE shared engine, exactly as the assembled router mounts them: a
/// flow log's destination bucket, and the policy on it, are resources of that service.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signature verification is the emulator's own concern and is exercised elsewhere;
        // these cases are about these services' behaviour.
        router.set_request_authentication(false);
        router.register_service(ec2::EmulatedService::new(Arc::clone(&engine)));
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            engine,
        ));
        Self { router }
    }

    /// The SigV4 `Authorization` header of a request signed for `service` in `region`, which
    /// is how the router picks the service a request belongs to.
    fn authorization(service: &str, region: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTEC2/20260101/{region}/{service}/\
             aws4_request, SignedHeaders=host;x-amz-date, Signature=00"
        )
    }

    /// One of EC2's own ec2Query actions, signed for `region`.
    async fn ec2_call(
        &self,
        region: &str,
        action: &str,
        params: &[(String, String)],
    ) -> (u16, String) {
        call(&self.router, region, action, params).await
    }

    /// One VPC of the account, through EC2's own `CreateVpc` — the resource a flow log
    /// monitors has to be one this account holds.
    async fn given_vpc(&self, region: &str, cidr_block: &str) -> String {
        let (status, body) = self
            .ec2_call(region, "CreateVpc", &[p("CidrBlock", cidr_block)])
            .await;
        assert_eq!(
            status, 200,
            "CreateVpc({cidr_block}) must answer 200, got {status} with body {body}"
        );
        element(&body, "vpcId")
            .unwrap_or_else(|| panic!("CreateVpc({cidr_block}) reported no vpcId in {body}"))
    }

    /// Create a bucket through Amazon S3's own (REST) wire, path style, with the
    /// `CreateBucketConfiguration` Amazon S3 requires outside its default Region.
    async fn given_bucket(&self, region: &str, name: &str) {
        let body = if region == REGION {
            Bytes::new()
        } else {
            Bytes::from(format!(
                "<CreateBucketConfiguration \
                 xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
                 <LocationConstraint>{region}</LocationConstraint>\
                 </CreateBucketConfiguration>"
            ))
        };
        let request = http::Request::builder()
            .method("PUT")
            .uri(format!("/{name}"))
            .header("host", format!("s3.{region}.amazonaws.com"))
            .header("authorization", Self::authorization("s3", region))
            .body(body)
            .expect("the test request is well formed");
        let (status, payload) = self.dispatch(request).await;
        assert_eq!(
            status, 200,
            "CreateBucket({name}) must succeed, got {status} with {payload}"
        );
    }

    /// Attach `policy` to `name` through Amazon S3's own `PutBucketPolicy`, so the merge has
    /// a policy of the bucket owner's own to merge into.
    async fn given_bucket_policy(&self, region: &str, name: &str, policy: &Value) {
        let request = http::Request::builder()
            .method("PUT")
            .uri(format!("/{name}?policy"))
            .header("host", format!("s3.{region}.amazonaws.com"))
            .header("authorization", Self::authorization("s3", region))
            .body(Bytes::from(policy.to_string()))
            .expect("the test request is well formed");
        let (status, payload) = self.dispatch(request).await;
        assert_eq!(
            status, 204,
            "PutBucketPolicy({name}) must succeed, got {status} with {payload}"
        );
    }

    /// The policy `name` carries right now, read through Amazon S3's own `GetBucketPolicy`:
    /// the status and the document.
    async fn bucket_policy(&self, region: &str, name: &str) -> (u16, Value) {
        let request = http::Request::builder()
            .method("GET")
            .uri(format!("/{name}?policy"))
            .header("host", format!("s3.{region}.amazonaws.com"))
            .header("authorization", Self::authorization("s3", region))
            .body(Bytes::new())
            .expect("the test request is well formed");
        self.dispatch(request).await
    }

    /// The document `name` must be carrying, as a policy — a read that insists on 200.
    async fn policy_of(&self, case: &str, region: &str, name: &str) -> Value {
        let (status, document) = self.bucket_policy(region, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the bucket {name} must carry a policy, got {status} {document}"
        );
        document
    }

    async fn dispatch(&self, request: http::Request<Bytes>) -> (u16, Value) {
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.into_body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).to_string()))
        };
        (status, parsed)
    }
}

/// One `CreateFlowLogs` that must succeed, answering the one flow log id it minted.
async fn created_flow_log(
    emulator: &Emulator,
    case: &str,
    region: &str,
    params: &[(String, String)],
) -> String {
    let (status, body) = emulator.ec2_call(region, "CreateFlowLogs", params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateFlowLogs must answer 200, got {status} with body {body}"
    );
    let ids = elements(&body, "item");
    assert_eq!(
        ids.len(),
        1,
        "case {case:?}: CreateFlowLogs must report exactly one flow log id, got {ids:?} \
         in {body}"
    );
    ids[0].clone()
}

/// The members a create carries for an S3 destination: the monitored VPC, the traffic type
/// `TrafficType` requires of a VPC, and the destination itself.
fn s3_params(vpc_id: &str, log_destination: &str, hive: Option<bool>) -> Vec<(String, String)> {
    let mut params = vec![
        p("ResourceType", "VPC"),
        p("ResourceId.1", vpc_id),
        p("TrafficType", "ALL"),
        p("LogDestinationType", "s3"),
        p("LogDestination", log_destination),
    ];
    if let Some(hive) = hive {
        params.push(p("DestinationOptions.FileFormat", "plain-text"));
        params.push(p("DestinationOptions.HiveCompatiblePartitions", hive.to_string()));
        params.push(p("DestinationOptions.PerHourPartition", "false"));
    }
    params
}

/// The statements a bucket policy document holds.
fn statements(document: &Value) -> Vec<Value> {
    document["Statement"].as_array().cloned().unwrap_or_default()
}

/// Whether a policy element written as one string or a list of them names `value`.
fn names(element: &Value, value: &str) -> bool {
    match element {
        Value::String(named) => named == value,
        Value::Array(named) => named.iter().any(|named| named == value),
        _ => false,
    }
}

/// The statements of `document` ALLOWING `action` to the log delivery service.
fn granting(document: &Value, action: &str) -> Vec<Value> {
    statements(document)
        .into_iter()
        .filter(|statement| {
            statement["Effect"] == json!("Allow")
                && statement["Principal"]["Service"] == json!(LOG_DELIVERY_PRINCIPAL)
                && names(&statement["Action"], action)
        })
        .collect()
}

/// The locations a statement's `Resource` names, in the order it names them.
fn resources(statement: &Value) -> Vec<String> {
    match &statement["Resource"] {
        Value::String(named) => vec![named.clone()],
        Value::Array(named) => named
            .iter()
            .filter_map(|entry| entry.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// The one statement of `document` granting `action`, which there must be exactly one of.
fn one_grant(case: &str, document: &Value, action: &str) -> Value {
    let found = granting(document, action);
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: the bucket must grant {action} to {LOG_DELIVERY_PRINCIPAL} exactly \
         once, got {document}"
    );
    found[0].clone()
}
/// The path AWS delivers a flow log's records to, as this suite states it independently of
/// the implementation: the bucket, the destination's own subfolder, the segment the delivery
/// writes under, and the account — as a Hive partition when the flow log asked for one.
fn delivered_path(bucket: &str, subfolder: &str, hive: bool) -> String {
    let account = ::engine::emulator_account_id();
    let account_segment = if hive {
        format!("aws-account-id={account}")
    } else {
        account.to_string()
    };
    let subfolder = if subfolder.is_empty() {
        String::new()
    } else {
        format!("{subfolder}/")
    };
    format!("arn:aws:s3:::{bucket}/{subfolder}{DELIVERED_SEGMENT}/{account_segment}/*")
}

/// Assert that `document` grants the log delivery service what a flow log's delivery needs:
/// the object write over exactly the locations `delivered` names, one statement each, and the
/// bucket-ACL read over the bucket itself, all scoped to this account's logging in `region`.
fn assert_grants_delivery(
    case: &str,
    document: &Value,
    region: &str,
    bucket: &str,
    delivered: &[String],
) {
    let account = ::engine::emulator_account_id();
    let source_arn = format!("arn:aws:logs:{region}:{account}:*");

    let writes = granting(document, PUT_OBJECT);
    let granted: Vec<String> = writes.iter().flat_map(resources).collect();
    assert_eq!(
        granted,
        delivered.to_vec(),
        "case {case:?}: the delivery must be granted the write over {delivered:?}, \
         got {document}"
    );
    for write in &writes {
        assert_eq!(
            write["Effect"],
            json!("Allow"),
            "case {case:?}: a write grant must allow, got {write}"
        );
        assert_eq!(
            write["Condition"]["StringEquals"]["s3:x-amz-acl"],
            json!("bucket-owner-full-control"),
            "case {case:?}: a write grant must require the owner-controlled ACL, got {write}"
        );
        assert_eq!(
            write["Condition"]["StringEquals"]["aws:SourceAccount"],
            json!(account),
            "case {case:?}: a write grant must scope the source account, got {write}"
        );
        assert_eq!(
            write["Condition"]["ArnLike"]["aws:SourceArn"],
            json!(source_arn),
            "case {case:?}: a write grant must scope the source ARN to {source_arn}, \
             got {write}"
        );
    }

    let acl_check = one_grant(case, document, GET_BUCKET_ACL);
    assert_eq!(
        acl_check["Effect"],
        json!("Allow"),
        "case {case:?}: the ACL-check grant must allow, got {acl_check}"
    );
    assert_eq!(
        resources(&acl_check),
        vec![format!("arn:aws:s3:::{bucket}")],
        "case {case:?}: the ACL-check grant must name the bucket itself, got {acl_check}"
    );
    assert_eq!(
        acl_check["Condition"]["StringEquals"]["aws:SourceAccount"],
        json!(account),
        "case {case:?}: the ACL-check grant must scope the source account, got {acl_check}"
    );
    assert_eq!(
        acl_check["Condition"]["ArnLike"]["aws:SourceArn"],
        json!(source_arn),
        "case {case:?}: the ACL-check grant must scope the source ARN to {source_arn}, \
         got {acl_check}"
    );
}

/// RULES 1 and 2: a create with an S3 destination leaves the bucket granting the delivery,
/// over the path the records land under.
///
/// One row per FORM of the destination and of the layout: the bucket alone, the optional
/// subfolder the member documents, a subfolder of several segments, the subfolder with and
/// without its trailing slash, Hive-compatible prefixes on and off (and absent), and a flow
/// log created in another Region, since the logging the grant is scoped to is a function of
/// that Region.
#[tokio::test]
async fn an_s3_flow_log_leaves_the_bucket_granting_the_log_delivery_service() {
    // (case, the Region, the bucket, the subfolder of the destination ARN, the trailing
    // slash the ARN carries, Hive-compatible prefixes as the request asks for them)
    let cases: Vec<DestinationRow> = vec![
        (
            "the bucket alone, with no destination options at all",
            REGION,
            "flow-logs-plain",
            "",
            false,
            None,
        ),
        (
            "the documented subfolder, with its trailing slash",
            REGION,
            "flow-logs-subfolder",
            "vpc",
            true,
            Some(false),
        ),
        (
            "a subfolder of several segments and no trailing slash",
            REGION,
            "flow.logs-nested",
            "vpc/prod",
            false,
            Some(false),
        ),
        (
            "Hive-compatible prefixes under a subfolder",
            REGION,
            "flow-logs-hive",
            "vpc",
            true,
            Some(true),
        ),
        (
            "Hive-compatible prefixes over the bucket itself",
            REGION,
            "flow-logs-hive-plain",
            "",
            false,
            Some(true),
        ),
        (
            "a flow log created in another Region",
            OTHER_REGION,
            "flow-logs-elsewhere",
            "vpc",
            true,
            Some(false),
        ),
    ];

    for (case, region, bucket, subfolder, trailing_slash, hive) in cases {
        // One emulator per row, so each row decides against a bucket holding only its own
        // flow log's grants.
        let emulator = Emulator::new();
        emulator.given_bucket(region, bucket).await;
        let vpc_id = emulator.given_vpc(region, "10.2.0.0/16").await;

        let mut log_destination = format!("arn:aws:s3:::{bucket}");
        if !subfolder.is_empty() {
            log_destination.push('/');
            log_destination.push_str(subfolder);
            if trailing_slash {
                log_destination.push('/');
            }
        }
        let flow_log_id = created_flow_log(
            &emulator,
            case,
            region,
            &s3_params(&vpc_id, &log_destination, hive),
        )
        .await;
        assert!(
            flow_log_id.starts_with("fl-"),
            "case {case:?}: the create must mint a flow log id, got {flow_log_id:?}"
        );

        let document = emulator.policy_of(case, region, bucket).await;
        let delivered = vec![delivered_path(bucket, subfolder, hive.unwrap_or(false))];
        assert_grants_delivery(case, &document, region, bucket, &delivered);
        assert_eq!(
            statements(&document).len(),
            2,
            "case {case:?}: the bucket must carry exactly the two delivery grants, \
             got {document}"
        );
    }
}

/// RULE 3: the grants are MERGED into the policy the bucket already carries — the owner's own
/// statements survive verbatim, and a second flow log is covered beside the first.
///
/// The steps differ in what the bucket carries when the next flow log is created (a policy of
/// the owner's own, then the grants of one flow log, then of two) and in where each flow log
/// delivers (the bucket itself, a Hive-partitioned subfolder, and the first destination
/// again, which adds no second entry).
#[tokio::test]
async fn the_delivery_grants_are_merged_into_the_policy_the_bucket_already_carries() {
    let emulator = Emulator::new();
    let bucket = "flow-logs-shared";
    emulator.given_bucket(REGION, bucket).await;
    let vpc_id = emulator.given_vpc(REGION, "10.3.0.0/16").await;
    let second_vpc_id = emulator.given_vpc(REGION, "10.4.0.0/16").await;

    // The statement a client of its own wrote on the bucket before any flow log: the
    // enforce-TLS deny that must survive a create.
    let owners = json!({
        "Sid": "DenyInsecureTransport",
        "Effect": "Deny",
        "Principal": "*",
        "Action": "s3:*",
        "Resource": [
            format!("arn:aws:s3:::{bucket}"),
            format!("arn:aws:s3:::{bucket}/*"),
        ],
        "Condition": { "Bool": { "aws:SecureTransport": "false" } },
    });
    emulator
        .given_bucket_policy(
            REGION,
            bucket,
            &json!({ "Version": "2012-10-17", "Statement": [owners.clone()] }),
        )
        .await;

    // (case, the monitored VPC, the subfolder, Hive prefixes, the locations the write grant
    // must then cover)
    let steps: Vec<(&str, &str, &str, bool, Vec<String>)> = vec![
        (
            "the first flow log, delivering to the bucket itself",
            &vpc_id,
            "",
            false,
            vec![delivered_path(bucket, "", false)],
        ),
        (
            "a second flow log, Hive-partitioned under a subfolder",
            &second_vpc_id,
            "vpc",
            true,
            vec![
                delivered_path(bucket, "", false),
                delivered_path(bucket, "vpc", true),
            ],
        ),
        (
            "a third flow log, delivering where the first one does",
            &second_vpc_id,
            "",
            false,
            vec![
                delivered_path(bucket, "", false),
                delivered_path(bucket, "vpc", true),
            ],
        ),
    ];

    for (case, monitored, subfolder, hive, delivered) in steps {
        let log_destination = if subfolder.is_empty() {
            format!("arn:aws:s3:::{bucket}")
        } else {
            format!("arn:aws:s3:::{bucket}/{subfolder}/")
        };
        created_flow_log(
            &emulator,
            case,
            REGION,
            &s3_params(monitored, &log_destination, Some(hive)),
        )
        .await;

        let document = emulator.policy_of(case, REGION, bucket).await;
        assert_grants_delivery(case, &document, REGION, bucket, &delivered);
        let kept: Vec<Value> = statements(&document)
            .into_iter()
            .filter(|statement| statement["Sid"] == json!("DenyInsecureTransport"))
            .collect();
        assert_eq!(
            kept,
            vec![owners.clone()],
            "case {case:?}: the owner's own statement must be left exactly as it was, \
             got {document}"
        );
        assert_eq!(
            statements(&document).len(),
            delivered.len() + 2,
            "case {case:?}: the policy must hold the owner's statement, one write grant per \
             delivered location and the ACL-check grant, got {document}"
        );
    }
}

/// RULE 4: a flow log that publishes anywhere else leaves the bucket's policy alone — the
/// grants belong to an S3 delivery, not to every create.
///
/// The rows differ in the destination type and in which member named the destination: a
/// CloudWatch Logs group by name, the same group by ARN, and a Firehose delivery stream.
#[tokio::test]
async fn a_flow_log_that_does_not_publish_to_amazon_s3_touches_no_bucket_policy() {
    const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/flow-logs-delivery";
    let bucket = "flow-logs-untouched";

    // (case, the members that name the destination)
    let cases: Vec<(&str, Vec<(String, String)>)> = vec![
        (
            "a CloudWatch Logs group named by LogGroupName",
            vec![
                p("LogGroupName", "/aws/vpc/flow-logs"),
                p("DeliverLogsPermissionArn", ROLE_ARN),
            ],
        ),
        (
            "a CloudWatch Logs group named by its ARN",
            vec![
                p(
                    "LogDestination",
                    "arn:aws:logs:us-east-1:000000000000:log-group:/aws/vpc/flow-logs",
                ),
                p("DeliverLogsPermissionArn", ROLE_ARN),
            ],
        ),
        (
            "a Firehose delivery stream",
            vec![
                p("LogDestinationType", "kinesis-data-firehose"),
                p(
                    "LogDestination",
                    "arn:aws:firehose:us-east-1:000000000000:deliverystream/flow-logs",
                ),
            ],
        ),
    ];

    for (case, destination) in cases {
        let emulator = Emulator::new();
        emulator.given_bucket(REGION, bucket).await;
        let vpc_id = emulator.given_vpc(REGION, "10.5.0.0/16").await;

        let mut params = vec![
            p("ResourceType", "VPC"),
            p("ResourceId.1", &vpc_id),
            p("TrafficType", "ALL"),
        ];
        params.extend(destination);
        created_flow_log(&emulator, case, REGION, &params).await;

        let (status, document) = emulator.bucket_policy(REGION, bucket).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a flow log publishing elsewhere must leave the bucket carrying \
             no policy, got {status} {document}"
        );
    }
}
