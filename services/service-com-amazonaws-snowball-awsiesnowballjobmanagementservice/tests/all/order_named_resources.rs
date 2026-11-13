// SPDX-License-Identifier: CC0-1.0

//! The AWS resources a Snow order NAMES are resolved against the services that hold them,
//! driven end to end over HTTP.
//!
//! The rule, asserted over a TABLE of resource kinds rather than over one request: an
//! operation that places or updates an order resolves every resource the order names — the
//! Amazon S3 buckets it transfers, the Lambda functions and the Amazon EC2 images it
//! carries, the IAM role it is performed under, the Key Management Service key it is
//! encrypted with and the Amazon SNS topics it notifies — against the service that holds
//! that resource, and refuses an order naming one that service answers it does not hold,
//! instead of minting an order against resources nobody holds. The refusal is the one the
//! model declares for that resource: `InvalidResourceException` ("The specified resource
//! can't be found …") for a bucket, role, function or topic, `KMSRequestFailedException`
//! for a key and `Ec2RequestFailedException` for an image.
//!
//! Every row is driven against each of the four operations that take those members —
//! `CreateJob`, `CreateCluster`, `UpdateJob`, `UpdateCluster` — and each refusal is checked
//! to have persisted nothing. The resources a row means to be REAL are created through the
//! owning service's OWN API on the same engine, so what the order is resolved against is
//! the record that service's create wrote.
//!
//! The last suite is the other half of the rule: a resource nobody answered about refuses
//! nothing, which is what a deployment that mounts Snowball without the service owning a
//! resource looks like.
//!
//! The Snowball harness (`router_with_engine`, `call_in_region`, `ok`, `create_address`,
//! `deliverable_address`, `complete_order`, `string`) and the describe helpers
//! (`job_metadata`, `cluster_metadata`) are the ones this crate's other suites drive the
//! service with, so every Snowball call here goes through the same front door a client uses.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::cluster_updates::{cluster_metadata, job_metadata};
use super::job_lifecycle::{
    call_in_region, complete_order, create_address, deliverable_address, ok, string,
};

/// The Region every request in this suite is signed for, Snowball's and the owning
/// services' alike: a resource is resolved where the order that names it is placed.
pub(crate) const REGION: &str = "us-west-2";

/// The account every resource this emulator holds belongs to.
fn account() -> &'static str {
    ::emulator_server::emulator_account_id()
}

/// Snowball with every service that holds a resource an order names mounted beside it on
/// the SAME engine, exactly as the assembled router mounts them.
fn router_with_owning_services() -> MultiServiceRouter {
    let (mut router, engine) = super::job_lifecycle::router_with_engine();
    router.register_service(
        ::service_com_amazonaws_s3_amazons3::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(::service_com_amazonaws_kms_trentservice::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(::service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        ::service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        ::service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(engine),
    );
    router
}

/// One request to a service other than Snowball, signed for that service's own endpoint in
/// [`REGION`]: the router picks a service by the SigV4 credential scope's service field.
/// Nothing is verified — the harness turns request authentication off.
pub(crate) async fn owning_service(
    router: &MultiServiceRouter,
    service: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, String) {
    let host = format!("{service}.{REGION}.amazonaws.com");
    let mut request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDSNOWBALLTEST/20260101/{REGION}/{service}\
                 /aws4_request, SignedHeaders=host, Signature=00"
            ),
        );
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = router
        .dispatch(
            request
                .body(bytes::Bytes::from(body.to_string()))
                .expect("a well-formed request"),
        )
        .await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).into_owned();
    (status, body)
}

