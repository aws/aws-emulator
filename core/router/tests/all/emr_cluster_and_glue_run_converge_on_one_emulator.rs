// SPDX-License-Identifier: CC0-1.0

//! Two clock-owed lifecycles IN FLIGHT AT ONCE on one assembled emulator: an Amazon EMR
//! cluster's step/resting-state/log sequence, and an AWS Glue job run's terminal state with the
//! `Glue Job State Change` delivery that follows it — each observed by a client that calls only
//! the service it is waiting on.
//!
//! THE RULE under test: **the work a launched resource has accepted is carried out by the
//! service that owns it, whether or not anybody asks about it — "Amazon EMR executes each step
//! in the order listed" (`AddJobFlowSteps` doc), "the cluster transitions to the `WAITING` state
//! rather than shutting down after the steps have completed" (`RunJobFlow` doc) and
//! `TerminateJobFlows` then takes it to `TERMINATED`; the log files written under `<LogUri>/
//! <cluster-id>/` ("the location in Amazon S3 to write the log files of the job flow",
//! `RunJobFlow$LogUri` doc) are objects of the BUCKET, so they keep answering the same listing
//! once the cluster is gone; and a Glue job run comes to rest in a terminal `JobRunState`
//! (`com.amazonaws.glue#JobRunState`), whose publication EventBridge delivers to each target of
//! a matching rule whose own queue policy admits `events.amazonaws.com` the send. So a client
//! polling ONE of those services reaches every one of those states while the OTHER service's own
//! run is still in flight: neither lifecycle waits on a call of the other to advance it.**
//!
//! What only this suite reaches: BOTH lifecycles owed at once on one emulator, which is the
//! shape a client drives this account in — the sibling suites
//! (`emr_keep_alive_cluster_log_post_mortem.rs`,
//! `glue_job_state_change_delivery_per_queue_policy.rs`) each drive one of them with the other
//! idle, so neither can show that the clock work of one service advances while the other's is
//! outstanding. Every launch and every run below is therefore started FIRST, and only then is
//! any of them observed; each wait polls the service that owns what it waits for and nothing
//! else. The roles, the instance profile, the subnet, the scripts, the queues and the rule are
//! all created through the services that own them, on a router that VERIFIES request signatures.
//!
//! The rows differ along what the rules generalise over: for a cluster, how the launch states
//! its capacity (the uniform instance-type form and an explicit instance group), where its
//! `LogUri` points (a prefix and a nested prefix) and how its step's work ENDED (a clean exit
//! and a non-zero status); for a run, what its script DID (exited cleanly, raised), which
//! decides the terminal state and therefore whether the rule's pattern matches its publication
//! at all. The delivery targets span the two policy forms the recorded case carries: a grant
//! scoped by `aws:SourceArn` to the invoking rule, and a sibling queue with no policy at all.
//!
//! Nothing here waits a fixed time: every phase of a cluster, every step's span, every run's
//! span and both deliveries are stored due instants, so each wait LOOKS AGAIN until the
//! condition holds or the generous [`DEADLINE`] passes, and no assertion reads elapsed time.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on an explicitly sized thread.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use aws_credential_types::Credentials;
use aws_sigv4::http_request::{sign, SignableBody, SignableRequest, SigningSettings};
use aws_sigv4::sign::v4::SigningParams;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, elements, form_encode};

/// Keep this at or above the size `boot_serve.rs` uses: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The Region every call is signed for and addressed in, and the zone this suite's subnet
/// lives in.
const REGION: &str = "us-west-2";
const ZONE: &str = "us-west-2a";

/// The XML namespace Amazon S3's own model declares, which a client's payload carries.
const S3_XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The bucket the scripts and every log file of this suite live in.
const BUCKET: &str = "xsvc-emr-glue-convergence";

/// The roles this suite creates: the one Amazon EMR assumes, the one the cluster's nodes hold
/// through their instance profile, and the one a Glue job run is performed as.
const SERVICE_ROLE: &str = "xsvc-emr-service-role";
const INSTANCE_PROFILE: &str = "xsvc-emr-ec2-role";
const GLUE_ROLE: &str = "xsvc-glue-run-role";

/// The policies AWS publishes for those roles, attached as the recorded scenario attaches them.
const EMR_SERVICE_POLICY: &str = "arn:aws:iam::aws:policy/service-role/AmazonElasticMapReduceRole";
const EMR_EC2_POLICY: &str =
    "arn:aws:iam::aws:policy/service-role/AmazonElasticMapReduceforEC2Role";
const GLUE_SERVICE_ROLE_POLICY: &str = "arn:aws:iam::aws:policy/service-role/AWSGlueServiceRole";

