// SPDX-License-Identifier: CC0-1.0

//! The CodeDeploy application / deployment-group / deployment lifecycle, driven
//! over the wire.
//!
//! Every case here goes through the same front door a client uses: a signed-shape
//! `POST /` with `X-Amz-Target: CodeDeploy_20141006.<Operation>` dispatched by the
//! multi-service router, so the boundary validator, this service's refusal
//! wording and the awsJson1_1 envelope are all exercised, not bypassed.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// One emulator with one CodeDeploy service mounted on one shared engine.
///
/// `pub(crate)` because the other suites in this test target drive the service
/// through the same front door; there is one harness, not one per suite. `engine`
/// is the very store the handlers write through, for a case whose resource no API
/// can create.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    pub engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        Self::mounted(false, false, false, false)
    }

    /// One emulator with IAM mounted beside CodeDeploy on the SAME engine.
    ///
    /// The role a deployment group runs as belongs to IAM, so a suite about that
    /// role creates it through IAM's own front door rather than reaching into its
    /// store.
    pub(crate) fn with_iam() -> Self {
        Self::mounted(false, false, true, false)
    }

    /// One emulator with CloudWatch mounted beside CodeDeploy on the SAME engine.
    ///
    /// The alarms a deployment is gated on belong to CloudWatch, so a suite about
    /// them drives both services through this one front door — the way the router
    /// mounts them — rather than reaching into either store.
    pub(crate) fn with_cloudwatch() -> Self {
        Self::mounted(true, false, false, false)
    }

    /// One emulator with AWS Lambda mounted beside CodeDeploy on the SAME engine.
    ///
    /// The alias an Lambda deployment shifts traffic on belongs to Lambda, so a
    /// suite about that shift creates the function and reads the alias through
    /// Lambda's own front door rather than reaching into its store.
    pub(crate) fn with_lambda() -> Self {
        Self::mounted(false, true, false, false)
    }

    /// One emulator with AWS Lambda, CloudWatch *and* IAM mounted beside CodeDeploy
    /// on the SAME engine, for a suite about what a deployment's SERVICE ROLE lets
    /// it do: the role and its policies belong to IAM, the alias the deployment
    /// moves to Lambda, and the alarms its group polls to CloudWatch.
    pub(crate) fn with_lambda_cloudwatch_and_iam() -> Self {
        Self::mounted(true, true, true, false)
    }

    /// One emulator with CloudWatch *and* AWS Lambda mounted beside CodeDeploy on
    /// the SAME engine, for a suite about a shift an ALARM interrupts: the alarm
    /// belongs to one of them and the alias to the other.
    pub(crate) fn with_lambda_and_cloudwatch() -> Self {
        Self::mounted(true, true, false, false)
    }

    /// One emulator with Amazon SNS and Amazon SQS mounted beside CodeDeploy on
    /// the SAME engine.
    ///
    /// The topic a deployment group's trigger publishes to belongs to SNS and the
    /// queue subscribed to that topic to SQS, so a suite about those notifications
    /// creates both through their own front doors and reads a notification where a
    /// client reads it: off the queue.
    pub(crate) fn with_sns_and_sqs() -> Self {
        Self::mounted(false, false, false, true)
    }

    /// One emulator with AWS Lambda *and* Amazon S3 mounted beside CodeDeploy on
    /// the SAME engine, for a suite about a revision that keeps its AppSpec in an
    /// S3 object: the object belongs to S3 and the alias the deployment moves to
    /// Lambda, so both are driven through their own front doors.
    pub(crate) fn with_lambda_and_s3() -> Self {
        let mut emulator = Self::mounted(false, true, false, false);
        emulator
            .router
            .register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
                Arc::clone(&emulator.engine),
            ));
        emulator
    }

    /// The one constructor behind all of them: CodeDeploy, and each of CloudWatch,
    /// AWS Lambda, IAM and the Amazon SNS/Amazon SQS pair the caller asks for, on
    /// one shared engine.
    fn mounted(cloudwatch: bool, lambda: bool, iam: bool, notifications: bool) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing; routing is by `X-Amz-Target`.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_codedeploy_codedeploy_20141006::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        if cloudwatch {
            router.register_service(
                service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
                    Arc::clone(&engine),
                ),
            );
        }
        if lambda {
            router.register_service(
                service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(
                    &engine,
                )),
            );
        }
        if iam {
            router.register_service(
                service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                    Arc::clone(&engine),
                ),
            );
        }
        if notifications {
            router.register_service(
                service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
                    Arc::clone(&engine),
                ),
            );
            router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
                Arc::clone(&engine),
            ));
        }
        Self { router, engine }
    }

    /// Call `operation` with `body`, answering its HTTP status and parsed body.
    pub(crate) async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        self.call_in_region(None, operation, body).await
    }

    /// Call `operation` with `body` as a request whose SigV4 credential scope
    /// names `region`, answering its HTTP status and parsed body.
    ///
    /// The scope is where this service takes the region of an ARN it reports
    /// from, so a case about an ARN's region drives the call through here; the
    /// suite verifies no signature, so the signature field is not one.
    pub(crate) async fn call_in_region(
        &self,
        region: Option<&str>,
        operation: &str,
        body: Value,
    ) -> (u16, Value) {
        let mut builder = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("x-amz-target", format!("CodeDeploy_20141006.{operation}"))
            .header("content-type", "application/x-amz-json-1.1");
        if let Some(region) = region {
            builder = builder.header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/codedeploy/\
                     aws4_request, SignedHeaders=host, Signature=0000"
                ),
            );
        }
        let request = builder
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Call `operation` and require success, answering the response body.
    pub(crate) async fn ok(&self, operation: &str, body: Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(status, 200, "{operation} must succeed, got {parsed:?}");
        parsed
    }

    /// Call one CLOUDWATCH `operation` through the same front door and require
    /// success, answering the response body.
    ///
    /// CloudWatch's model declares `awsJson1_0`
    /// (`smithy_query(query="service_info")`), so the envelope and the target
    /// prefix are that service's own, not CodeDeploy's. Only meaningful on an
    /// [`Emulator::with_cloudwatch`].
    pub(crate) async fn cloudwatch_ok(&self, operation: &str, body: Value) -> Value {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "monitoring.us-east-1.amazonaws.com")
            .header(
                "x-amz-target",
                format!("GraniteServiceVersion20100801.{operation}"),
            )
            .header("content-type", "application/x-amz-json-1.0")
            // The SigV4 credential scope is what routes the request to the
            // `monitoring` signing name; the suite verifies no signature.
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/monitoring/\
                 aws4_request, SignedHeaders=host, Signature=0000",
            )
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let answered = String::from_utf8_lossy(response.body()).to_string();
        assert_eq!(
            status, 200,
            "cloudwatch {operation} must succeed, got {status} with body {answered}"
        );
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    }

    /// One `awsQuery` request against the IAM emulator through the same front
    /// door, answering its HTTP status and XML body.
    ///
    /// IAM's model declares `awsQuery` (`smithy_query(query="service_info")`), so
    /// its parameters travel as a form body and its answer comes back as XML; the
    /// SigV4 credential scope is what routes it to the `iam` signing name, and the
    /// suite verifies no signature. Only meaningful on an [`Emulator::with_iam`].
    pub(crate) async fn iam_query(&self, action: &str, params: &[(&str, &str)]) -> (u16, String) {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in params {
            body.push('&');
            body.push_str(&form_encode(name));
            body.push('=');
            body.push_str(&form_encode(value));
        }        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "iam.amazonaws.com")
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/iam/\
                 aws4_request, SignedHeaders=host, Signature=0000",
            )
            .body(bytes::Bytes::from(body))
            .expect("the request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (status, String::from_utf8_lossy(response.body()).to_string())
    }

    /// One `awsQuery` request against the Amazon SNS emulator through the same
    /// front door, answering its HTTP status and XML body.
    ///
    /// SNS's model declares `awsQuery` (`smithy_query(query="service_info")`), so
    /// its parameters travel as a form body and its answer comes back as XML; the
    /// SigV4 credential scope is what routes it to the `sns` signing name, and the
    /// suite verifies no signature. Only meaningful on an
    /// [`Emulator::with_sns_and_sqs`].
    pub(crate) async fn sns_query(
        &self,
        region: &str,
        action: &str,
        params: &[(&str, &str)],
    ) -> (u16, String) {
        let mut body = format!("Action={action}&Version=2010-03-31");
        for (name, value) in params {
            body.push('&');
            body.push_str(&form_encode(name));
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("sns.{region}.amazonaws.com"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/sns/\
                     aws4_request, SignedHeaders=host, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(body))
            .expect("the request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (status, String::from_utf8_lossy(response.body()).to_string())
    }

    /// One `awsJson1_0` request against the Amazon SQS emulator through the same
    /// front door, answering its HTTP status and parsed body.
    ///
    /// SQS's model declares `awsJson1_0` with an `AmazonSQS.<Operation>` target
    /// (`smithy_query(query="service_info")`), and the SigV4 credential scope is
    /// what routes the request to the `sqs` signing name; the suite verifies no
    /// signature. Only meaningful on an [`Emulator::with_sns_and_sqs`].
    pub(crate) async fn sqs_json(
        &self,
        region: &str,
        operation: &str,
        body: Value,
    ) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("sqs.{region}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("AmazonSQS.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/sqs/\
                     aws4_request, SignedHeaders=host, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One `restJson1` request against the LAMBDA emulator through the same front
    /// door, answering its HTTP status and parsed body.
    ///
    /// Lambda's model declares `restJson1`, so the request is addressed by method
    /// and URI rather than by a target header, and the SigV4 credential scope is
    /// what routes it to the `lambda` signing name; the suite verifies no
    /// signature. Only meaningful on an [`Emulator::with_lambda`].
    pub(crate) async fn lambda(&self, method: &str, uri: &str, body: Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method(method)
            .uri(format!("https://lambda.us-east-1.amazonaws.com{uri}"))
            .header("host", "lambda.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/lambda/\
                 aws4_request, SignedHeaders=host, Signature=0000",
            )
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One Lambda request that has to answer `expected`, answering its body; see
    /// [`Emulator::lambda`].
    pub(crate) async fn lambda_ok(
        &self,
        case: &str,
        method: &str,
        uri: &str,
        body: Value,
        expected: u16,
    ) -> Value {
        let (status, parsed) = self.lambda(method, uri, body).await;
        assert_eq!(
            status, expected,
            "setup {case:?}: expected HTTP {expected} from {method} {uri}, got {status} with \
             {parsed:?}"
        );
        parsed
    }

    /// One request against the Amazon S3 emulator through the same front door,
    /// answering its HTTP status, its `ETag` response header and its body.
    ///
    /// S3's model declares `restXml` with path-style addressing, so the bucket and
    /// the key ride the URI, and the SigV4 credential scope is what routes the
    /// request to the `s3` signing name; the suite verifies no signature. Only
    /// meaningful on an [`Emulator::with_lambda_and_s3`].
    pub(crate) async fn s3(
        &self,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> (u16, Option<String>, Vec<u8>) {
        let host = "s3.us-east-1.amazonaws.com";
        let request = ::http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/s3/\
                 aws4_request, SignedHeaders=host, Signature=0000",
            )
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("the request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let e_tag = response
            .headers()
            .get("etag")
            .map(|value| String::from_utf8_lossy(value.as_bytes()).to_string());
        (status, e_tag, response.body().to_vec())
    }

    /// Create `bucket` through S3's own `CreateBucket`; see [`Emulator::s3`].
    pub(crate) async fn s3_bucket(&self, case: &str, bucket: &str) {
        let (status, _, body) = self.s3("PUT", &format!("/{bucket}"), b"").await;
        assert_eq!(
            status,
            200,
            "setup {case:?}: CreateBucket {bucket} must succeed, got {status} with {:?}",
            String::from_utf8_lossy(&body)
        );
    }

    /// Write `body` at `key` in `bucket` through S3's own `PutObject`, answering
    /// the entity tag S3 reported for it; see [`Emulator::s3`].
    pub(crate) async fn s3_put_object(
        &self,
        case: &str,
        bucket: &str,
        key: &str,
        body: &[u8],
    ) -> String {
        let (status, e_tag, answered) = self.s3("PUT", &format!("/{bucket}/{key}"), body).await;
        assert_eq!(
            status,
            200,
            "setup {case:?}: PutObject {bucket}/{key} must succeed, got {status} with {:?}",
            String::from_utf8_lossy(&answered)
        );
        e_tag.unwrap_or_else(|| panic!("setup {case:?}: PutObject answered no ETag"))
    }
}

/// The wire error code of a failure body: awsJson carries it in `__type`, either
/// bare or namespaced.
pub(crate) fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// Percent-encode one form value the way an AWS SDK encodes an `awsQuery` body, for
/// [`Emulator::iam_query`]: the crate that owns percent-encoding, over the set that
/// escapes everything outside the unreserved characters.
fn form_encode(value: &str) -> String {
    ::percent_encoding::utf8_percent_encode(value, ::percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// A trust policy that lets `principal`, and only `principal`, assume the role it is
/// attached to — the document the IAM fixtures of this target create their roles with.
pub(crate) fn trust_policy(principal: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": principal},
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// Create one IAM role named `name` with `trust` as its trust document, through IAM's
/// own `CreateRole`, and answer the ARN IAM persisted for it. Only meaningful on an
/// [`Emulator::with_iam`].
pub(crate) async fn given_role(emulator: &Emulator, name: &str, trust: &str) -> String {
    let (status, created) = emulator
        .iam_query(
            "CreateRole",
            &[("RoleName", name), ("AssumeRolePolicyDocument", trust)],
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateRole {name} must succeed, got {status} with {created}"
    );
    xml_field(&created, "Arn")
        .unwrap_or_else(|| panic!("CreateRole {name} answered no Arn: {created}"))
}

/// The text of the first `<name>` element of an `awsQuery` XML answer — IAM speaks
/// that protocol, so a fixture reading one value back out of its response reads it
/// here rather than searching the document twice.
pub(crate) fn xml_field(document: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = document.find(&open)? + open.len();
    let end = document[start..].find(&close)? + start;
    Some(document[start..end].to_string())
}

/// An `s3Location` revision, the form the AWS CLI's `--s3-location` sends.
pub(crate) fn s3_revision(bucket: &str, key: &str) -> Value {
    json!({"revisionType": "S3", "s3Location": {"bucket": bucket, "key": key, "bundleType": "zip"}})
}

/// Create an application and a deployment group on it, answering the group name.
pub(crate) async fn seeded_group(
    emulator: &Emulator,
    application: &str,
    platform: &str,
) -> String {
    emulator
        .ok(
            "CreateApplication",
            json!({"applicationName": application, "computePlatform": platform}),
        )
        .await;
    let group = format!("{application}-dg");
    emulator
        .ok(
            "CreateDeploymentGroup",
            json!({
                "applicationName": application,
                "deploymentGroupName": group,
                "serviceRoleArn": "arn:aws:iam::123456789012:role/CodeDeployRole",
            }),
        )
        .await;
    group
}

/// How long a suite waits for a transition the clock owes, and how often it
/// looks: the emulator owes a deployment its terminal status well inside this.
///
/// A deployment runs for the span this emulator gives a transition a caller has
/// to be able to read (`state::DEPLOYMENT_COMPLETION_MILLIS`), so the deadline
/// sits well above it — a suite waiting for one is not a suite measuring it.
pub(crate) const POLL_DEADLINE: Duration = Duration::from_secs(15);
/// See [`POLL_DEADLINE`].
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Poll `GetDeployment` until `deployment_id` holds a terminal status, answering
/// its `deploymentInfo`.
///
/// The ONE poll loop every suite of this target uses, so "what a deployment ends
/// as" is read the same way everywhere. The terminal statuses are the members of
/// the model's `DeploymentStatus` enum that no further transition leaves
/// (`smithy_query(query="shape",shape="DeploymentStatus")`: Created | Queued |
/// InProgress | Baking | Succeeded | Failed | Stopped | Ready).
pub(crate) async fn terminal_deployment(
    emulator: &Emulator,
    deployment_id: &str,
    case: &str,
) -> Value {
    terminal_deployment_within(emulator, deployment_id, case, POLL_DEADLINE).await
}

/// [`terminal_deployment`] with the deadline the caller allows.
///
/// A deployment under a time-based traffic routing configuration is held open for
/// the window its increments describe, which is longer than the span a deployment
/// that cuts over in one step takes, so a suite that creates one says how long it
/// is prepared to wait for its window to close.
pub(crate) async fn terminal_deployment_within(
    emulator: &Emulator,
    deployment_id: &str,
    case: &str,
    deadline: Duration,
) -> Value {
    let deadline_at = Instant::now() + deadline;
    loop {
        let info = emulator
            .ok("GetDeployment", json!({"deploymentId": deployment_id}))
            .await["deploymentInfo"]
            .clone();
        if matches!(
            info["status"].as_str(),
            Some("Succeeded" | "Failed" | "Stopped")
        ) {
            return info;
        }
        assert!(
            Instant::now() < deadline_at,
            "case {case:?}: expected a terminal status within {deadline:?}, got {:?}",
            info["status"]
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// How long a suite waits for the deployment group it is deploying to to be free.
///
/// A deployment group runs ONE deployment at a time, so a second deployment to it
/// is refused while the first is active ([`deployment_on_free_group`] waits that
/// out): the wait is one deployment's own run, so this sits above the deadline a
/// single deployment is given.
pub(crate) const GROUP_FREE_DEADLINE: Duration = Duration::from_secs(15);

/// Create a deployment, waiting out the refusal a deployment group that is
/// already running one answers with.
///
/// A client that deploys twice to one deployment group is refused with
/// `DeploymentLimitExceededException` until the first deployment is terminal, so a
/// suite that deploys to one group repeatedly waits exactly as a client does. Any
/// other refusal is this suite's own failure.
pub(crate) async fn deployment_on_free_group(emulator: &Emulator, request: Value) -> Value {
    let deadline_at = Instant::now() + GROUP_FREE_DEADLINE;
    loop {
        let (status, body) = emulator.call("CreateDeployment", request.clone()).await;
        if status == 200 {
            return body;
        }
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "DeploymentLimitExceededException"),
            "CreateDeployment must succeed or wait for its deployment group, got {status} {body:?}"
        );
        assert!(
            Instant::now() < deadline_at,
            "expected the deployment group to be free within {GROUP_FREE_DEADLINE:?}, got \
             {body:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The id of a deployment created through [`deployment_on_free_group`].
pub(crate) async fn deployment_id_on_free_group(emulator: &Emulator, request: Value) -> String {
    deployment_on_free_group(emulator, request).await["deploymentId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

// ---------------------------------------------------------------------------
// Applications
// ---------------------------------------------------------------------------
/// RULE: a created resource is answered from the stored record — the id
/// `CreateApplication` minted comes back from `GetApplication`, with the
/// compute platform the request asked for, or CodeDeploy's `Server` default
/// when the request named none.
#[tokio::test]
async fn an_application_reads_back_as_it_was_created() {
    let emulator = Emulator::new();
    // Rows differ along `computePlatform`: every value the model's enum carries,
    // plus its absence.
    let cases: &[(&str, Option<&str>, &str)] = &[
        ("app-server", Some("Server"), "Server"),
        ("app-lambda", Some("Lambda"), "Lambda"),
        ("app-ecs", Some("ECS"), "ECS"),
        ("app-defaulted", None, "Server"),
    ];

    let mut seen_ids: Vec<String> = Vec::new();
    for (name, requested, expected) in cases {
        let mut request = json!({"applicationName": name});
        if let Some(platform) = requested {
            request["computePlatform"] = json!(platform);
        }
        let created = emulator.ok("CreateApplication", request).await;
        let application_id = created["applicationId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !application_id.is_empty(),
            "case {name:?}: expected a minted applicationId, got {created:?}"
        );

        let fetched = emulator
            .ok("GetApplication", json!({"applicationName": name}))
            .await;
        let application = &fetched["application"];
        assert_eq!(
            application["applicationId"].as_str(),
            Some(application_id.as_str()),
            "case {name:?}: expected the stored applicationId {application_id}, got {application:?}"
        );
        assert_eq!(
            application["applicationName"].as_str(),
            Some(*name),
            "case {name:?}: expected the stored name, got {application:?}"
        );
        assert_eq!(
            application["computePlatform"].as_str(),
            Some(*expected),
            "case {name:?}: expected computePlatform {expected}, got {application:?}"
        );
        assert_eq!(
            application["linkedToGitHub"].as_bool(),
            Some(false),
            "case {name:?}: expected linkedToGitHub false, got {application:?}"
        );
        assert!(
            application["createTime"].as_f64().unwrap_or_default() > 0.0,
            "case {name:?}: expected a createTime, got {application:?}"
        );

        // A second read answers the same record: reporting changes nothing.
        let again = emulator
            .ok("GetApplication", json!({"applicationName": name}))
            .await;
        assert_eq!(
            again, fetched,
            "case {name:?}: expected a read to be repeatable, got {again:?}"
        );

        assert!(
            !seen_ids.contains(&application_id),
            "case {name:?}: expected a distinct applicationId, {application_id} was minted twice"
        );
        seen_ids.push(application_id);
    }

    let listed = emulator.ok("ListApplications", json!({})).await;
    let applications = listed["applications"].as_array().cloned().unwrap_or_default();
    for (name, _, _) in cases {
        assert!(
            applications.iter().any(|listed| listed.as_str() == Some(*name)),
            "case {name:?}: expected ListApplications to carry it, got {applications:?}"
        );
    }
}

/// RULE: an application name is unique per account, so re-creating one is
/// refused with `ApplicationAlreadyExistsException` whatever the name looks like.
#[tokio::test]
async fn re_creating_an_application_is_refused() {
    let emulator = Emulator::new();
    // Rows differ along the FORM of an accepted name: plain, every punctuation
    // character the modelled pattern allows, and the modelled maximum length.
    let cases: &[&str] = &["demo-app", "a+b=c,d.e@f_g-h", &"n".repeat(100)];

    for name in cases {
        emulator
            .ok("CreateApplication", json!({"applicationName": name}))
            .await;
        let (status, body) = emulator
            .call("CreateApplication", json!({"applicationName": name}))
            .await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400 on the second create, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ApplicationAlreadyExistsException",
            "case {name:?}: expected ApplicationAlreadyExistsException, got {body:?}"
        );
    }
}

/// RULE: a refused input member is answered with CodeDeploy's OWN code for that
/// member — the `…RequiredException` when it was not given (absent or empty), the
/// `Invalid…Exception` when the value is not one the service accepts — on every
/// operation that carries the member, not a generic `ValidationException`.
#[tokio::test]
async fn a_refused_member_is_answered_with_codedeploys_code_for_it() {
    let emulator = Emulator::new();
    seeded_group(&emulator, "refusals", "Server").await;

    // Rows span the members and the two refusal kinds: absent, empty, too long,
    // outside the pattern, and outside an enum's value set.
    let long_name = "n".repeat(101);
    let cases: &[(&str, Value, &str)] = &[
        (
            "CreateApplication",
            json!({}),
            "ApplicationNameRequiredException",
        ),
        (
            "CreateApplication",
            json!({"applicationName": ""}),
            "ApplicationNameRequiredException",
        ),
        (
            "CreateApplication",
            json!({"applicationName": long_name}),
            "InvalidApplicationNameException",
        ),
        (
            "CreateApplication",
            json!({"applicationName": "has spaces"}),
            "InvalidApplicationNameException",
        ),
        (
            "CreateApplication",
            json!({"applicationName": "ok-name", "computePlatform": "Windows"}),
            "InvalidComputePlatformException",
        ),
        (
            "GetApplication",
            json!({"applicationName": ""}),
            "ApplicationNameRequiredException",
        ),
        (
            "GetApplication",
            json!({"applicationName": "has spaces"}),
            "InvalidApplicationNameException",
        ),
        (
            "DeleteApplication",
            json!({"applicationName": ""}),
            "ApplicationNameRequiredException",
        ),
        (
            "ListDeployments",
            json!({"applicationName": "refusals", "deploymentGroupName": ""}),
            "DeploymentGroupNameRequiredException",
        ),
        (
            "GetDeploymentGroup",
            json!({"applicationName": "refusals", "deploymentGroupName": "has spaces"}),
            "InvalidDeploymentGroupNameException",
        ),
        (
            "GetDeployment",
            json!({}),
            "DeploymentIdRequiredException",
        ),
        (
            "GetDeploymentConfig",
            json!({"deploymentConfigName": ""}),
            "DeploymentConfigNameRequiredException",
        ),
        (
            "CreateDeploymentGroup",
            json!({
                "applicationName": "refusals",
                "deploymentGroupName": "ok-group",
                "serviceRoleArn": ""
            }),
            "RoleRequiredException",
        ),
    ];

    for (operation, request, expected) in cases {
        let (status, body) = emulator.call(operation, request.clone()).await;
        assert_eq!(
            status, 400,
            "case {operation}/{request}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            *expected,
            "case {operation}/{request}: expected {expected}, got {body:?}"
        );
    }
}

/// RULE: `ListApplications` is `@paginated` on `nextToken` alone, so a token
/// CONTINUES the listing — every stored name is reported exactly once across the
/// pages — and a token this service did not issue is refused.
#[tokio::test]
async fn listing_applications_pages_without_repeating_or_restarting() {
    let emulator = Emulator::new();
    let total = 150;
    for index in 0..total {
        emulator
            .ok(
                "CreateApplication",
                json!({"applicationName": format!("paged-{index:03}")}),
            )
            .await;
    }

    let mut collected: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let request = match &token {
            Some(token) => json!({"nextToken": token}),
            None => json!({}),
        };
        let page = emulator.ok("ListApplications", request).await;
        let names: Vec<String> = page["applications"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|name| name.as_str().map(str::to_string))
            .collect();
        assert!(
            !names.is_empty(),
            "case page {pages}: expected a non-empty page, got {page:?}"
        );
        collected.extend(names);
        pages += 1;
        assert!(pages <= 5, "case page {pages}: paging did not terminate");
        token = page["nextToken"].as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
    }

    assert!(
        pages > 1,
        "case 150 applications: expected more than one page, got {pages}"
    );
    assert_eq!(
        collected.len(),
        total,
        "case 150 applications: expected every name exactly once, got {} names",
        collected.len()
    );
    let mut sorted = collected.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        total,
        "case 150 applications: expected no repeats across pages, got {} distinct of {}",
        sorted.len(),
        collected.len()
    );

    for bogus in ["not-a-token", "p", "pzzz-", ""] {
        let (status, body) = emulator
            .call("ListApplications", json!({"nextToken": bogus}))
            .await;
        assert_eq!(
            status, 400,
            "case token {bogus:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidNextTokenException",
            "case token {bogus:?}: expected InvalidNextTokenException, got {body:?}"
        );
    }
}

/// RULE: deleting an application removes it and the deployment groups it owns,
/// and deleting one that is not there is a success.
#[tokio::test]
async fn deleting_an_application_removes_it_and_its_groups() {
    let emulator = Emulator::new();
    let group = seeded_group(&emulator, "doomed", "Server").await;
    emulator
        .ok("CreateApplication", json!({"applicationName": "survivor"}))
        .await;

    emulator
        .ok("DeleteApplication", json!({"applicationName": "doomed"}))
        .await;

    let (status, body) = emulator
        .call("GetApplication", json!({"applicationName": "doomed"}))
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ApplicationDoesNotExistException"),
        "case deleted application: expected ApplicationDoesNotExistException, got {status} {body:?}"
    );

    let (status, body) = emulator
        .call(
            "GetDeploymentGroup",
            json!({"applicationName": "doomed", "deploymentGroupName": group}),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ApplicationDoesNotExistException"),
        "case group of a deleted application: expected ApplicationDoesNotExistException, got {status} {body:?}"
    );

    let listed = emulator.ok("ListApplications", json!({})).await;
    let applications = listed["applications"].as_array().cloned().unwrap_or_default();
    assert!(
        applications.iter().all(|name| name.as_str() != Some("doomed")),
        "case deleted application: expected it gone from ListApplications, got {applications:?}"
    );
    assert!(
        applications.iter().any(|name| name.as_str() == Some("survivor")),
        "case surviving application: expected it still listed, got {applications:?}"
    );

    // Deleting what is not there is a no-op success, twice over: the name just
    // deleted, and one never created.
    for name in ["doomed", "never-existed"] {
        let (status, body) = emulator
            .call("DeleteApplication", json!({"applicationName": name}))
            .await;
        assert_eq!(
            status, 200,
            "case delete {name:?}: expected a no-op success, got {status} {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Deployment configurations
// ---------------------------------------------------------------------------

/// RULE: the `CodeDeployDefault.*` configurations CodeDeploy ships resolve
/// without anyone creating them, each with the healthy-hosts or traffic-routing
/// setting documented for it; anything else does not resolve.
#[tokio::test]
async fn the_builtin_deployment_configurations_resolve() {
    let emulator = Emulator::new();
    // Catalog source: the AWS CodeDeploy User Guide's deployment-configuration
    // tables (EC2/on-premises, and AWS Lambda / Amazon ECS).
    struct Builtin {
        name: &'static str,
        platform: &'static str,
        /// `minimumHealthyHosts` as (type, value).
        healthy: Option<(&'static str, i64)>,
        /// `trafficRoutingConfig.type`.
        routing: Option<&'static str>,
    }
    let cases = &[
        Builtin {
            name: "CodeDeployDefault.OneAtATime",
            platform: "Server",
            healthy: Some(("FLEET_PERCENT", 99)),
            routing: None,
        },
        Builtin {
            name: "CodeDeployDefault.HalfAtATime",
            platform: "Server",
            healthy: Some(("FLEET_PERCENT", 50)),
            routing: None,
        },
        Builtin {
            name: "CodeDeployDefault.AllAtOnce",
            platform: "Server",
            healthy: Some(("HOST_COUNT", 0)),
            routing: None,
        },
        Builtin {
            name: "CodeDeployDefault.LambdaAllAtOnce",
            platform: "Lambda",
            healthy: None,
            routing: Some("AllAtOnce"),
        },
        Builtin {
            name: "CodeDeployDefault.ECSAllAtOnce",
            platform: "ECS",
            healthy: None,
            routing: Some("AllAtOnce"),
        },
    ];

    for Builtin {
        name,
        platform,
        healthy,
        routing,
    } in cases
    {
        let fetched = emulator
            .ok("GetDeploymentConfig", json!({"deploymentConfigName": name}))
            .await;
        let info = &fetched["deploymentConfigInfo"];
        assert_eq!(
            info["deploymentConfigName"].as_str(),
            Some(*name),
            "case {name}: expected the configuration's own name, got {info:?}"
        );
        assert!(
            !info["deploymentConfigId"].as_str().unwrap_or_default().is_empty(),
            "case {name}: expected a deploymentConfigId, got {info:?}"
        );
        assert_eq!(
            info["computePlatform"].as_str(),
            Some(*platform),
            "case {name}: expected computePlatform {platform}, got {info:?}"
        );
        match healthy {
            Some((kind, value)) => assert_eq!(
                (
                    info["minimumHealthyHosts"]["type"].as_str(),
                    info["minimumHealthyHosts"]["value"].as_i64()
                ),
                (Some(*kind), Some(*value)),
                "case {name}: expected minimumHealthyHosts {kind}/{value}, got {info:?}"
            ),
            None => assert!(
                info["minimumHealthyHosts"].is_null(),
                "case {name}: expected no minimumHealthyHosts, got {info:?}"
            ),
        }
        match routing {
            Some(kind) => assert_eq!(
                info["trafficRoutingConfig"]["type"].as_str(),
                Some(*kind),
                "case {name}: expected trafficRoutingConfig {kind}, got {info:?}"
            ),
            None => assert!(
                info["trafficRoutingConfig"].is_null(),
                "case {name}: expected no trafficRoutingConfig, got {info:?}"
            ),
        }
    }

    for unknown in ["CodeDeployDefault.Nope", "my-own-config"] {
        let (status, body) = emulator
            .call(
                "GetDeploymentConfig",
                json!({"deploymentConfigName": unknown}),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "DeploymentConfigDoesNotExistException"),
            "case {unknown}: expected DeploymentConfigDoesNotExistException, got {status} {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Deployment groups
// ---------------------------------------------------------------------------

/// RULE: a deployment group is stored under its application with everything the
/// request gave it, and an omitted `deploymentConfigName` resolves to the
/// default built-in for the application's compute platform.
#[tokio::test]
async fn a_deployment_group_reads_back_as_it_was_created() {
    let emulator = Emulator::new();
    // Rows differ along compute platform (which default applies) and along how
    // much optional configuration the request carried.
    let cases: &[(&str, &str, Option<&str>, &str, Value)] = &[
        (
            "grp-server",
            "Server",
            None,
            "CodeDeployDefault.OneAtATime",
            json!({}),
        ),
        (
            "grp-lambda",
            "Lambda",
            None,
            "CodeDeployDefault.LambdaAllAtOnce",
            json!({}),
        ),
        (
            "grp-ecs",
            "ECS",
            None,
            "CodeDeployDefault.ECSAllAtOnce",
            json!({}),
        ),
        (
            "grp-explicit",
            "Server",
            Some("CodeDeployDefault.HalfAtATime"),
            "CodeDeployDefault.HalfAtATime",
            json!({
                "ec2TagFilters": [{"Key": "Name", "Value": "web", "Type": "KEY_AND_VALUE"}],
                "autoRollbackConfiguration": {"enabled": true, "events": ["DEPLOYMENT_FAILURE"]},
                "deploymentStyle": {"deploymentType": "IN_PLACE", "deploymentOption": "WITHOUT_TRAFFIC_CONTROL"},
                "terminationHookEnabled": true,
            }),
        ),
    ];

    for (application, platform, config, expected_config, extras) in cases {
        emulator
            .ok(
                "CreateApplication",
                json!({"applicationName": application, "computePlatform": platform}),
            )
            .await;
        let mut request = json!({
            "applicationName": application,
            "deploymentGroupName": "dg",
            "serviceRoleArn": "arn:aws:iam::123456789012:role/CodeDeployRole",
        });
        if let Some(config) = config {
            request["deploymentConfigName"] = json!(config);
        }
        for (member, value) in extras.as_object().cloned().unwrap_or_default() {
            request[member] = value;
        }

        let created = emulator.ok("CreateDeploymentGroup", request.clone()).await;
        let group_id = created["deploymentGroupId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !group_id.is_empty(),
            "case {application}: expected a minted deploymentGroupId, got {created:?}"
        );

        let fetched = emulator
            .ok(
                "GetDeploymentGroup",
                json!({"applicationName": application, "deploymentGroupName": "dg"}),
            )
            .await;
        let info = &fetched["deploymentGroupInfo"];
        assert_eq!(
            info["deploymentGroupId"].as_str(),
            Some(group_id.as_str()),
            "case {application}: expected the stored deploymentGroupId, got {info:?}"
        );
        assert_eq!(
            info["applicationName"].as_str(),
            Some(*application),
            "case {application}: expected the owning application, got {info:?}"
        );
        assert_eq!(
            info["deploymentConfigName"].as_str(),
            Some(*expected_config),
            "case {application}: expected deploymentConfigName {expected_config}, got {info:?}"
        );
        assert_eq!(
            info["serviceRoleArn"].as_str(),
            Some("arn:aws:iam::123456789012:role/CodeDeployRole"),
            "case {application}: expected the stored serviceRoleArn, got {info:?}"
        );
        assert_eq!(
            info["computePlatform"].as_str(),
            Some(*platform),
            "case {application}: expected the application's computePlatform, got {info:?}"
        );
        // Whatever optional configuration the request carried comes back; what it
        // did not carry is absent rather than invented.
        if extras.get("autoRollbackConfiguration").is_some() {
            assert_eq!(
                info["autoRollbackConfiguration"]["events"][0].as_str(),
                Some("DEPLOYMENT_FAILURE"),
                "case {application}: expected the stored autoRollbackConfiguration, got {info:?}"
            );
            assert_eq!(
                info["ec2TagFilters"][0]["Key"].as_str(),
                Some("Name"),
                "case {application}: expected the stored ec2TagFilters, got {info:?}"
            );
            assert_eq!(
                info["terminationHookEnabled"].as_bool(),
                Some(true),
                "case {application}: expected terminationHookEnabled true, got {info:?}"
            );
        } else {
            // A list member the request did not carry reads back as the empty
            // list, not as an absent member.
            assert_eq!(
                info["ec2TagFilters"].as_array().map(Vec::len),
                Some(0),
                "case {application}: expected an empty ec2TagFilters, got {info:?}"
            );
            assert!(
                info["autoRollbackConfiguration"].is_null(),
                "case {application}: expected no autoRollbackConfiguration, got {info:?}"
            );
        }

        // The same group name is taken now.
        let (status, body) = emulator.call("CreateDeploymentGroup", request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "DeploymentGroupAlreadyExistsException"),
            "case {application}: expected DeploymentGroupAlreadyExistsException, got {status} {body:?}"
        );
    }
}

/// RULE: a deployment group can only be created on an application that exists,
/// with a deployment configuration that exists.
#[tokio::test]
async fn a_deployment_group_needs_an_application_and_a_configuration() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateApplication", json!({"applicationName": "real-app"}))
        .await;

    let cases: &[(&str, &str, Option<&str>, &str)] = &[
        (
            "missing-app",
            "dg",
            None,
            "ApplicationDoesNotExistException",
        ),
        (
            "real-app",
            "dg",
            Some("CodeDeployDefault.NotAThing"),
            "DeploymentConfigDoesNotExistException",
        ),
    ];

    for (application, group, config, expected) in cases {
        let mut request = json!({
            "applicationName": application,
            "deploymentGroupName": group,
            "serviceRoleArn": "arn:aws:iam::123456789012:role/CodeDeployRole",
        });
        if let Some(config) = config {
            request["deploymentConfigName"] = json!(config);
        }
        let (status, body) = emulator.call("CreateDeploymentGroup", request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, *expected),
            "case {application}/{group}: expected {expected}, got {status} {body:?}"
        );
    }

    let (status, body) = emulator
        .call(
            "GetDeploymentGroup",
            json!({"applicationName": "real-app", "deploymentGroupName": "nope"}),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "DeploymentGroupDoesNotExistException"),
        "case unknown group: expected DeploymentGroupDoesNotExistException, got {status} {body:?}"
    );
}

// ---------------------------------------------------------------------------
// Deployments
// ---------------------------------------------------------------------------

/// RULE: `GetDeployment` reconstructs the deployment from the stored record —
/// the revision exactly as supplied, the effective configuration, the creator,
/// and the application's compute platform — for every form a revision takes.
#[tokio::test]
async fn a_deployment_reads_back_as_it_was_created() {
    let emulator = Emulator::new();
    let group = seeded_group(&emulator, "deployed", "Server").await;

    // Rows differ along the revision's own form: each member the model's
    // `RevisionLocation` carries.
    let cases: &[(&str, Value)] = &[
        ("s3", s3_revision("my-bucket", "app.zip")),
        (
            "github",
            json!({"revisionType": "GitHub", "gitHubLocation": {"repository": "octo/app", "commitId": "0f1e2d3"}}),
        ),
        (
            "string",
            json!({"revisionType": "String", "string": {"content": "version: 0.0", "sha256": "abc"}}),
        ),
        (
            "appspec",
            json!({"revisionType": "AppSpecContent", "appSpecContent": {"content": "version: 0.0", "sha256": "def"}}),
        ),
    ];

    let mut seen_ids: Vec<String> = Vec::new();
    for (case, revision) in cases {
        let created = deployment_on_free_group(
            &emulator,
            json!({
                "applicationName": "deployed",
                "deploymentGroupName": group,
                "revision": revision,
                "description": format!("deploy {case}"),
            }),
        )
        .await;
        let deployment_id = created["deploymentId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            deployment_id.starts_with("d-") && deployment_id.len() == 11,
            "case {case}: expected a d-XXXXXXXXX deployment id, got {created:?}"
        );
        assert!(
            !seen_ids.contains(&deployment_id),
            "case {case}: expected a distinct deploymentId, {deployment_id} was minted twice"
        );
        seen_ids.push(deployment_id.clone());

        let fetched = emulator
            .ok("GetDeployment", json!({"deploymentId": deployment_id}))
            .await;
        let info = &fetched["deploymentInfo"];
        assert_eq!(
            info["deploymentId"].as_str(),
            Some(deployment_id.as_str()),
            "case {case}: expected the stored deploymentId, got {info:?}"
        );
        assert_eq!(
            info["applicationName"].as_str(),
            Some("deployed"),
            "case {case}: expected the stored applicationName, got {info:?}"
        );
        assert_eq!(
            info["deploymentGroupName"].as_str(),
            Some(group.as_str()),
            "case {case}: expected the stored deploymentGroupName, got {info:?}"
        );
        assert_eq!(
            info["deploymentConfigName"].as_str(),
            Some("CodeDeployDefault.OneAtATime"),
            "case {case}: expected the group's effective configuration, got {info:?}"
        );
        assert_eq!(
            info["revision"], *revision,
            "case {case}: expected the revision as supplied, got {:?}",
            info["revision"]
        );
        assert_eq!(
            info["creator"].as_str(),
            Some("user"),
            "case {case}: expected creator user, got {info:?}"
        );
        assert_eq!(
            info["computePlatform"].as_str(),
            Some("Server"),
            "case {case}: expected the application's computePlatform, got {info:?}"
        );
        assert_eq!(
            info["description"].as_str(),
            Some(format!("deploy {case}").as_str()),
            "case {case}: expected the stored description, got {info:?}"
        );
        assert_eq!(
            info["ignoreApplicationStopFailures"].as_bool(),
            Some(false),
            "case {case}: expected the modelled default false, got {info:?}"
        );
        assert!(
            info["createTime"].as_f64().unwrap_or_default() > 0.0,
            "case {case}: expected a createTime, got {info:?}"
        );
    }

    // The second deployment onwards reports the one before it as its previous
    // revision.
    let latest = seen_ids.last().expect("a deployment was created");
    let info = emulator
        .ok("GetDeployment", json!({"deploymentId": latest}))
        .await;
    assert_eq!(
        info["deploymentInfo"]["previousRevision"], cases[cases.len() - 2].1,
        "case previousRevision: expected the revision of the deployment before it, got {:?}",
        info["deploymentInfo"]["previousRevision"]
    );
}

/// RULE: a deployment the emulator created starts non-terminal and the clock
/// owes it its terminal status, with a start and a completion time, after which
/// reading it reports the same record every time. The row here is a deployment
/// whose group resolves a target, so its terminal status is `Succeeded`; which
/// terminal status a group's targets imply is covered in
/// `codedeploy_deployment_outcome`.
#[tokio::test]
async fn a_deployment_reaches_a_terminal_status() {
    let emulator = Emulator::new();
    let group = seeded_group(&emulator, "waited-on", "Lambda").await;
    let created = emulator
        .ok(
            "CreateDeployment",
            json!({
                "applicationName": "waited-on",
                "deploymentGroupName": group,
                "revision": s3_revision("my-bucket", "app.zip"),
            }),
        )
        .await;
    let deployment_id = created["deploymentId"].as_str().expect("a deployment id");

    let first = emulator
        .ok("GetDeployment", json!({"deploymentId": deployment_id}))
        .await;
    let status = first["deploymentInfo"]["status"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        matches!(status.as_str(), "Created" | "InProgress"),
        "case straight after create: expected a non-terminal status, got {status:?}"
    );
    assert!(
        first["deploymentInfo"]["completeTime"].is_null(),
        "case straight after create: expected no completeTime, got {:?}",
        first["deploymentInfo"]
    );

    // The transition is owed by the clock, not by the poll: it arrives without
    // anyone asking more than once.
    let deadline = Instant::now() + POLL_DEADLINE;
    let terminal = loop {
        let polled = emulator
            .ok("GetDeployment", json!({"deploymentId": deployment_id}))
            .await;
        if polled["deploymentInfo"]["status"].as_str() == Some("Succeeded") {
            break polled;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for completion: expected Succeeded within {POLL_DEADLINE:?}, got {:?}",
            polled["deploymentInfo"]["status"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };

    let info = &terminal["deploymentInfo"];
    let create_time = info["createTime"].as_f64().unwrap_or_default();
    let start_time = info["startTime"].as_f64().unwrap_or_default();
    let complete_time = info["completeTime"].as_f64().unwrap_or_default();
    assert!(
        create_time <= start_time && start_time <= complete_time,
        "case terminal deployment: expected createTime <= startTime <= completeTime, got {info:?}"
    );

    // Terminal is terminal: reading it again changes nothing.
    let again = emulator
        .ok("GetDeployment", json!({"deploymentId": deployment_id}))
        .await;
    assert_eq!(
        again, terminal,
        "case terminal deployment: expected a repeatable read, got {again:?}"
    );
}

/// RULE: a deployment names an application, a deployment group of it and a
/// revision; each one missing or unknown is refused with its own modelled code.
#[tokio::test]
async fn a_deployment_needs_an_application_a_group_and_a_revision() {
    let emulator = Emulator::new();
    let group = seeded_group(&emulator, "checked", "Server").await;
    let revision = s3_revision("my-bucket", "app.zip");

    let cases: &[(&str, Value, &str)] = &[
        (
            "unknown application",
            json!({"applicationName": "ghost", "deploymentGroupName": group, "revision": revision}),
            "ApplicationDoesNotExistException",
        ),
        (
            "unknown group",
            json!({"applicationName": "checked", "deploymentGroupName": "ghost-dg", "revision": revision}),
            "DeploymentGroupDoesNotExistException",
        ),
        (
            "no group",
            json!({"applicationName": "checked", "revision": revision}),
            "DeploymentGroupNameRequiredException",
        ),
        (
            "no revision",
            json!({"applicationName": "checked", "deploymentGroupName": group}),
            "RevisionRequiredException",
        ),
        (
            "empty revision",
            json!({"applicationName": "checked", "deploymentGroupName": group, "revision": {}}),
            "InvalidRevisionException",
        ),
        (
            "revision naming only a type",
            json!({"applicationName": "checked", "deploymentGroupName": group, "revision": {"revisionType": "S3"}}),
            "InvalidRevisionException",
        ),
        (
            "unknown configuration",
            json!({
                "applicationName": "checked",
                "deploymentGroupName": group,
                "revision": revision,
                "deploymentConfigName": "CodeDeployDefault.NotAThing",
            }),
            "DeploymentConfigDoesNotExistException",
        ),
    ];

    for (case, request, expected) in cases {
        let (status, body) = emulator.call("CreateDeployment", request.clone()).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, *expected),
            "case {case}: expected {expected}, got {status} {body:?}"
        );
    }

    // Both ids are WELL FORMED (`d-` and nine characters of the deployment-id
    // alphabet), so they are answered on existence rather than on format; the
    // format gate itself is covered in `codedeploy_deployment_outcome`.
    for unknown in ["d-NOTREAL12", "d-ZZZZZZZZZ"] {
        let (status, body) = emulator
            .call("GetDeployment", json!({"deploymentId": unknown}))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "DeploymentDoesNotExistException"),
            "case {unknown}: expected DeploymentDoesNotExistException, got {status} {body:?}"
        );
    }

    // An id that is not in CodeDeploy's deployment-id format at all is refused
    // for its FORMAT, before the deployment is looked up — so a caller can tell
    // "you asked for something that cannot be a deployment id" from "that
    // deployment does not exist". Rows differ along how the id leaves the
    // format: prose, the wrong prefix, the wrong alphabet, nothing after the
    // prefix.
    for malformed in [
        "not-a-deployment-id",
        "x-ABCDEFGHI",
        "d-abcdefghi",
        "d-ABCDEFGH!",
        "d-",
    ] {
        let (status, body) = emulator
            .call("GetDeployment", json!({"deploymentId": malformed}))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidDeploymentIdException"),
            "case {malformed}: expected InvalidDeploymentIdException, got {status} {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(
                format!("Specified DeploymentId is not in the valid format: {malformed}").as_str()
            ),
            "case {malformed}: expected the clause naming the id, got {body:?}"
        );
    }
}

/// RULE: `ListDeployments` answers the stored deployment ids newest first,
/// narrowed by each filter the request carries, and refuses a filter it cannot
/// resolve.
#[tokio::test]
async fn listing_deployments_orders_and_filters_by_the_stored_records() {
    let emulator = Emulator::new();
    let first_group = seeded_group(&emulator, "alpha", "Server").await;
    let second_group = seeded_group(&emulator, "beta", "Server").await;
    // A second group on `alpha`, so the group filter has something to exclude.
    emulator
        .ok(
            "CreateDeploymentGroup",
            json!({
                "applicationName": "alpha",
                "deploymentGroupName": "alpha-other",
                "serviceRoleArn": "arn:aws:iam::123456789012:role/CodeDeployRole",
            }),
        )
        .await;

    let mut created: Vec<(String, String, String)> = Vec::new();
    for (application, group) in [
        ("alpha", first_group.as_str()),
        ("alpha", "alpha-other"),
        ("beta", second_group.as_str()),
        ("alpha", first_group.as_str()),
    ] {
        let response = deployment_on_free_group(
            &emulator,
            json!({
                "applicationName": application,
                "deploymentGroupName": group,
                "revision": s3_revision("my-bucket", "app.zip"),
            }),
        )
        .await;
        created.push((
            application.to_string(),
            group.to_string(),
            response["deploymentId"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        ));
    }

    let ids = |body: &Value| -> Vec<String> {
        body["deployments"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|id| id.as_str().map(str::to_string))
            .collect()
    };

    // Newest first, across every application.
    let all = ids(&emulator.ok("ListDeployments", json!({})).await);
    let newest_first: Vec<String> = created
        .iter()
        .rev()
        .map(|(_, _, id)| id.clone())
        .collect();
    assert_eq!(
        all, newest_first,
        "case no filter: expected the stored ids newest first, got {all:?}"
    );

    // Rows differ along which filter is applied.
    let cases: &[(&str, Value, Vec<String>)] = &[
        (
            "by application and its other group",
            json!({"applicationName": "alpha", "deploymentGroupName": "alpha-other"}),
            created
                .iter()
                .rev()
                .filter(|(application, group, _)| {
                    application == "alpha" && group == "alpha-other"
                })
                .map(|(_, _, id)| id.clone())
                .collect(),
        ),
        (
            "by application and group",
            json!({"applicationName": "alpha", "deploymentGroupName": first_group}),
            created
                .iter()
                .rev()
                .filter(|(application, group, _)| application == "alpha" && group == &first_group)
                .map(|(_, _, id)| id.clone())
                .collect(),
        ),
        (
            "by a status nothing holds",
            json!({"includeOnlyStatuses": ["Stopped"]}),
            Vec::new(),
        ),
        (
            "by an external id nothing holds",
            json!({"externalId": "arn:aws:cloudformation:us-west-2:123456789012:stack/x"}),
            Vec::new(),
        ),
        (
            "by a creation window in the past",
            json!({"createTimeRange": {"start": 1.0, "end": 2.0}}),
            Vec::new(),
        ),
    ];

    for (case, request, expected) in cases {
        let got = ids(&emulator.ok("ListDeployments", request.clone()).await);
        assert_eq!(
            &got, expected,
            "case {case}: expected {expected:?}, got {got:?}"
        );
    }

    // A status every deployment eventually holds selects them all. Every group
    // here is an EC2/on-premises one that resolves no target, so the status they
    // all reach is `Failed`.
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let succeeded = ids(&emulator
            .ok("ListDeployments", json!({"includeOnlyStatuses": ["Failed"]}))
            .await);
        if succeeded == newest_first {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case by status Failed: expected every deployment within {POLL_DEADLINE:?}, got \
             {succeeded:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let refusals: &[(&str, Value, &str)] = &[
        (
            "a group without an application",
            json!({"deploymentGroupName": "alpha-other"}),
            "ApplicationNameRequiredException",
        ),
        (
            "an application without a group",
            json!({"applicationName": "alpha"}),
            "DeploymentGroupNameRequiredException",
        ),
        (
            "an unknown application",
            json!({"applicationName": "ghost", "deploymentGroupName": "ghost-dg"}),
            "ApplicationDoesNotExistException",
        ),
        (
            "an unknown group",
            json!({"applicationName": "alpha", "deploymentGroupName": "ghost-dg"}),
            "DeploymentGroupDoesNotExistException",
        ),
        (
            "a backwards creation window",
            json!({"createTimeRange": {"start": 2.0, "end": 1.0}}),
            "InvalidTimeRangeException",
        ),
        (
            "a token this service did not issue",
            json!({"nextToken": "not-a-token"}),
            "InvalidNextTokenException",
        ),
        (
            "a status outside the modelled set",
            json!({"includeOnlyStatuses": ["Exploded"]}),
            "InvalidDeploymentStatusException",
        ),
    ];

    for (case, request, expected) in refusals {
        let (status, body) = emulator.call("ListDeployments", request.clone()).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, *expected),
            "case {case}: expected {expected}, got {status} {body:?}"
        );
    }
}

/// RULE: a deployment group reports its last attempted and last successful
/// deployment, derived from the deployments actually stored for it.
#[tokio::test]
async fn a_deployment_group_reports_its_last_deployment() {
    let emulator = Emulator::new();
    let group = seeded_group(&emulator, "tracked", "Lambda").await;

    let before = emulator
        .ok(
            "GetDeploymentGroup",
            json!({"applicationName": "tracked", "deploymentGroupName": group}),
        )
        .await;
    assert!(
        before["deploymentGroupInfo"]["lastAttemptedDeployment"].is_null(),
        "case no deployments yet: expected no lastAttemptedDeployment, got {:?}",
        before["deploymentGroupInfo"]
    );

    let revision = s3_revision("my-bucket", "app.zip");
    let created = emulator
        .ok(
            "CreateDeployment",
            json!({
                "applicationName": "tracked",
                "deploymentGroupName": group,
                "revision": revision,
            }),
        )
        .await;
    let deployment_id = created["deploymentId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let after = emulator
        .ok(
            "GetDeploymentGroup",
            json!({"applicationName": "tracked", "deploymentGroupName": group}),
        )
        .await;
    let info = &after["deploymentGroupInfo"];
    assert_eq!(
        info["lastAttemptedDeployment"]["deploymentId"].as_str(),
        Some(deployment_id.as_str()),
        "case one deployment: expected it as lastAttemptedDeployment, got {info:?}"
    );
    assert_eq!(
        info["targetRevision"], revision,
        "case one deployment: expected its revision as the group's targetRevision, got {info:?}"
    );

    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let polled = emulator
            .ok(
                "GetDeploymentGroup",
                json!({"applicationName": "tracked", "deploymentGroupName": group}),
            )
            .await;
        let successful = &polled["deploymentGroupInfo"]["lastSuccessfulDeployment"];
        if successful["deploymentId"].as_str() == Some(deployment_id.as_str()) {
            assert_eq!(
                successful["status"].as_str(),
                Some("Succeeded"),
                "case succeeded deployment: expected the status with it, got {successful:?}"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case succeeded deployment: expected a lastSuccessfulDeployment within \
             {POLL_DEADLINE:?}, got {successful:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    // A deployment that FAILED is the group's last attempt but never its last
    // success: both members are derived from the stored deployments, not from the
    // fact that one was made.
    let failing_group = seeded_group(&emulator, "tracked-server", "Server").await;
    let failed = emulator
        .ok(
            "CreateDeployment",
            json!({
                "applicationName": "tracked-server",
                "deploymentGroupName": failing_group,
                "revision": revision,
            }),
        )
        .await;
    let failed_id = failed["deploymentId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let polled = emulator
            .ok(
                "GetDeploymentGroup",
                json!({"applicationName": "tracked-server", "deploymentGroupName": failing_group}),
            )
            .await;
        let info = &polled["deploymentGroupInfo"];
        if info["lastAttemptedDeployment"]["status"].as_str() == Some("Failed") {
            assert_eq!(
                info["lastAttemptedDeployment"]["deploymentId"].as_str(),
                Some(failed_id.as_str()),
                "case failed deployment: expected it as lastAttemptedDeployment, got {info:?}"
            );
            assert!(
                info["lastSuccessfulDeployment"].is_null(),
                "case failed deployment: expected no lastSuccessfulDeployment, got {info:?}"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case failed deployment: expected a terminal lastAttemptedDeployment within \
             {POLL_DEADLINE:?}, got {info:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ---------------------------------------------------------------------------
// Bounds
// ---------------------------------------------------------------------------

/// RULE: a quota CodeDeploy publishes is enforced, with the modelled
/// `…LimitExceededException` for it — the account's application quota, and an
/// application's deployment-group quota — so repeated requests cannot grow the
/// store without limit.
#[tokio::test]
async fn the_published_quotas_are_enforced() {
    let emulator = Emulator::new();
    // Quota source: "Quotas in AWS CodeDeploy" — 1,000 applications per account
    // per Region, mirrored by `state::MAX_APPLICATIONS`.
    let quota = 1_000;
    for index in 0..quota {
        emulator
            .ok(
                "CreateApplication",
                json!({"applicationName": format!("quota-{index:04}")}),
            )
            .await;
    }

    let (status, body) = emulator
        .call(
            "CreateApplication",
            json!({"applicationName": "one-too-many"}),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ApplicationLimitExceededException"),
        "case the application after the quota: expected ApplicationLimitExceededException, got {status} {body:?}"
    );

    // One under the quota accepts a create again, so the refusal is the quota
    // and not a wedged store.
    emulator
        .ok("DeleteApplication", json!({"applicationName": "quota-0000"}))
        .await;
    emulator
        .ok("CreateApplication", json!({"applicationName": "room-again"}))
        .await;
}

/// RULE: the deployment history this emulator retains is bounded, so creating
/// deployments without limit drops the oldest rather than growing the store —
/// and the newest deployments are always the ones still readable.
#[tokio::test]
async fn the_deployment_history_is_bounded() {
    let emulator = Emulator::new();
    seeded_group(&emulator, "churn", "Server").await;
    // Retention bound: `state::MAX_DEPLOYMENT_HISTORY`.
    let retained = 500;
    let mut ids: Vec<String> = Vec::new();
    for index in 0..(retained + 2) {
        // Each deployment goes to a group of its own: a deployment group runs one
        // deployment at a time, and what this case is about is the HISTORY's
        // bound, not one group's.
        let group = format!("churn-dg-{index}");
        emulator
            .ok(
                "CreateDeploymentGroup",
                json!({
                    "applicationName": "churn",
                    "deploymentGroupName": group,
                    "serviceRoleArn": "arn:aws:iam::123456789012:role/CodeDeployRole",
                }),
            )
            .await;
        let created = emulator
            .ok(
                "CreateDeployment",
                json!({
                    "applicationName": "churn",
                    "deploymentGroupName": group,
                    "revision": s3_revision("my-bucket", "app.zip"),
                }),
            )
            .await;
        ids.push(
            created["deploymentId"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }

    // The two oldest fell out of the history; the newest is still there.
    for dropped in &ids[..2] {
        let (status, body) = emulator
            .call("GetDeployment", json!({"deploymentId": dropped}))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "DeploymentDoesNotExistException"),
            "case evicted {dropped}: expected DeploymentDoesNotExistException, got {status} {body:?}"
        );
    }
    let newest = ids.last().expect("deployments were created");
    emulator
        .ok("GetDeployment", json!({"deploymentId": newest}))
        .await;

    // Walking every page reports exactly the retained window, newest first.
    let mut listed: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let request = match &token {
            Some(token) => json!({"nextToken": token}),
            None => json!({}),
        };
        let page = emulator.ok("ListDeployments", request).await;
        listed.extend(
            page["deployments"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|id| id.as_str().map(str::to_string)),
        );
        token = page["nextToken"].as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
        assert!(
            listed.len() <= retained,
            "case paging the history: expected at most {retained} ids, got {}",
            listed.len()
        );
    }
    let expected: Vec<String> = ids.iter().skip(2).rev().cloned().collect();
    assert_eq!(
        listed, expected,
        "case paging the history: expected the retained window newest first, got {} ids",
        listed.len()
    );
}

/// RULE: a deployment group's instance selector is given EITHER as a flat tag
/// FILTER list OR as the grouped tag SET of the same tags, never as both at
/// once, so a request carrying both forms of one selector is refused with that
/// selector's `Invalid…TagCombinationException` and stores nothing — while
/// either form alone, and the two different selectors together, are accepted.
#[tokio::test]
async fn a_deployment_group_takes_one_form_of_each_tag_selector() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateApplication", json!({"applicationName": "tag-forms"}))
        .await;

    // Both selector members and both of their forms, from
    // `smithy_query(query="shape",shape="EC2TagSet,EC2TagFilter,OnPremisesTagSet,TagFilter")`.
    let ec2_filters = json!([{"Key": "Name", "Value": "web", "Type": "KEY_AND_VALUE"}]);
    let ec2_set = json!({"ec2TagSetList": [[{"Key": "Name", "Value": "web", "Type": "KEY_AND_VALUE"}]]});
    let premises_filters = json!([{"Key": "Stage", "Value": "prod", "Type": "KEY_AND_VALUE"}]);
    let premises_set =
        json!({"onPremisesTagSetList": [[{"Key": "Stage", "Value": "prod", "Type": "KEY_AND_VALUE"}]]});

    // Rows differ along which selector is doubled up, and include the empty
    // filter list — an explicitly specified list is still specified.
    let cases: &[(&str, Value, Option<&str>)] = &[
        (
            "ec2 filters alone",
            json!({"ec2TagFilters": ec2_filters}),
            None,
        ),
        ("ec2 tag set alone", json!({"ec2TagSet": ec2_set}), None),
        (
            "on-premises filters alone",
            json!({"onPremisesInstanceTagFilters": premises_filters}),
            None,
        ),
        (
            "on-premises tag set alone",
            json!({"onPremisesTagSet": premises_set}),
            None,
        ),
        (
            "one form of each different selector",
            json!({"ec2TagFilters": ec2_filters, "onPremisesTagSet": premises_set}),
            None,
        ),
        (
            "both forms of the ec2 selector",
            json!({"ec2TagFilters": ec2_filters, "ec2TagSet": ec2_set}),
            Some("InvalidEC2TagCombinationException"),
        ),
        (
            "both forms of the ec2 selector, the filter list empty",
            json!({"ec2TagFilters": [], "ec2TagSet": ec2_set}),
            Some("InvalidEC2TagCombinationException"),
        ),
        (
            "both forms of the on-premises selector",
            json!({
                "onPremisesInstanceTagFilters": premises_filters,
                "onPremisesTagSet": premises_set
            }),
            Some("InvalidOnPremisesTagCombinationException"),
        ),
    ];

    for (index, (case, selector, expected)) in cases.iter().enumerate() {
        let group = format!("tag-forms-{index}");
        let mut request = json!({
            "applicationName": "tag-forms",
            "deploymentGroupName": group,
            "serviceRoleArn": "arn:aws:iam::123456789012:role/CodeDeployRole",
        });
        for (member, value) in selector.as_object().expect("the selector is an object") {
            request[member] = value.clone();
        }
        let (status, body) = emulator.call("CreateDeploymentGroup", request).await;
        match expected {
            None => assert_eq!(
                status, 200,
                "case {case}: expected the group created, got {status} {body:?}"
            ),
            Some(code) => {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, *code),
                    "case {case}: expected 400 {code}, got {status} {body:?}"
                );
                // A refusal stores nothing: the group is not readable back.
                let (read_status, read_body) = emulator
                    .call(
                        "GetDeploymentGroup",
                        json!({"applicationName": "tag-forms", "deploymentGroupName": group}),
                    )
                    .await;
                assert_eq!(
                    (read_status, error_code(&read_body).as_str()),
                    (400, "DeploymentGroupDoesNotExistException"),
                    "case {case}: a refused create must store nothing, got {read_status} \
                     {read_body:?}"
                );
            }
        }
    }
}