/// The text of the first `<tag>` element of an XML body, which is how the query-protocol
/// services here report a minted ARN or id. A scan rather than a document model, as this
/// crate's sibling suites do: every tag read here is a leaf whose content is text.
pub(crate) fn element(body: &str, tag: &str, case: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = body
        .find(&open)
        .unwrap_or_else(|| panic!("case {case:?}: expected a <{tag}> in {body}"));
    let rest = &body[start + open.len()..];
    let end = rest
        .find(&close)
        .unwrap_or_else(|| panic!("case {case:?}: <{tag}> is never closed in {body}"));
    rest[..end].to_string()
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC 3986's
/// unreserved characters literal.
pub(crate) fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// `PUT /<bucket>` on the mounted Amazon S3 service: the bucket really is in the account's
/// S3 inventory afterwards, so an order naming it names a bucket S3 itself reports.
///
/// The create carries the location constraint of the Region it is addressed to, which S3
/// requires of a create sent to a Region-specific endpoint.
async fn create_bucket(router: &MultiServiceRouter, bucket: &str) -> String {
    let case = format!("create the bucket {bucket}");
    let configuration = format!(
        "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
         <LocationConstraint>{REGION}</LocationConstraint></CreateBucketConfiguration>"
    );
    let (status, body) = owning_service(
        router,
        "s3",
        "PUT",
        &format!("/{bucket}"),
        &[("content-type", "application/xml")],
        &configuration,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateBucket to answer 200, got {status} with {body}"
    );
    format!("arn:aws:s3:::{bucket}")
}

/// The service principal a Snow order's role is assumed under, which is what its trust
/// policy has to name.
const SNOWBALL_PRINCIPAL: &str = "importexport.amazonaws.com";

/// The service principal a Lambda function's execution role is assumed under. A role is
/// only assumable by the service its trust policy names, so the function below needs its
/// OWN role rather than the order's.
const LAMBDA_PRINCIPAL: &str = "lambda.amazonaws.com";

/// `CreateRole` on the mounted IAM service, answering the role ARN IAM minted.
///
/// `service_principal` is the service the role is created FOR: its trust policy names that
/// principal and no other, exactly as a role an account creates for one service does.
async fn create_role(
    router: &MultiServiceRouter,
    role_name: &str,
    service_principal: &str,
) -> String {
    let case = format!("create the role {role_name}");
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": service_principal },
            "Action": "sts:AssumeRole"
        }]
    })
    .to_string();
    let body = format!(
        "Action=CreateRole&Version=2010-05-08&RoleName={}&AssumeRolePolicyDocument={}",
        form_encode(role_name),
        form_encode(&trust)
    );
    let (status, answered) = owning_service(
        router,
        "iam",
        "POST",
        "/",
        &[("content-type", "application/x-www-form-urlencoded")],
        &body,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateRole to answer 200, got {status} with {answered}"
    );
    element(&answered, "Arn", &case)
}

/// `CreateKey` on the mounted Key Management Service, answering the key ARN KMS minted.
async fn create_kms_key(router: &MultiServiceRouter) -> String {
    let case = "create a customer managed key";
    let (status, answered) = owning_service(
        router,
        "kms",
        "POST",
        "/",
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("x-amz-target", "TrentService.CreateKey"),
        ],
        "{}",
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateKey to answer 200, got {status} with {answered}"
    );
    let parsed: Value = serde_json::from_str(&answered).expect("CreateKey answers a JSON body");
    parsed["KeyMetadata"]["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a key ARN, got {parsed}"))
        .to_string()
}

/// `CreateTopic` on the mounted Amazon SNS service, answering the topic ARN SNS minted.
pub(crate) async fn create_topic(router: &MultiServiceRouter, name: &str) -> String {
    let case = format!("create the topic {name}");
    let body = format!(
        "Action=CreateTopic&Version=2010-03-31&Name={}",
        form_encode(name)
    );
    let (status, answered) = owning_service(
        router,
        "sns",
        "POST",
        "/",
        &[("content-type", "application/x-www-form-urlencoded")],
        &body,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateTopic to answer 200, got {status} with {answered}"
    );
    element(&answered, "TopicArn", &case)
}