/// The release every launch names, and the instance type its capacity is stated in.
const RELEASE_LABEL: &str = "emr-6.15.0";
const INSTANCE_TYPE: &str = "m5.xlarge";

/// What a completing step prints, which is the whole of its work, and the status the command of
/// a step that gives up exits with. CHOSEN: a status of the command's OWN, so the step's failure
/// cannot be read as the status a session gives a command it could not run at all.
const MARKER: &str = "xsvc-emr-glue-marker";
const GIVING_UP_STATUS: i32 = 4;

/// The `source` and `detail-type` a Glue job run's terminal state is published under, spelled
/// out here rather than imported, so a wrong constant in the implementation cannot make this
/// suite pass.
const SOURCE: &str = "aws.glue";
const DETAIL_TYPE: &str = "Glue Job State Change";

/// The service principal EventBridge delivers to an SQS target as, and the action such a
/// delivery performs on the queue.
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";
const SEND_MESSAGE: &str = "sqs:SendMessage";

/// The terminal states this suite names (`com.amazonaws.glue#JobRunState`,
/// `com.amazonaws.emr#StepState`, `com.amazonaws.emr#ClusterState`), and the cluster states a
/// launch that is still coming up reports.
const SUCCEEDED: &str = "SUCCEEDED";
const FAILED: &str = "FAILED";
const TERMINAL_RUN_STATES: [&str; 6] = [
    "STOPPED",
    "SUCCEEDED",
    "FAILED",
    "TIMEOUT",
    "ERROR",
    "EXPIRED",
];
const TERMINAL_STEP: [&str; 4] = ["COMPLETED", "CANCELLED", "FAILED", "INTERRUPTED"];
const COMING_UP: [&str; 3] = ["STARTING", "BOOTSTRAPPING", "RUNNING"];
const TERMINAL_CLUSTER: [&str; 2] = ["TERMINATED", "TERMINATED_WITH_ERRORS"];

/// How long a look for a clock-owed move keeps looking, and how often it looks. Generous, and
/// only here so a failure is a failure rather than a hang: every wait is on the CONDITION, so a
/// loaded host delays a row instead of breaking it.
const DEADLINE: Duration = Duration::from_secs(180);
const POLL: Duration = Duration::from_millis(100);

/// How long the queue that must receive nothing keeps being looked at once every admitted
/// delivery has arrived. A bound on an ABSENCE, so it is the one span a slow host may spend
/// without changing the verdict.
const WITHHELD_WINDOW: Duration = Duration::from_secs(5);

/// How many times a client re-reads a cluster's log destination once the cluster is gone, with
/// no Amazon EMR call in between. CHOSEN: more than one, so "the files are still there" is a
/// repeated answer rather than a single look.
const PERSISTENCE_REREADS: usize = 4;

/// The key pair this process publishes once request authentication is on, which every call
/// below is signed with.
struct Signer {
    key_id: String,
    secret: String,
}

impl Signer {
    /// Sign `request` for `signing_name` in [`REGION`], in the `Authorization` header an SDK
    /// request carries.
    fn sign(
        &self,
        mut request: http::Request<Vec<u8>>,
        signing_name: &str,
    ) -> http::Request<Vec<u8>> {
        let identity = Credentials::new(&self.key_id, &self.secret, None, None, "test").into();
        let params = SigningParams::builder()
            .identity(&identity)
            .region(REGION)
            .name(signing_name)
            .time(SystemTime::now())
            .settings(SigningSettings::default())
            .build()
            .expect("the signing parameters are complete")
            .into();
        let (method, uri) = (request.method().to_string(), request.uri().to_string());
        let headers: Vec<(String, String)> = request
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_string(),
                    value.to_str().expect("a text header").to_string(),
                )
            })
            .collect();
        let signable = SignableRequest::new(
            &method,
            &uri,
            headers
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str())),
            SignableBody::Bytes(request.body()),
        )
        .expect("the request is signable");
        let (instructions, _) = sign(signable, &params)
            .expect("the request signs")
            .into_parts();
        instructions.apply_to_request_http1x(&mut request);
        request
    }
}

/// What one call answered.
struct Answer {
    status: u16,
    body: String,
}

impl Answer {
    /// The body as JSON, or `Value::Null` for one that is not.
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

/// The emulator this suite drives: the whole router over one engine, serving as it serves
/// traffic — with request authentication ON.
struct Emulator {
    router: MultiServiceRouter,
    signer: Signer,
}

impl Emulator {
    fn start() -> Self {
        let engine = aws_emulator_router::new_engine();
        let mut router = aws_emulator_router::build_router(Arc::clone(&engine));
        router.set_request_authentication(true);
        let published = ::emulator_server::SigV4Authenticator::minted_credentials()
            .expect("turning authentication on publishes this process's keys");
        Self {
            signer: Signer {
                key_id: published.access_key_id.clone(),
                secret: published.secret_access_key.clone(),
            },
            router,
        }
    }

    /// Dispatch one signed request as it arrives on the wire.
    async fn send(
        &self,
        method: &str,
        host: &str,
        signing: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> Answer {
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let built = builder
            .body(body.as_bytes().to_vec())
            .expect("a well-formed request");
        let (parts, payload) = self.signer.sign(built, signing).into_parts();
        let response = self
            .router
            .dispatch(http::Request::from_parts(
                parts,
                bytes::Bytes::from(payload),
            ))
            .await;
        Answer {
            status: response.status().as_u16(),
            body: String::from_utf8_lossy(response.body()).into_owned(),
        }
    }

    /// One Amazon S3 REST call, path-style as the emulator serves it, which must be served.
    async fn s3(&self, what: &str, method: &str, path: &str, body: &str) -> Answer {
        let host = format!("s3.{REGION}.amazonaws.com");
        let answered = self.send(method, &host, "s3", path, &[], body).await;
        assert!(
            (200..300).contains(&answered.status),
            "case {what:?}: {method} {path} must be served, got {} with {}",
            answered.status,
            answered.body
        );
        answered
    }

    /// One IAM `awsQuery` call, which must be served.
    async fn iam(&self, what: &str, action: &str, params: &[(&str, &str)]) -> Answer {
        self.query("iam.amazonaws.com", "iam", what, action, "2010-05-08", params)
            .await
    }

    /// One Amazon EC2 `ec2Query` call, which must be served.
    async fn ec2(&self, what: &str, action: &str, params: &[(&str, &str)]) -> Answer {
        let host = format!("ec2.{REGION}.amazonaws.com");
        self.query(&host, "ec2", what, action, "2016-11-15", params)
            .await
    }

    /// One form-encoded call of either of those two protocols.
    async fn query(
        &self,
        host: &str,
        signing: &str,
        what: &str,
        action: &str,
        version: &str,
        params: &[(&str, &str)],
    ) -> Answer {
        let mut body = format!("Action={action}&Version={version}");
        for (name, value) in params {
            body.push('&');
            body.push_str(&form_encode(name));
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let answered = self
            .send(
                "POST",
                host,
                signing,
                "/",
                &[("content-type", "application/x-www-form-urlencoded")],
                &body,
            )
            .await;
        assert_eq!(
            answered.status, 200,
            "case {what:?}: {action} must answer 200, got {} with {}",
            answered.status, answered.body
        );
        answered
    }

    /// One `X-Amz-Target` RPC call, which must answer 200.
    async fn rpc(
        &self,
        what: &str,
        signing: &str,
        content_type: &str,
        target: &str,
        body: &Value,
    ) -> Value {
        let host = format!("{signing}.{REGION}.amazonaws.com");
        let answered = self
            .send(
                "POST",
                &host,
                signing,
                "/",
                &[("content-type", content_type), ("x-amz-target", target)],
                &body.to_string(),
            )
            .await;
        assert_eq!(
            answered.status, 200,
            "case {what:?}: {target} must answer 200, got {} with {}",
            answered.status, answered.body
        );
        answered.json()
    }

    /// One Amazon EMR `awsJson1_1` call.
    async fn emr(&self, what: &str, operation: &str, body: &Value) -> Value {
        self.rpc(
            what,
            "elasticmapreduce",
            "application/x-amz-json-1.1",
            &format!("ElasticMapReduce.{operation}"),
            body,
        )
        .await
    }

    /// One AWS Glue `awsJson1_1` call.
    async fn glue(&self, what: &str, operation: &str, body: &Value) -> Value {
        self.rpc(
            what,
            "glue",
            "application/x-amz-json-1.1",
            &format!("AWSGlue.{operation}"),
            body,
        )
        .await
    }

    /// One EventBridge `awsJson1_1` call.
    async fn events(&self, what: &str, operation: &str, body: &Value) -> Value {
        self.rpc(
            what,
            "events",
            "application/x-amz-json-1.1",
            &format!("AWSEvents.{operation}"),
            body,
        )
        .await
    }

    /// One Amazon SQS `awsJson1_0` call.
    async fn sqs(&self, what: &str, operation: &str, body: &Value) -> Value {
        self.rpc(
            what,
            "sqs",
            "application/x-amz-json-1.0",
            &format!("AmazonSQS.{operation}"),
            body,
        )
        .await
    }

    /// A queue called `name`: its url and the ARN a rule names it by.
    async fn given_queue(&self, what: &str, name: &str) -> (String, String) {
        let created = self
            .sqs(what, "CreateQueue", &json!({ "QueueName": name }))
            .await;
        let url = created["QueueUrl"]
            .as_str()
            .unwrap_or_else(|| panic!("case {what:?}: expected a QueueUrl in {created}"))
            .to_string();
        let attributes = self
            .sqs(
                what,
                "GetQueueAttributes",
                &json!({ "QueueUrl": &url, "AttributeNames": ["QueueArn"] }),
            )
            .await;
        let arn = attributes["Attributes"]["QueueArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {what:?}: expected a QueueArn in {attributes}"))
            .to_string();
        (url, arn)
    }

    /// Receive everything waiting on the queue at `url` and delete it; answers the envelopes
    /// its message bodies carried.
    ///
    /// Each message received is DELETED, so a message this answered with cannot come back when
    /// its visibility timeout lapses and be read as a second delivery of the same event.
    async fn drain(&self, what: &str, url: &str) -> Vec<Value> {
        let received = self
            .sqs(
                what,
                "ReceiveMessage",
                &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
            )
            .await;
        let messages = received["Messages"].as_array().cloned().unwrap_or_default();
        let mut envelopes = Vec::with_capacity(messages.len());
        for message in &messages {
            if let Some(handle) = message["ReceiptHandle"].as_str() {
                self.sqs(
                    what,
                    "DeleteMessage",
                    &json!({ "QueueUrl": url, "ReceiptHandle": handle }),
                )
                .await;
            }
            envelopes.push(
                serde_json::from_str::<Value>(message["Body"].as_str().unwrap_or_default())
                    .unwrap_or(Value::Null),
            );
        }
        envelopes
    }

    /// The keys of every object under `prefix`, from Amazon S3's own `ListObjectsV2`.
    async fn keys_under(&self, what: &str, prefix: &str) -> BTreeSet<String> {
        let listed = self
            .s3(
                what,
                "GET",
                &format!("/{BUCKET}?list-type=2&prefix={prefix}"),
                "",
            )
            .await;
        elements(&listed.body, "Key").into_iter().collect()
    }
}

/// A trust policy admitting `principal` and nothing else.
fn trust_policy(principal: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": principal },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// How a launch states the capacity of its one primary node.
#[derive(Debug, Clone, Copy)]
enum Capacity {
    /// The uniform form a command-line client's `--instance-type`/`--instance-count` writes.
    Uniform,
    /// One explicit instance group.
    Group,
}

/// What a step's command DID, which is what decides the state its step ends in.
#[derive(Debug, Clone, Copy)]
enum Work {
    /// Printed a line and exited zero.
    Echo,
    /// Exited with a status of its own.
    GiveUp,
}

/// One cluster row: how its launch is stated, where its logs go, what its step did and the
/// `StepState` that step must end in.
struct ClusterRow {
    case: &'static str,
    capacity: Capacity,
    prefix: &'static str,
    work: Work,
    ends: &'static str,
}

/// One run row: what its script did, the `JobRunState` it must come to rest in, and whether its
/// publication matches the rule — and so whether the admitted target receives it.
struct RunRow {
    case: &'static str,
    script: &'static str,
    rests_in: &'static str,
    delivered: bool,
}

/// The cluster rows, spanning the launch form, the destination form and how the step ended.
fn cluster_rows() -> Vec<ClusterRow> {
    vec![
        ClusterRow {
            case: "a uniform-capacity cluster whose step exits zero",
            capacity: Capacity::Uniform,
            prefix: "logs",
            work: Work::Echo,
            ends: "COMPLETED",
        },
        ClusterRow {
            case: "an instance-group cluster logging under a nested prefix",
            capacity: Capacity::Group,
            prefix: "nested/logs",
            work: Work::Echo,
            ends: "COMPLETED",
        },
        ClusterRow {
            case: "a uniform-capacity cluster whose step exits non-zero",
            capacity: Capacity::Uniform,
            prefix: "logs",
            work: Work::GiveUp,
            ends: FAILED,
        },
    ]
}

/// The run rows: a script that exits cleanly, and one that raises — the terminal state the rule's
/// pattern matches, and one it does not.
fn run_rows() -> Vec<RunRow> {
    vec![
        RunRow {
            case: "a pythonshell script that exits cleanly",
            script: "print(\"ran\")\n",
            rests_in: SUCCEEDED,
            delivered: true,
        },
        RunRow {
            case: "a pythonshell script that raises",
            script: "raise ValueError(\"the input partition is malformed\")\n",
            rests_in: FAILED,
            delivered: false,
        },
    ]
}

/// The command line one row's step submits, in the `command-runner.jar` form a command-line
/// client's `--steps` writes.
fn command_of(work: Work, ordinal: usize) -> Vec<String> {
    let program = match work {
        Work::Echo => format!("echo {MARKER}-{ordinal}"),
        Work::GiveUp => format!("exit {GIVING_UP_STATUS}"),
    };
    vec!["bash".to_string(), "-c".to_string(), program]
}

/// RULE: every state these two lifecycles owe a client is reached while the other lifecycle is
/// still in flight on the same emulator, and each is reached through the service that owns it.
#[test]
fn an_emr_cluster_and_a_glue_run_both_converge_on_one_emulator() {
    std::thread::Builder::new()
        .name("emr-glue-convergence".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .worker_threads(2)
                .build()
                .expect("a multi-thread runtime");
            runtime.block_on(drive());
        })
        .expect("spawn the sized thread")
        .join()
        .expect("the suite thread runs to completion");
}

/// Every row, end to end.
async fn drive() {
    let emulator = Emulator::start();
    let clusters = cluster_rows();
    let runs = run_rows();

    // (0) The account the rows are driven in: the bucket, the two scripts, the three roles, the
    // instance profile the launches name, the subnet they are placed in, the two target queues
    // and the rule — each through the service that owns it.
    emulator
        .s3(
            "the bucket",
            "PUT",
            &format!("/{BUCKET}"),
            &format!(
                "<CreateBucketConfiguration xmlns=\"{S3_XMLNS}\">\
                 <LocationConstraint>{REGION}</LocationConstraint>\
                 </CreateBucketConfiguration>"
            ),
        )
        .await;
    for (index, row) in runs.iter().enumerate() {
        emulator
            .s3(
                row.case,
                "PUT",
                &format!("/{BUCKET}/scripts/job_{index}.py"),
                row.script,
            )
            .await;
    }

    for (role, principal, policy_arn) in [
        (
            SERVICE_ROLE,
            "elasticmapreduce.amazonaws.com",
            EMR_SERVICE_POLICY,
        ),
        (INSTANCE_PROFILE, "ec2.amazonaws.com", EMR_EC2_POLICY),
        (GLUE_ROLE, "glue.amazonaws.com", GLUE_SERVICE_ROLE_POLICY),
    ] {
        emulator
            .iam(
                role,
                "CreateRole",
                &[
                    ("RoleName", role),
                    ("AssumeRolePolicyDocument", &trust_policy(principal)),
                ],
            )
            .await;
        emulator
            .iam(
                role,
                "AttachRolePolicy",
                &[("RoleName", role), ("PolicyArn", policy_arn)],
            )
            .await;
    }
    // The grants a Glue job run needs to read its script, and the grant a cluster's nodes need
    // to push their log files, stated inline beside the attachments above.
    let object_access = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": ["s3:GetObject", "s3:PutObject", "s3:ListBucket"],
            "Resource": [
                format!("arn:aws:s3:::{BUCKET}"),
                format!("arn:aws:s3:::{BUCKET}/*"),
            ],
        }],
    })
    .to_string();
    for role in [INSTANCE_PROFILE, GLUE_ROLE] {
        emulator
            .iam(
                role,
                "PutRolePolicy",
                &[
                    ("RoleName", role),
                    ("PolicyName", "read-and-write-the-bucket"),
                    ("PolicyDocument", &object_access),
                ],
            )
            .await;
    }
    emulator
        .iam(
            INSTANCE_PROFILE,
            "CreateInstanceProfile",
            &[("InstanceProfileName", INSTANCE_PROFILE)],
        )
        .await;
    emulator
        .iam(
            INSTANCE_PROFILE,
            "AddRoleToInstanceProfile",
            &[
                ("InstanceProfileName", INSTANCE_PROFILE),
                ("RoleName", INSTANCE_PROFILE),
            ],
        )
        .await;
    let glue_role_arn = {
        let read = emulator
            .iam(GLUE_ROLE, "GetRole", &[("RoleName", GLUE_ROLE)])
            .await;
        element(&read.body, "Arn")
    };

    let network = emulator
        .ec2("the network", "CreateVpc", &[("CidrBlock", "10.0.0.0/16")])
        .await;
    let vpc = element(&network.body, "vpcId");
    let placed = emulator
        .ec2(
            "the network",
            "CreateSubnet",
            &[
                ("VpcId", &vpc),
                ("CidrBlock", "10.0.1.0/24"),
                ("AvailabilityZone", ZONE),
            ],
        )
        .await;
    let subnet = element(&placed.body, "subnetId");

    let (admitted_url, admitted_arn) = emulator
        .given_queue("the admitted target", "xsvc-emr-glue-ok")
        .await;
    let (withheld_url, withheld_arn) = emulator
        .given_queue("the withheld target", "xsvc-emr-glue-broken")
        .await;
    let job_names: Vec<String> = (0..runs.len())
        .map(|index| format!("xsvc_glue_job_{index}"))
        .collect();
    let rule_name = "xsvc-glue-job-state-change";
    let rule = emulator
        .events(
            "the rule",
            "PutRule",
            &json!({
                "Name": rule_name,
                "State": "ENABLED",
                "EventPattern": json!({
                    "source": [SOURCE],
                    "detail-type": [DETAIL_TYPE],
                    "detail": { "jobName": &job_names, "state": [SUCCEEDED] },
                })
                .to_string(),
            }),
        )
        .await;
    let rule_arn = rule["RuleArn"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: PutRule must answer the rule's ARN, got {rule}"))
        .to_string();
    // The admitted target grants the events principal the send for THIS rule, conditioned on
    // "the ARN of the rule" (`aws:SourceArn`); the withheld one carries no policy at all, which
    // is the one thing that differs between them.
    emulator
        .sqs(
            "the admitted target",
            "SetQueueAttributes",
            &json!({
                "QueueUrl": &admitted_url,
                "Attributes": {
                    "Policy": json!({
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": { "Service": EVENTS_PRINCIPAL },
                            "Action": SEND_MESSAGE,
                            "Resource": &admitted_arn,
                            "Condition": { "ArnEquals": { "aws:SourceArn": &rule_arn } },
                        }],
                    })
                    .to_string(),
                },
            }),
        )
        .await;
    let targeted = emulator
        .events(
            "the targets",
            "PutTargets",
            &json!({
                "Rule": rule_name,
                "Targets": [
                    { "Id": "ok", "Arn": &admitted_arn },
                    { "Id": "broken", "Arn": &withheld_arn },
                ],
            }),
        )
        .await;
    assert_eq!(
        targeted["FailedEntryCount"],
        json!(0),
        "setup: both queue targets must be accepted, got {targeted}"
    );
    for (index, row) in runs.iter().enumerate() {
        emulator
            .glue(
                row.case,
                "CreateJob",
                &json!({
                    "Name": &job_names[index],
                    "Role": &glue_role_arn,
                    "Command": {
                        "Name": "pythonshell",
                        "PythonVersion": "3.9",
                        "ScriptLocation": format!("s3://{BUCKET}/scripts/job_{index}.py"),
                    },
                    "MaxCapacity": 0.0625,
                    "Timeout": 5,
                }),
            )
            .await;
    }

    // (1) Everything is started BEFORE anything is observed, so every row's clock-owed work is
    // outstanding at the same time and no row's waiting stands in front of another's start.
    let mut launched: Vec<String> = Vec::with_capacity(clusters.len());
    for (ordinal, row) in clusters.iter().enumerate() {
        launched.push(launch(&emulator, row, ordinal, &subnet).await);
    }
    let mut started: Vec<String> = Vec::with_capacity(runs.len());
    for (index, row) in runs.iter().enumerate() {
        let answered = emulator
            .glue(
                row.case,
                "StartJobRun",
                &json!({ "JobName": &job_names[index] }),
            )
            .await;
        started.push(
            answered["JobRunId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: StartJobRun must answer a JobRunId, got {answered}",
                        row.case
                    )
                })
                .to_string(),
        );
    }

    // (2) Each run comes to rest in the state its own script answered, read through Glue alone.
    for (index, row) in runs.iter().enumerate() {
        let case = row.case;
        let run = run_at_rest(&emulator, case, &job_names[index], &started[index]).await;
        assert_eq!(
            run["JobRunState"].as_str(),
            Some(row.rests_in),
            "case {case:?}: expected the run to come to rest in {}, got {run}",
            row.rests_in
        );
        let reported = run["ErrorMessage"].as_str().unwrap_or_default();
        assert_eq!(
            !reported.is_empty(),
            row.rests_in == FAILED,
            "case {case:?}: a run that failed reports why and one that succeeded reports \
             nothing, got {run}"
        );
    }

    // (3) Each cluster's step ends where its own work took it, the kept-alive cluster then rests
    // in WAITING, its log files are under its own `LogUri` — and they are all still there, read
    // after read, once the caller has shut the cluster down and is asking Amazon EMR nothing
    // more.
    for (ordinal, (row, cluster_id)) in clusters.iter().zip(&launched).enumerate() {
        let case = row.case;
        let prefix = format!("{}/{cluster_id}/", row.prefix);
        let step = settled_step(&emulator, case, cluster_id).await;
        assert_eq!(
            step["Status"]["State"].as_str(),
            Some(row.ends),
            "case {case:?}: expected the step to end {}, got {step}",
            row.ends
        );
        let config = step["Config"]["Args"].as_array().cloned().unwrap_or_default();
        let submitted: Vec<String> = command_of(row.work, ordinal);
        assert_eq!(
            config,
            submitted
                .iter()
                .map(|word| json!(word))
                .collect::<Vec<Value>>(),
            "case {case:?}: the step reports the command line it was submitted with, got {step}"
        );

        let resting = awaited_cluster_state(&emulator, case, cluster_id, "WAITING", |state| {
            !COMING_UP.contains(&state)
        })
        .await;
        assert_eq!(
            resting, "WAITING",
            "case {case:?}: expected the kept-alive cluster to rest in WAITING, got {resting:?}"
        );

        let delivered = awaited_keys(&emulator, case, &prefix).await;
        assert!(
            !delivered.is_empty(),
            "case {case:?}: expected the cluster's log files under {prefix}, got none"
        );
        assert!(
            delivered.iter().all(|key| key.starts_with(&prefix)),
            "case {case:?}: every log file belongs under {prefix}, got {delivered:?}"
        );

        emulator
            .emr(
                case,
                "TerminateJobFlows",
                &json!({ "JobFlowIds": [cluster_id] }),
            )
            .await;
        let ended = awaited_cluster_state(&emulator, case, cluster_id, "TERMINATED", |state| {
            TERMINAL_CLUSTER.contains(&state)
        })
        .await;
        assert_eq!(
            ended, "TERMINATED",
            "case {case:?}: expected the cluster the caller shut down to end TERMINATED, got \
             {ended:?}"
        );
        // The files belong to the BUCKET and not to the cluster, so every re-read made with no
        // Amazon EMR call in between still answers every file the read before it answered — a
        // listing that grows is a delivery the step was still owed, one that loses a file is the
        // defect this is about.
        let mut seen = delivered.clone();
        for round in 0..PERSISTENCE_REREADS {
            let after = emulator.keys_under(case, &prefix).await;
            assert!(
                seen.is_subset(&after),
                "case {case:?}: re-read {round} after termination must still answer every file \
                 it answered before, expected at least {seen:?}, got {after:?}"
            );
            assert!(
                after.iter().all(|key| key.starts_with(&prefix)),
                "case {case:?}: every log file belongs under {prefix}, got {after:?}"
            );
            seen = after;
        }
        assert!(
            !seen.is_empty(),
            "case {case:?}: the terminated cluster's log files are still listed under {prefix}"
        );
    }

    // (4) The publication of every run the rule matches reached the target whose own policy
    // admits the send, and the sibling target with no policy received nothing — watched for a
    // further window once every admitted delivery had arrived.
    let mut admitted: Vec<Value> = Vec::new();
    let mut withheld: Vec<Value> = Vec::new();
    let expected = runs.iter().filter(|row| row.delivered).count();
    let deadline = Instant::now() + DEADLINE;
    while admitted.len() < expected {
        admitted.extend(emulator.drain("the admitted target", &admitted_url).await);
        withheld.extend(emulator.drain("the withheld target", &withheld_url).await);
        assert!(
            Instant::now() < deadline,
            "expected {expected} delivered event(s) on the admitted target, got {admitted:?}"
        );
        tokio::time::sleep(POLL).await;
    }
    let watching = Instant::now();
    while watching.elapsed() < WITHHELD_WINDOW {
        admitted.extend(emulator.drain("the admitted target", &admitted_url).await);
        withheld.extend(emulator.drain("the withheld target", &withheld_url).await);
        tokio::time::sleep(POLL).await;
    }
    assert!(
        withheld.is_empty(),
        "a target queue with no resource policy receives nothing, got {withheld:?}"
    );
    assert_eq!(
        admitted.len(),
        expected,
        "expected exactly {expected} delivered event(s), got {admitted:?}"
    );
    for (index, row) in runs.iter().enumerate() {
        let case = row.case;
        let found = admitted.iter().any(|envelope| {
            envelope.pointer("/detail/jobName") == Some(&json!(&job_names[index]))
        });
        assert_eq!(
            found, row.delivered,
            "case {case:?}: expected delivered={}, got {admitted:?}",
            row.delivered
        );
        if !row.delivered {
            continue;
        }
        let envelope = admitted
            .iter()
            .find(|envelope| {
                envelope.pointer("/detail/jobName") == Some(&json!(&job_names[index]))
            })
            .expect("the delivered envelope just found");
        for (pointer, wanted) in [
            ("/source", json!(SOURCE)),
            ("/detail-type", json!(DETAIL_TYPE)),
            ("/region", json!(REGION)),
            ("/detail/jobRunId", json!(&started[index])),
            ("/detail/state", json!(row.rests_in)),
        ] {
            let actual = envelope.pointer(pointer).cloned().unwrap_or(Value::Null);
            assert_eq!(
                actual, wanted,
                "case {case:?}: expected {pointer} to be {wanted}, got {actual} in {envelope}"
            );
        }
    }
}