/// The key the deployment package of the function below is uploaded under, and read back
/// from: Lambda reads the package out of the Amazon S3 object the `Code` names, so the
/// object has to be there before the function is created.
const PACKAGE_KEY: &str = "function.zip";

/// `PUT /<bucket>/<key>` on the mounted Amazon S3 service: the deployment package the
/// function below is created from, in the bucket this suite created.
///
/// AWS Lambda UNZIPS the package it stores, so the object has to be a readable .zip archive;
/// nothing this suite calls reads its CONTENTS, so the object is the 22-byte
/// end-of-central-directory record an archive with no entries is ([`EMPTY_ARCHIVE`]).
async fn put_package(router: &MultiServiceRouter, bucket: &str, case: &str) {
    let (status, body) = owning_service(
        router,
        "s3",
        "PUT",
        &format!("/{bucket}/{PACKAGE_KEY}"),
        &[("content-type", "application/zip")],
        EMPTY_ARCHIVE,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected PutObject to answer 200, got {status} with {body}"
    );
}

/// An EMPTY .zip archive: the end-of-central-directory record (`PK\x05\x06` and its 18
/// remaining bytes), which is a whole archive holding no entry at all.
const EMPTY_ARCHIVE: &str = "PK\u{5}\u{6}\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0";

/// `CreateFunction` on the mounted Lambda service, answering the function ARN it minted.
///
/// The deployment package is the S3 object form, uploaded to the bucket this suite created
/// before the create is made ([`put_package`]): Lambda reads the package out of the object
/// the `Code` names, and refuses a create whose bucket holds nothing under that key.
async fn create_function(
    router: &MultiServiceRouter,
    name: &str,
    role_arn: &str,
    bucket: &str,
) -> String {
    let case = format!("create the function {name}");
    put_package(router, bucket, &case).await;
    let body = json!({
        "FunctionName": name,
        "Role": role_arn,
        "Runtime": "python3.12",
        "Handler": "index.handler",
        "Code": { "S3Bucket": bucket, "S3Key": PACKAGE_KEY },
    })
    .to_string();
    let (status, answered) = owning_service(
        router,
        "lambda",
        "POST",
        "/2015-03-31/functions",
        &[("content-type", "application/json")],
        &body,
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected CreateFunction to answer 201, got {status} with {answered}"
    );
    let parsed: Value =
        serde_json::from_str(&answered).expect("CreateFunction answers a JSON body");
    parsed["FunctionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a function ARN, got {parsed}"))
        .to_string()
}

/// The id of an image the mounted Amazon EC2 service reports to this account, read through
/// `DescribeImages` — so the AMI an order names is one EC2 itself reports rather than a
/// value this suite invented.
async fn visible_ami(router: &MultiServiceRouter) -> String {
    let case = "read an image EC2 reports";
    let (status, answered) = owning_service(
        router,
        "ec2",
        "POST",
        "/",
        &[("content-type", "application/x-www-form-urlencoded")],
        "Action=DescribeImages&Version=2016-11-15",
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeImages to answer 200, got {status} with {answered}"
    );
    element(&answered, "imageId", case)
}

/// Every resource this suite holds, each created through the API of the service that owns
/// it, plus the Snowball records an update is driven against.
struct Held {
    address_id: String,
    bucket_arn: String,
    other_bucket_arn: String,
    role_arn: String,
    key_arn: String,
    topic_arn: String,
    pickup_topic_arn: String,
    function_arn: String,
    ami_id: String,
    job_id: String,
    cluster_id: String,
}

impl Held {
    /// Create one of everything, and the order records `UpdateJob` and `UpdateCluster` are
    /// driven against.
    async fn create(router: &MultiServiceRouter) -> Self {
        let address_id = create_address(
            router,
            deliverable_address("Resource Receiver", "1 Resolution Way", json!({})),
            "the address every order in this suite ships to",
        )
        .await;
        let bucket_arn = create_bucket(router, "snowball-order-source").await;
        let other_bucket_arn = create_bucket(router, "snowball-order-second-source").await;
        let role_arn = create_role(router, "snowball-order-role", SNOWBALL_PRINCIPAL).await;
        let key_arn = create_kms_key(router).await;
        let topic_arn = create_topic(router, "snowball-order-events").await;
        let pickup_topic_arn = create_topic(router, "snowball-order-pickup").await;
        // The function RUNS AS its own execution role, which Lambda assumes, so that role
        // trusts the Lambda service principal rather than the one the order's role names.
        let function_role_arn =
            create_role(router, "snowball-order-function-role", LAMBDA_PRINCIPAL).await;
        let function_arn = create_function(
            router,
            "snowball-order-function",
            &function_role_arn,
            "snowball-order-source",
        )
        .await;
        let ami_id = visible_ami(router).await;

        let mut held = Self {
            address_id,
            bucket_arn,
            other_bucket_arn,
            role_arn,
            key_arn,
            topic_arn,
            pickup_topic_arn,
            function_arn,
            ami_id,
            job_id: String::new(),
            cluster_id: String::new(),
        };
        let created = ok(
            router,
            "CreateJob",
            held.job_order(&json!({})),
            "the job every update row is driven against",
        )
        .await;
        held.job_id = string(&created, "JobId", "the job every update row is driven against");
        let created = ok(
            router,
            "CreateCluster",
            held.cluster_order(&json!({})),
            "the cluster every update row is driven against",
        )
        .await;
        held.cluster_id = string(
            &created,
            "ClusterId",
            "the cluster every update row is driven against",
        );
        held
    }

    /// Every resource member of an order, each naming a resource its owning service holds.
    fn resources(&self) -> Value {
        json!({
            "S3Resources": [{ "BucketArn": self.bucket_arn }],
            "LambdaResources": [{ "LambdaArn": self.function_arn }],
            "Ec2AmiResources": [{ "AmiId": self.ami_id }],
        })
    }

    /// The notification members of an order, each naming a topic Amazon SNS holds.
    fn notification(&self) -> Value {
        json!({
            "SnsTopicARN": self.topic_arn,
            "DevicePickupSnsTopicARN": self.pickup_topic_arn,
            "NotifyAll": true,
        })
    }

    /// A complete `CreateJob` body whose every resource member names a held resource, with
    /// `fragment` merged over it so a row states only the member it varies.
    fn job_order(&self, fragment: &Value) -> Value {
        let mut body = complete_order(&json!({
            "AddressId": self.address_id,
            "JobType": "IMPORT",
            "SnowballType": "EDGE",
            "ShippingOption": "SECOND_DAY",
            "RoleARN": self.role_arn,
            "KmsKeyARN": self.key_arn,
            "Resources": self.resources(),
            "Notification": self.notification(),
        }));
        merge(&mut body, fragment);
        body
    }

    /// The same for `CreateCluster`, in the one job type and device type a cluster supports.
    fn cluster_order(&self, fragment: &Value) -> Value {
        let mut body = json!({
            "AddressId": self.address_id,
            "JobType": "LOCAL_USE",
            "SnowballType": "EDGE",
            "ShippingOption": "SECOND_DAY",
            "RoleARN": self.role_arn,
            "KmsKeyARN": self.key_arn,
            "Resources": self.resources(),
            "Notification": self.notification(),
        });
        merge(&mut body, fragment);
        body
    }

    /// An `UpdateJob` body naming the held job, carrying `fragment`.
    fn job_update(&self, fragment: &Value) -> Value {
        let mut body = json!({ "JobId": self.job_id });
        merge(&mut body, fragment);
        body
    }

    /// An `UpdateCluster` body naming the held cluster, carrying `fragment`.
    fn cluster_update(&self, fragment: &Value) -> Value {
        let mut body = json!({ "ClusterId": self.cluster_id });
        merge(&mut body, fragment);
        body
    }