/// Launch one row's cluster, with the step and the `LogUri` the rule is about, and answer its id.
async fn launch(
    emulator: &Emulator,
    row: &ClusterRow,
    ordinal: usize,
    subnet: &str,
) -> String {
    let mut instances = json!({
        // "If the JobFlowInstancesConfig KeepJobFlowAliveWhenNoSteps parameter is set to TRUE,
        // the cluster transitions to the WAITING state" (`RunJobFlow` doc).
        "KeepJobFlowAliveWhenNoSteps": true,
        "Ec2SubnetId": subnet,
    });
    match row.capacity {
        Capacity::Uniform => {
            instances["InstanceCount"] = json!(1);
            instances["MasterInstanceType"] = json!(INSTANCE_TYPE);
        }
        Capacity::Group => {
            instances["InstanceGroups"] = json!([{
                "Name": "Primary",
                "InstanceRole": "MASTER",
                "InstanceType": INSTANCE_TYPE,
                "InstanceCount": 1,
            }]);
        }
    }
    let answered = emulator
        .emr(
            row.case,
            "RunJobFlow",
            &json!({
                "Name": row.case,
                "ReleaseLabel": RELEASE_LABEL,
                "JobFlowRole": INSTANCE_PROFILE,
                "ServiceRole": SERVICE_ROLE,
                "Applications": [{ "Name": "Hadoop" }],
                "LogUri": format!("s3://{BUCKET}/{}/", row.prefix),
                "VisibleToAllUsers": true,
                "Steps": [{
                    "Name": row.case,
                    // "CONTINUE — Continues to the next step in the queue"
                    // (`StepConfig$ActionOnFailure` doc): a step that fails neither cancels the
                    // queue behind it nor takes the cluster down.
                    "ActionOnFailure": "CONTINUE",
                    "HadoopJarStep": {
                        "Jar": "command-runner.jar",
                        "Args": command_of(row.work, ordinal),
                    },
                }],
                "Instances": instances,
            }),
        )
        .await;
    answered["JobFlowId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {:?}: RunJobFlow must answer a JobFlowId", row.case))
        .to_string()
}