    /// The four operations a row is driven against, each with the body that carries
    /// `fragment`. A fragment naming `KmsKeyARN` is driven against the two operations that
    /// model it — `UpdateJobRequest` and `UpdateClusterRequest` carry no such member
    /// (`smithy_query(query="operation", operation="UpdateJob")`).
    fn calls(&self, fragment: &Value) -> Vec<(&'static str, Value)> {
        let mut calls = vec![
            ("CreateJob", self.job_order(fragment)),
            ("CreateCluster", self.cluster_order(fragment)),
        ];
        if fragment.get("KmsKeyARN").is_none() {
            calls.push(("UpdateJob", self.job_update(fragment)));
            calls.push(("UpdateCluster", self.cluster_update(fragment)));
        }
        calls
    }
}

/// Merge `fragment` over `body`: a member the fragment names replaces what the base order
/// carried, and one it leaves out stays as it was.
fn merge(body: &mut Value, fragment: &Value) {
    for (member, value) in fragment
        .as_object()
        .expect("the fragment is a JSON object")
        .iter()
    {
        body[member] = value.clone();
    }
}

/// One refusal row: the case, the order members that name a resource the owning service
/// does not hold, the modelled error the refusal must carry and the `ResourceType` it must
/// report (`None` where the refusal's shape models no such member).
struct Row {
    case: String,
    fragment: Value,
    error_code: &'static str,
    resource_type: Option<&'static str>,
    /// The value the refusal's message must name, for the refusals that name one.
    named: Option<String>,
}

/// Every refusal row: one per kind of resource an order names, plus the rows that place an
/// unheld resource beside held ones (a second bucket entry, a second topic member) so the
/// rule is asserted over the whole member rather than over a single-entry list.
fn refusal_rows(held: &Held) -> Vec<Row> {
    let absent_bucket = "arn:aws:s3:::snowball-order-absent";
    let absent_role = format!("arn:aws:iam::{}:role/snowball-order-never-created", account());
    let absent_key = format!(
        "arn:aws:kms:{REGION}:{}:key/00000000-0000-0000-0000-000000000000",
        account()
    );
    let absent_function = format!(
        "arn:aws:lambda:{REGION}:{}:function:snowball-order-never-created",
        account()
    );
    let absent_topic = format!(
        "arn:aws:sns:{REGION}:{}:snowball-order-never-created",
        account()
    );
    // An id in the modelled `AmiId` form (`^(ami-[0-9a-f]{8})|(ami-[0-9a-f]{17})$`) that
    // names no image this account can see.
    let absent_ami = "ami-00000000000000000";

    vec![
        Row {
            case: "a bucket the account does not hold".to_string(),
            fragment: json!({ "Resources": { "S3Resources": [{ "BucketArn": absent_bucket }] } }),
            error_code: "InvalidResourceException",
            resource_type: Some("s3Bucket"),
            named: Some(absent_bucket.to_string()),
        },
        Row {
            case: "an unheld bucket beside a held one, in a later list entry".to_string(),
            fragment: json!({
                "Resources": {
                    "S3Resources": [
                        { "BucketArn": held.bucket_arn },
                        { "BucketArn": absent_bucket },
                    ]
                }
            }),
            error_code: "InvalidResourceException",
            resource_type: Some("s3Bucket"),
            named: Some(absent_bucket.to_string()),
        },
        Row {
            case: "a bucket named by an object ARN inside it".to_string(),
            fragment: json!({
                "Resources": {
                    "S3Resources": [{ "BucketArn": format!("{absent_bucket}/prefix/key") }]
                }
            }),
            error_code: "InvalidResourceException",
            resource_type: Some("s3Bucket"),
            named: Some(format!("{absent_bucket}/prefix/key")),
        },
        Row {
            case: "a role nobody created".to_string(),
            fragment: json!({ "RoleARN": absent_role }),
            error_code: "InvalidResourceException",
            resource_type: Some("role"),
            named: Some(absent_role.clone()),
        },
        Row {
            case: "a role under a path that nobody created".to_string(),
            fragment: json!({
                "RoleARN": format!("arn:aws:iam::{}:role/service-role/snowball-absent", account())
            }),
            error_code: "InvalidResourceException",
            resource_type: Some("role"),
            named: Some(format!(
                "arn:aws:iam::{}:role/service-role/snowball-absent",
                account()
            )),
        },
        Row {
            case: "a Lambda function nobody created".to_string(),
            fragment: json!({
                "Resources": {
                    "S3Resources": [{ "BucketArn": held.bucket_arn }],
                    "LambdaResources": [{ "LambdaArn": absent_function }],
                }
            }),
            error_code: "InvalidResourceException",
            resource_type: Some("lambdaFunction"),
            named: Some(absent_function.clone()),
        },
        Row {
            case: "a topic nobody created".to_string(),
            fragment: json!({ "Notification": { "SnsTopicARN": absent_topic } }),
            error_code: "InvalidResourceException",
            resource_type: Some("snsTopic"),
            named: Some(absent_topic.clone()),
        },
        Row {
            case: "a device pickup topic nobody created, beside a held topic".to_string(),
            fragment: json!({
                "Notification": {
                    "SnsTopicARN": held.topic_arn,
                    "DevicePickupSnsTopicARN": absent_topic,
                }
            }),
            error_code: "InvalidResourceException",
            resource_type: Some("snsTopic"),
            named: Some(absent_topic.clone()),
        },
        Row {
            case: "an AMI the account can see no image for".to_string(),
            fragment: json!({
                "Resources": {
                    "S3Resources": [{ "BucketArn": held.bucket_arn }],
                    "Ec2AmiResources": [{ "AmiId": absent_ami }],
                }
            }),
            error_code: "Ec2RequestFailedException",
            resource_type: None,
            named: None,
        },
        Row {
            case: "an unheld AMI beside a held image".to_string(),
            fragment: json!({
                "Resources": {
                    "S3Resources": [{ "BucketArn": held.bucket_arn }],
                    "Ec2AmiResources": [{ "AmiId": held.ami_id }, { "AmiId": absent_ami }],
                }
            }),
            error_code: "Ec2RequestFailedException",
            resource_type: None,
            named: None,
        },
        Row {
            case: "a key that resolves to no key".to_string(),
            fragment: json!({ "KmsKeyARN": absent_key }),
            error_code: "KMSRequestFailedException",
            resource_type: None,
            named: None,
        },
    ]
}

/// A Snowball call that must be refused with HTTP 400, answering the parsed body.
async fn refused_body(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    case: &str,
) -> Value {
    let (status, parsed) = call_in_region(router, REGION, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {operation} to answer 400, got {status} with {parsed}"
    );
    parsed
}