/// Poll `ListSteps` until the cluster's one step is terminal, and answer it.
async fn settled_step(emulator: &Emulator, case: &str, cluster_id: &str) -> Value {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let listed = emulator
            .emr(case, "ListSteps", &json!({ "ClusterId": cluster_id }))
            .await;
        let steps = listed["Steps"].as_array().cloned().unwrap_or_default();
        if let Some(step) = steps.first() {
            let state = step["Status"]["State"].as_str().unwrap_or_default();
            if TERMINAL_STEP.contains(&state) {
                return step.clone();
            }
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the submitted step to reach a terminal state, got {listed}"
        );
        tokio::time::sleep(POLL).await;
    }
}

/// Poll `DescribeCluster` until the cluster's state satisfies `settled`, and answer that state.
async fn awaited_cluster_state<F: Fn(&str) -> bool>(
    emulator: &Emulator,
    case: &str,
    cluster_id: &str,
    wanted: &str,
    settled: F,
) -> String {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let described = emulator
            .emr(case, "DescribeCluster", &json!({ "ClusterId": cluster_id }))
            .await;
        let state = described["Cluster"]["Status"]["State"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if settled(&state) {
            return state;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the cluster to reach {wanted}, got {state:?}"
        );
        tokio::time::sleep(POLL).await;
    }
}

/// Poll Amazon S3 until `prefix` holds at least one object, and answer every key it holds then.
async fn awaited_keys(emulator: &Emulator, case: &str, prefix: &str) -> BTreeSet<String> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let keys = emulator.keys_under(case, prefix).await;
        if !keys.is_empty() {
            return keys;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the cluster's log files under {prefix}, got none"
        );
        tokio::time::sleep(POLL).await;
    }
}

/// Poll `GetJobRun` until the run has come to rest, exactly as a client waits for one, and
/// answer the `JobRun` it then reports.
async fn run_at_rest(emulator: &Emulator, case: &str, job: &str, run_id: &str) -> Value {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let reported = emulator
            .glue(
                case,
                "GetJobRun",
                &json!({ "JobName": job, "RunId": run_id }),
            )
            .await;
        let run = reported["JobRun"].clone();
        let state = run["JobRunState"].as_str().unwrap_or_default().to_string();
        if TERMINAL_RUN_STATES.contains(&state.as_str()) {
            return run;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the run never came to rest; it reports {run}"
        );
        tokio::time::sleep(POLL).await;
    }
}