/// The `Message` an error body carries, whichever casing the protocol put it under.
fn message(body: &Value) -> String {
    body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// The number of jobs `ListJobs` reports, so a refused create can be shown to have stored
/// nothing.
async fn job_count(router: &MultiServiceRouter, case: &str) -> usize {
    let listed = ok(router, "ListJobs", json!({}), case).await;
    listed["JobListEntries"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

#[tokio::test]
async fn an_order_naming_a_resource_its_owning_service_does_not_hold_is_refused() {
    let router = router_with_owning_services();
    let held = Held::create(&router).await;

    // What the held order looks like before any refused call: a refusal must leave both
    // records exactly as they are.
    let job_before = job_metadata(&router, &held.job_id, "the held job before any refusal").await;
    let cluster_before = cluster_metadata(
        &router,
        &held.cluster_id,
        "the held cluster before any refusal",
    )
    .await;
    let jobs_before = job_count(&router, "the jobs held before any refusal").await;

    for row in refusal_rows(&held) {
        for (operation, body) in held.calls(&row.fragment) {
            let case = format!("{operation}: {}", row.case);
            let refusal = refused_body(&router, operation, body, &case).await;
            let wire_type = refusal["__type"].as_str().unwrap_or_default();
            assert!(
                wire_type.contains(row.error_code),
                "case {case:?}: expected a {} , got {refusal}",
                row.error_code
            );
            let reported = message(&refusal);
            assert!(
                reported.ends_with(
                    "Check the information you provided in your last request, and try again."
                ) || row.error_code != "InvalidResourceException",
                "case {case:?}: expected the modelled wording, got {reported:?}"
            );
            if let Some(named) = &row.named {
                assert!(
                    reported.contains(named),
                    "case {case:?}: expected the refusal to name {named:?}, got {reported:?}"
                );
            }
            let reported_type = refusal["ResourceType"].as_str();
            assert_eq!(
                reported_type, row.resource_type,
                "case {case:?}: expected ResourceType {:?}, got {reported_type:?}",
                row.resource_type
            );
        }
    }

    // Nothing a refused call named reached the store: no job was minted and neither held
    // record changed.
    let jobs_after = job_count(&router, "the jobs held after every refusal").await;
    assert_eq!(
        jobs_after, jobs_before,
        "expected the refused creates to have stored no job, got {jobs_after} jobs where \
         {jobs_before} were held"
    );
    let job_after = job_metadata(&router, &held.job_id, "the held job after every refusal").await;
    assert_eq!(
        job_after, job_before,
        "expected the refused updates to have left the job as it was, got {job_after}"
    );
    let cluster_after = cluster_metadata(
        &router,
        &held.cluster_id,
        "the held cluster after every refusal",
    )
    .await;
    assert_eq!(
        cluster_after, cluster_before,
        "expected the refused updates to have left the cluster as it was, got {cluster_after}"
    );
}

#[tokio::test]
async fn an_order_naming_resources_every_owning_service_holds_is_accepted_and_read_back() {
    let router = router_with_owning_services();
    let held = Held::create(&router).await;

    // A create naming one of every kind of resource, each one its owning service holds, is
    // accepted — and the order is read back pointing at exactly those resources.
    let case = "an order naming one held resource of every kind";
    let created = ok(&router, "CreateJob", held.job_order(&json!({})), case).await;
    let job_id = string(&created, "JobId", case);
    let metadata = job_metadata(&router, &job_id, case).await;
    for (member, expected) in [
        ("RoleARN", json!(held.role_arn)),
        ("KmsKeyARN", json!(held.key_arn)),
        ("Resources", held.resources()),
        ("Notification", held.notification()),
    ] {
        assert_eq!(
            metadata[member], expected,
            "case {case:?}: expected {member} to be reported as {expected}, got {}",
            metadata[member]
        );
    }

    let cluster_case = "a cluster order naming one held resource of every kind";
    let created = ok(
        &router,
        "CreateCluster",
        held.cluster_order(&json!({})),
        cluster_case,
    )
    .await;
    let cluster_id = string(&created, "ClusterId", cluster_case);
    let metadata = cluster_metadata(&router, &cluster_id, cluster_case).await;
    for (member, expected) in [
        ("RoleARN", json!(held.role_arn)),
        ("KmsKeyARN", json!(held.key_arn)),
        ("Resources", held.resources()),
    ] {
        assert_eq!(
            metadata[member], expected,
            "case {cluster_case:?}: expected {member} to be reported as {expected}, got {}",
            metadata[member]
        );
    }

    // Every row an update is accepted with, each naming resources the owning services hold
    // — including the values a service answers nothing about, which refuse nothing: a role
    // of ANOTHER account (not this emulator's IAM to be asked about) and a bucket ARN of
    // another vendor's namespace (which names no Amazon S3 bucket at all).
    let update_rows: Vec<(&str, Value)> = vec![
        (
            "a second held bucket",
            json!({ "Resources": { "S3Resources": [{ "BucketArn": held.other_bucket_arn }] } }),
        ),
        (
            "two held buckets at once",
            json!({
                "Resources": {
                    "S3Resources": [
                        { "BucketArn": held.bucket_arn },
                        { "BucketArn": held.other_bucket_arn },
                    ]
                }
            }),
        ),
        (
            "the held topics",
            json!({ "Notification": held.notification() }),
        ),
        (
            "a role of another account, which this emulator's IAM cannot answer about",
            json!({ "RoleARN": "arn:aws:iam::123456789012:role/another-accounts-role" }),
        ),
        (
            "a bucket ARN of another vendor's namespace, which names no Amazon S3 bucket",
            json!({
                "Resources": {
                    "S3Resources": [{ "BucketArn": "arn:aws:s3-outposts:::outpost-bucket" }]
                }
            }),
        ),
    ];
    for (case, fragment) in update_rows {
        for (operation, body) in held.calls(&fragment) {
            let case = format!("{operation}: {case}");
            let answered = ok(&router, operation, body, &case).await;
            if operation == "CreateJob" {
                let job_id = string(&answered, "JobId", &case);
                let metadata = job_metadata(&router, &job_id, &case).await;
                for (member, expected) in fragment
                    .as_object()
                    .expect("the fragment is a JSON object")
                    .iter()
                {
                    assert_eq!(
                        &metadata[member], expected,
                        "case {case:?}: expected {member} to be reported as {expected}, got {}",
                        metadata[member]
                    );
                }
            }
        }
    }

    // The update reached the records it named, read back through the describes.
    let case = "the job after its last accepted update";
    let metadata = job_metadata(&router, &held.job_id, case).await;
    assert_eq!(
        metadata["Resources"],
        json!({ "S3Resources": [{ "BucketArn": "arn:aws:s3-outposts:::outpost-bucket" }] }),
        "case {case:?}: expected the last accepted update's resources, got {}",
        metadata["Resources"]
    );
}

#[tokio::test]
async fn an_order_is_not_refused_where_no_owning_service_answered() {
    // Snowball alone, which is what a deployment that mounts it without the services
    // holding the resources an order names looks like: nothing answers about those
    // resources, so no order is refused on them. The SAME orders the suite above is
    // refused for are accepted here.
    let router = super::job_lifecycle::router();
    let address_id = create_address(
        &router,
        deliverable_address("Unanswered Receiver", "2 Resolution Way", json!({})),
        "the address every order in this suite ships to",
    )
    .await;
    let held = Held {
        address_id,
        bucket_arn: "arn:aws:s3:::snowball-order-source".to_string(),
        other_bucket_arn: "arn:aws:s3:::snowball-order-second-source".to_string(),
        role_arn: format!("arn:aws:iam::{}:role/snowball-order-role", account()),
        key_arn: format!(
            "arn:aws:kms:{REGION}:{}:key/11111111-1111-1111-1111-111111111111",
            account()
        ),
        topic_arn: format!("arn:aws:sns:{REGION}:{}:snowball-order-events", account()),
        pickup_topic_arn: format!("arn:aws:sns:{REGION}:{}:snowball-order-pickup", account()),
        function_arn: format!(
            "arn:aws:lambda:{REGION}:{}:function:snowball-order-function",
            account()
        ),
        ami_id: "ami-11111111111111111".to_string(),
        job_id: String::new(),
        cluster_id: String::new(),
    };

    for row in refusal_rows(&held) {
        let case = format!("CreateJob with no owning service mounted: {}", row.case);
        let created = ok(&router, "CreateJob", held.job_order(&row.fragment), &case).await;
        let job_id = string(&created, "JobId", &case);
        assert!(
            job_id.starts_with("JID"),
            "case {case:?}: expected a minted job id, got {job_id:?}"
        );
    }
}
