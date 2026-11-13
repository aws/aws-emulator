// SPDX-License-Identifier: CC0-1.0

//! The EMR job-flow (cluster) lifecycle, end to end over the wire.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the
//! router's `dispatch`, so these are the same bytes an SDK client sends. Nothing
//! reaches into the service's state struct: a cluster or step is only ever read back
//! through `DescribeCluster`, `ListClusters`, `DescribeStep` or `ListSteps`, which is
//! what makes these tests evidence that the responses are computed from persisted
//! state.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_emr_elasticmapreduce::state::{CLUSTER_PHASE_SPAN, MAX_CLUSTERS};
use service_com_amazonaws_emr_elasticmapreduce::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Longer than the service's clock-owed transition span (500 ms), so a cluster that was
/// `TERMINATING`, or a step that was `PENDING` on a live cluster, has reached its next
/// state by the time we look.
///
/// `pub(crate)` for the reason [`PAST_CLUSTER_READY`] is.
pub(crate) const PAST_TRANSITION: Duration = Duration::from_millis(900);

/// Longer than the TWO phases a new cluster comes up over — one
/// [`CLUSTER_PHASE_SPAN`] in `STARTING` and one in `BOOTSTRAPPING` — so a cluster has
/// reached its ready state by the time we look.
///
/// `pub(crate)` so a sibling suite waits out the same span through one constant rather
/// than its own copy.
pub(crate) const PAST_CLUSTER_READY: Duration = CLUSTER_PHASE_SPAN
    .saturating_mul(2)
    .saturating_add(PAST_TRANSITION);

/// How long a wait for a cluster to reach a state the clock owes it may take before the
/// case gives up: far above every span this service owes a cluster, so a slow or loaded host
/// delays a wait rather than failing it ([`await_cluster_where`]).
const CLUSTER_WAIT_DEADLINE: Duration = Duration::from_secs(90);

/// One look at the cluster inside [`CLUSTER_WAIT_DEADLINE`].
const CLUSTER_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// `ListClusters` / `ListSteps` page size, documented on both operations ("a maximum of
/// 50 clusters … per call", "a list greater than 50 steps").
const PAGE_SIZE: usize = 50;

/// `com.amazonaws.emr#ActionOnFailure`, every value the model declares
/// (`smithy_query(query="shape", shape="ActionOnFailure")`).
const ACTION_ON_FAILURE: [&str; 4] = [
    "TERMINATE_JOB_FLOW",
    "TERMINATE_CLUSTER",
    "CANCEL_AND_WAIT",
    "CONTINUE",
];

static NULL: Value = Value::Null;

/// The EC2 instance profile every `RunJobFlow` below names as its `JobFlowRole`.
///
/// `RunJobFlow` requires one — "also called instance profile and Amazon EC2 role. An IAM
/// role for an Amazon EMR cluster. The Amazon EC2 instances of the cluster assume this
/// role" (`RunJobFlow$JobFlowRole` doc) — so a launch that is meant to SUCCEED names it.
/// This suite carries only EMR, so there is no IAM here to resolve it against; the
/// IAM-grounded half of that rule lives in
/// `core/router/tests/all/emr_job_flow_role_instance_profile.rs`.
pub(crate) const INSTANCE_PROFILE: &str = "EMR_EC2_DefaultRole";

/// The service role every `RunJobFlow` below names, which the launch requires too — "the
/// IAM role that Amazon EMR assumes in order to access Amazon Web Services resources on
/// your behalf" (`RunJobFlow$ServiceRole` doc).
pub(crate) const SERVICE_ROLE: &str = "EMR_DefaultRole";

/// The access key id the harness's requests PRESENT by default, so a suite that reads
/// this service's audit stream can attribute a call to it. Request authentication is off
/// in these tests; this is the identity a request carries, not a verified one.
pub(crate) const PRESENTED_AKID: &str = "AKIDEMRTESTHARNESS";

/// The request target every harness call names, as an audit record reports it.
pub(crate) const REQUEST_TARGET: &str = "POST /";

/// The Region every request below is signed for unless a suite picks another
/// ([`Emr::with_region`]).
pub(crate) const DEFAULT_REGION: &str = "us-east-1";

/// One emulator: a router carrying only this service, over one shared engine.
///
/// `pub(crate)` so every suite in this test target drives the emulator through the
/// same wire harness rather than growing its own copy.
pub(crate) struct Emr {
    router: MultiServiceRouter,
    akid: &'static str,
    region: &'static str,
}

impl Emr {
    pub(crate) fn new() -> Self {
        Self::with_akid(PRESENTED_AKID)
    }

    /// An emulator whose requests present `akid` as their SigV4 access key id, for a
    /// suite that has to tell its own calls apart from another's.
    pub(crate) fn with_akid(akid: &'static str) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self {
            router,
            akid,
            region: DEFAULT_REGION,
        }
    }

    /// An emulator whose requests are signed for `region` — the credential scope is what
    /// names the Region a request is addressed to, so a suite asserting something
    /// Region-dependent picks it here rather than building its own requests.
    pub(crate) fn with_region(region: &'static str) -> Self {
        Self {
            region,
            ..Self::new()
        }
    }

    pub(crate) async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        self.call_as(Some(self.akid), operation, body).await
    }

    /// [`Emr::call`], signed by `akid` when one is given — the access key id is what
    /// names the calling principal, so what an operation records as its caller follows
    /// it.
    pub(crate) async fn call_as(
        &self,
        akid: Option<&str>,
        operation: &str,
        body: Value,
    ) -> (u16, Value) {
        let mut builder = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("elasticmapreduce.{}.amazonaws.com", self.region))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("ElasticMapReduce.{operation}"));
        // `None` is an UNSIGNED request — the emulator attributes it to the `default`
        // user — while `Some` overrides this emulator's own [`Emr::with_akid`] key.
        if let Some(akid) = akid {
            builder = builder.header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={akid}/20261022/{}/\
                     elasticmapreduce/aws4_request, SignedHeaders=host, Signature=00",
                    self.region
                ),
            );
        }
        let request = builder
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, payload)
    }

    pub(crate) async fn ok(&self, operation: &str, body: Value) -> Value {
        let (status, payload) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation} must answer 200, got {status} with {payload}"
        );
        payload
    }

    /// `RunJobFlow`, answering the minted cluster id.
    pub(crate) async fn run_job_flow(&self, request: Value) -> String {
        let out = self.ok("RunJobFlow", request).await;
        out["JobFlowId"]
            .as_str()
            .unwrap_or_else(|| panic!("RunJobFlow must answer a JobFlowId, got {out}"))
            .to_string()
    }

    pub(crate) async fn describe_cluster(&self, cluster_id: &str) -> Value {
        self.ok("DescribeCluster", json!({ "ClusterId": cluster_id }))
            .await["Cluster"]
            .clone()
    }

    pub(crate) async fn list_steps(&self, request: Value) -> Value {
        self.ok("ListSteps", request).await
    }
}

/// Walk a JSON path, answering `null` for anything absent.
pub(crate) fn at<'a>(value: &'a Value, path: &[&str]) -> &'a Value {
    let mut cursor = value;
    for key in path {
        cursor = cursor.get(*key).unwrap_or(&NULL);
    }
    cursor
}

pub(crate) fn epoch_secs(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

pub(crate) fn assert_refused(case: &str, status: u16, payload: &Value, expected_code: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {payload}"
    );
    let code = at(payload, &["__type"]).as_str().unwrap_or_default();
    assert!(
        code.contains(expected_code),
        "case {case:?}: expected error code {expected_code}, got {code:?} in {payload}"
    );
}

/// An EMR-minted id: the documented prefix plus uppercase alphanumerics.
pub(crate) fn assert_minted_id(case: &str, prefix: &str, id: &str) {
    let body = id
        .strip_prefix(prefix)
        .unwrap_or_else(|| panic!("case {case:?}: id {id:?} must start with {prefix:?}"));
    assert!(
        !body.is_empty() && body.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()),
        "case {case:?}: id {id:?} must be {prefix:?} plus uppercase alphanumerics"
    );
}

pub(crate) fn step_config(name: &str, jar: &str, args: Value, extra: Value) -> Value {
    let mut hadoop = json!({ "Jar": jar });
    if !args.is_null() {
        hadoop["Args"] = args;
    }
    if let Some(members) = extra.as_object() {
        for (key, value) in members {
            hadoop[key] = value.clone();
        }
    }
    json!({ "Name": name, "HadoopJarStep": hadoop })
}

/// The smallest request that satisfies `RunJobFlow`'s required members.
pub(crate) fn minimal_request(name: &str) -> Value {
    json!({
        "Name": name,
        "JobFlowRole": INSTANCE_PROFILE, "ServiceRole": SERVICE_ROLE,
        "Instances": { "InstanceCount": 1, "MasterInstanceType": "m5.xlarge" },
    })
}

/// A cluster that stays up when it has no steps to run.
pub(crate) fn kept_alive_request(name: &str) -> Value {
    json!({
        "Name": name,
        "JobFlowRole": INSTANCE_PROFILE, "ServiceRole": SERVICE_ROLE,
        "Instances": {
            "InstanceCount": 2,
            "MasterInstanceType": "m5.xlarge",
            "SlaveInstanceType": "m5.xlarge",
            "KeepJobFlowAliveWhenNoSteps": true,
        },
    })
}

// ---------------------------------------------------------------------------
// RunJobFlow -> DescribeCluster
// ---------------------------------------------------------------------------

/// THE RULE: `RunJobFlow` persists a cluster built from the request and answers its
/// minted id plus that cluster's ARN; `DescribeCluster` answers the SAME cluster back.
/// The table spans the request shapes the operation accepts — instance groups vs
/// instance fleets, release label vs AMI version, with and without steps, tags,
/// applications and roles, kept alive vs transient, every `VisibleToAllUsers` form.
#[tokio::test]
async fn run_job_flow_persists_each_cluster_and_describe_cluster_reads_it_back() {
    struct Case {
        label: &'static str,
        request: Value,
        collection_type: &'static str,
        auto_terminate: bool,
        step_concurrency_level: i64,
        step_count: usize,
    }

    let started = epoch_secs(SystemTime::now());
    let emr = Emr::new();

    let cases = vec![
        Case {
            label: "uniform instance group, kept alive, one step",
            request: json!({
                "Name": "demo",
                "ReleaseLabel": "emr-6.15.0",
                "LogUri": "s3://logs/demo/",
                "ServiceRole": "EMR_DefaultRole",
                "JobFlowRole": "EMR_EC2_DefaultRole",
                "Applications": [{ "Name": "Spark" }],
                "Tags": [{ "Key": "env", "Value": "test" }],
                "VisibleToAllUsers": true,
                "Instances": {
                    "InstanceCount": 3,
                    "MasterInstanceType": "m5.xlarge",
                    "SlaveInstanceType": "m5.xlarge",
                    "KeepJobFlowAliveWhenNoSteps": true,
                    "Ec2KeyName": "dev-key",
                    "Ec2SubnetId": "subnet-0a1b2c",
                },
                "Steps": [step_config("count", "s3://jars/count.jar", json!(["10"]), Value::Null)],
            }),
            collection_type: "INSTANCE_GROUP",
            auto_terminate: false,
            step_concurrency_level: 1,
            step_count: 1,
        },
        Case {
            label: "transient cluster, nothing optional supplied",
            request: minimal_request("minimal"),
            collection_type: "INSTANCE_GROUP",
            auto_terminate: true,
            step_concurrency_level: 1,
            step_count: 0,
        },
        Case {
            label: "instance fleets, three applications, maximum concurrency",
            request: json!({
                "Name": "fleet cluster",
                "ReleaseLabel": "emr-7.1.0",
                "AutoScalingRole": "EMR_AutoScaling_DefaultRole",
                "JobFlowRole": INSTANCE_PROFILE, "ServiceRole": SERVICE_ROLE,
                "ScaleDownBehavior": "TERMINATE_AT_TASK_COMPLETION",
                "CustomAmiId": "ami-0123456789abcdef0",
                "StepConcurrencyLevel": 256,
                "Applications": [{ "Name": "Hive" }, { "Name": "Hadoop" }, { "Name": "Trino" }],
                "Tags": [
                    { "Key": "a", "Value": "1" },
                    { "Key": "b", "Value": "2" },
                    { "Key": "c", "Value": "3" },
                ],
                "Instances": {
                    "KeepJobFlowAliveWhenNoSteps": true,
                    "InstanceFleets": [{
                        "InstanceFleetType": "MASTER",
                        "TargetOnDemandCapacity": 1,
                        "InstanceTypeConfigs": [{ "InstanceType": "m5.xlarge" }],
                    }],
                },
                "Steps": [
                    step_config("one", "command-runner.jar", json!(["state-pusher-script"]), Value::Null),
                    step_config("two", "s3://jars/two.jar", Value::Null, json!({ "MainClass": "org.Two" })),
                ],
            }),
            collection_type: "INSTANCE_FLEET",
            auto_terminate: false,
            step_concurrency_level: 256,
            step_count: 2,
        },
        Case {
            label: "AMI-version cluster, termination protected, VisibleToAllUsers false",
            request: json!({
                "Name": "legacy",
                "AmiVersion": "2.4.2",
                "VisibleToAllUsers": false,
                "JobFlowRole": INSTANCE_PROFILE, "ServiceRole": SERVICE_ROLE,
                "Instances": {
                    "InstanceCount": 2,
                    "MasterInstanceType": "m1.small",
                    "SlaveInstanceType": "m1.small",
                    "TerminationProtected": true,
                },
            }),
            collection_type: "INSTANCE_GROUP",
            auto_terminate: true,
            step_concurrency_level: 1,
            step_count: 0,
        },
        Case {
            label: "empty name, which the model's length 0..256 allows",
            request: minimal_request(""),
            collection_type: "INSTANCE_GROUP",
            auto_terminate: true,
            step_concurrency_level: 1,
            step_count: 0,
        },
    ];

    let mut seen_ids: Vec<String> = Vec::new();
    for case in &cases {
        let label = case.label;
        let out = emr.ok("RunJobFlow", case.request.clone()).await;
        let id = out["JobFlowId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: no JobFlowId in {out}"))
            .to_string();
        assert_minted_id(label, "j-", &id);
        assert_eq!(
            out["ClusterArn"].as_str().unwrap_or_default(),
            format!("arn:aws:elasticmapreduce:us-east-1:000000000000:cluster/{id}"),
            "case {label:?}: the ARN must name the minted cluster, got {out}"
        );
        assert!(
            !seen_ids.contains(&id),
            "case {label:?}: id {id} was already minted for another cluster ({seen_ids:?})"
        );
        seen_ids.push(id.clone());

        let cluster = emr.describe_cluster(&id).await;
        assert_eq!(
            cluster["Id"].as_str(),
            Some(id.as_str()),
            "case {label:?}: DescribeCluster must answer the same id, got {cluster}"
        );
        assert_eq!(
            cluster["ClusterArn"], out["ClusterArn"],
            "case {label:?}: DescribeCluster must answer the ARN RunJobFlow minted, got {cluster}"
        );
        for member in ["Name", "ReleaseLabel", "LogUri", "ServiceRole", "Tags"] {
            if let Some(expected) = case.request.get(member) {
                assert_eq!(
                    &cluster[member], expected,
                    "case {label:?}: {member} must round-trip, got {cluster}"
                );
            }
        }
        // `Applications` names applications, not versions: the cluster reports the names
        // the request asked for, each resolved to the version its release installs (which
        // `tests/all/cluster_defaults.rs` pins against the release catalog).
        if let Some(requested) = case.request.get("Applications").and_then(Value::as_array) {
            let asked: Vec<&str> = requested
                .iter()
                .map(|application| application["Name"].as_str().unwrap_or_default())
                .collect();
            let reported: Vec<&str> = cluster["Applications"]
                .as_array()
                .map(|applications| {
                    applications
                        .iter()
                        .map(|application| application["Name"].as_str().unwrap_or_default())
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                reported, asked,
                "case {label:?}: Applications must round-trip, got {cluster}"
            );
        }
        if let Some(expected) = case.request.get("AmiVersion") {
            assert_eq!(
                &cluster["RequestedAmiVersion"], expected,
                "case {label:?}: AmiVersion is reported as RequestedAmiVersion, got {cluster}"
            );
        }
        assert_eq!(
            cluster["InstanceCollectionType"].as_str(),
            Some(case.collection_type),
            "case {label:?}: wrong InstanceCollectionType in {cluster}"
        );
        assert_eq!(
            cluster["AutoTerminate"].as_bool(),
            Some(case.auto_terminate),
            "case {label:?}: AutoTerminate is the complement of \
             KeepJobFlowAliveWhenNoSteps, got {cluster}"
        );
        assert_eq!(
            cluster["TerminationProtected"].as_bool(),
            Some(
                at(&case.request, &["Instances", "TerminationProtected"])
                    .as_bool()
                    .unwrap_or(false)
            ),
            "case {label:?}: TerminationProtected must round-trip, got {cluster}"
        );
        assert_eq!(
            cluster["StepConcurrencyLevel"].as_i64(),
            Some(case.step_concurrency_level),
            "case {label:?}: wrong StepConcurrencyLevel in {cluster}"
        );
        // "The VisibleToAllUsers parameter is no longer supported. By default, the value
        // is set to true. Setting it to false now has no effect."
        // (RunJobFlow$VisibleToAllUsers doc.)
        assert_eq!(
            cluster["VisibleToAllUsers"].as_bool(),
            Some(true),
            "case {label:?}: VisibleToAllUsers is always true, got {cluster}"
        );
        assert_eq!(
            at(&cluster, &["Ec2InstanceAttributes", "IamInstanceProfile"]),
            case.request.get("JobFlowRole").unwrap_or(&NULL),
            "case {label:?}: JobFlowRole is the cluster's instance profile, got {cluster}"
        );
        assert_eq!(
            at(&cluster, &["Ec2InstanceAttributes", "Ec2KeyName"]),
            at(&case.request, &["Instances", "Ec2KeyName"]),
            "case {label:?}: Ec2KeyName must round-trip, got {cluster}"
        );
        assert_eq!(
            at(&cluster, &["Ec2InstanceAttributes", "Ec2SubnetId"]),
            at(&case.request, &["Instances", "Ec2SubnetId"]),
            "case {label:?}: Ec2SubnetId must round-trip, got {cluster}"
        );

        // A brand-new cluster is not up yet, and its creation instant is recorded.
        assert_eq!(
            at(&cluster, &["Status", "State"]).as_str(),
            Some("STARTING"),
            "case {label:?}: a new cluster starts in STARTING, got {cluster}"
        );
        let created = at(&cluster, &["Status", "Timeline", "CreationDateTime"])
            .as_f64()
            .unwrap_or_else(|| panic!("case {label:?}: no CreationDateTime in {cluster}"));
        assert!(
            created >= started - 1.0,
            "case {label:?}: CreationDateTime {created} predates the test start {started}"
        );
        assert!(
            at(&cluster, &["Status", "Timeline", "EndDateTime"]).is_null(),
            "case {label:?}: a live cluster has no EndDateTime, got {cluster}"
        );

        // Every submitted step was persisted against this cluster.
        let steps = emr.list_steps(json!({ "ClusterId": id })).await;
        assert_eq!(
            steps["Steps"].as_array().map(Vec::len),
            Some(case.step_count),
            "case {label:?}: wrong number of persisted steps in {steps}"
        );
    }
}

/// THE RULE: a cluster AWS answers in a non-terminal state reaches its next state once
/// the clock makes the transition due — a new cluster provisions its capacity (`STARTING`)
/// and then runs its bootstrap actions (`BOOTSTRAPPING`) before it is ready, which is
/// `WAITING` for a cluster kept alive with no steps and `RUNNING` otherwise — and that is
/// recorded as `Timeline.ReadyDateTime`. A read never moves it anywhere else.
///
/// A cluster that is NOT kept alive does not REST in its ready state: its step list is
/// complete (it has none), so the shutdown "after the steps complete" is owed from one span
/// after it became ready and the cluster walks on through `TERMINATING` to `TERMINATED` —
/// the walk `cluster_self_termination.rs` pins instant by instant. What this case asserts
/// is the half that walk shares with the kept-alive one: the cluster LEFT `STARTING` for a
/// state of the shutdown path and recorded its `ReadyDateTime`.
#[tokio::test]
async fn a_starting_cluster_reaches_its_ready_state_when_the_clock_makes_it_due() {
    struct Case {
        label: &'static str,
        request: Value,
        /// The states the cluster may report once its launch span has passed.
        ready_states: &'static [&'static str],
        /// The state it comes to REST in, which nothing but a caller moves it out of.
        resting_state: &'static str,
    }

    let emr = Emr::new();
    let cases = [
        Case {
            label: "kept alive with no steps",
            request: kept_alive_request("waiting"),
            ready_states: &["WAITING"],
            resting_state: "WAITING",
        },
        Case {
            label: "transient",
            request: minimal_request("running"),
            ready_states: &["RUNNING", "TERMINATING", "TERMINATED"],
            resting_state: "TERMINATED",
        },
    ];

    let mut ids = Vec::new();
    for case in &cases {
        let id = emr.run_job_flow(case.request.clone()).await;
        let cluster = emr.describe_cluster(&id).await;
        assert_eq!(
            at(&cluster, &["Status", "State"]).as_str(),
            Some("STARTING"),
            "case {:?}: the transient state must be observable, got {cluster}",
            case.label
        );
        assert!(
            at(&cluster, &["Status", "Timeline", "ReadyDateTime"]).is_null(),
            "case {:?}: a STARTING cluster is not ready yet, got {cluster}",
            case.label
        );
        ids.push(id);
    }

    for (case, id) in cases.iter().zip(&ids) {
        // Waits for the CONDITION — the cluster having left the states it comes up through
        // for one of its ready path — rather than for a span.
        let ready_states = case.ready_states;
        let cluster = await_cluster_where(
            &emr,
            id,
            case.label,
            "a state of its ready path",
            |cluster| {
                ready_states.contains(
                    &at(cluster, &["Status", "State"]).as_str().unwrap_or_default(),
                )
            },
        )
        .await;
        let ready = at(&cluster, &["Status", "Timeline", "ReadyDateTime"])
            .as_f64()
            .unwrap_or_else(|| panic!("case {:?}: no ReadyDateTime in {cluster}", case.label));
        let created = at(&cluster, &["Status", "Timeline", "CreationDateTime"])
            .as_f64()
            .unwrap_or_default();
        assert!(
            ready > created,
            "case {:?}: ReadyDateTime {ready} must follow CreationDateTime {created}",
            case.label
        );

        // A read changes nothing a later caller sees — asserted once the cluster has come
        // to REST, because a cluster still walking the shutdown it was owed is moved
        // between two reads by the clock rather than by either read.
        let rested = await_cluster_state(&emr, id, case.label, case.resting_state).await;
        let again = emr.describe_cluster(id).await;
        assert_eq!(
            again, rested,
            "case {:?}: two DescribeCluster calls must answer the same cluster",
            case.label
        );
    }
}

/// Poll `DescribeCluster` until the cluster reports `state`, and answer what it then
/// reported.
///
/// Waits for the CONDITION rather than for a span: the deadline is far above every span
/// this service owes a cluster, so a slow host delays this rather than failing it.
pub(crate) async fn await_cluster_state(emr: &Emr, id: &str, case: &str, state: &str) -> Value {
    await_cluster_where(emr, id, case, state, |cluster| {
        at(cluster, &["Status", "State"]).as_str() == Some(state)
    })
    .await
}

/// Poll `DescribeCluster` until `holds` answers true of the cluster, and answer what it then
/// reported; `wanted` names what is waited for in the report a timeout leaves.
///
/// The one wait of this suite: it looks again until the condition holds or a deadline far
/// above every span this service owes a cluster passes, so a slow or loaded host delays a
/// case rather than failing it.
pub(crate) async fn await_cluster_where(
    emr: &Emr,
    id: &str,
    case: &str,
    wanted: &str,
    holds: impl Fn(&Value) -> bool,
) -> Value {
    let deadline = std::time::Instant::now() + CLUSTER_WAIT_DEADLINE;
    loop {
        let cluster = emr.describe_cluster(id).await;
        if holds(&cluster) {
            return cluster;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the cluster never reached {wanted}, last {cluster}"
        );
        tokio::time::sleep(CLUSTER_POLL_INTERVAL).await;
    }
}

// ---------------------------------------------------------------------------
// AddJobFlowSteps -> ListSteps / DescribeStep
// ---------------------------------------------------------------------------

/// THE RULE: a submitted step is persisted against its cluster under a minted `s-` id,
/// and both `ListSteps` and `DescribeStep` answer the name, `ActionOnFailure` and
/// Jar/Args/MainClass/Properties EXACTLY as submitted, newest step first. The table
/// spans the `StepConfig` shapes the model allows and every `ActionOnFailure` value.
#[tokio::test]
async fn submitted_steps_are_read_back_as_submitted_newest_first() {
    let emr = Emr::new();
    let cluster_id = emr.run_job_flow(kept_alive_request("stepped")).await;

    // Rows differ along: args (none / one / many), MainClass, Properties, and which
    // ActionOnFailure the step carries.
    let mut submitted: Vec<Value> = vec![
        step_config("bare jar", "s3://jars/bare.jar", Value::Null, Value::Null),
        step_config(
            "one arg",
            "command-runner.jar",
            json!(["state-pusher-script"]),
            Value::Null,
        ),
        step_config(
            "many args and a main class",
            "s3://jars/wordcount.jar",
            json!(["s3://in", "s3://out", "--verbose"]),
            json!({ "MainClass": "org.apache.hadoop.examples.WordCount" }),
        ),
        step_config(
            "properties",
            "s3://jars/props.jar",
            Value::Null,
            json!({ "Properties": [
                { "Key": "mapreduce.job.reduces", "Value": "4" },
                { "Key": "mapreduce.job.maps", "Value": "8" },
            ] }),
        ),
    ];
    for action in ACTION_ON_FAILURE {
        let mut step = step_config(
            action,
            "s3://jars/action.jar",
            json!(["--action", action]),
            Value::Null,
        );
        step["ActionOnFailure"] = json!(action);
        submitted.push(step);
    }

    // The first two arrive with the cluster, the rest through AddJobFlowSteps, so the
    // same rule is asserted on both entry points.
    let with_cluster = submitted[..2].to_vec();

    let second_cluster = emr
        .run_job_flow(json!({
            "Name": "stepped at launch",
            "JobFlowRole": INSTANCE_PROFILE, "ServiceRole": SERVICE_ROLE,
            "Instances": { "InstanceCount": 1, "MasterInstanceType": "m5.xlarge",
                           "KeepJobFlowAliveWhenNoSteps": true },
            "Steps": with_cluster,
        }))
        .await;
    let launched = emr.list_steps(json!({ "ClusterId": &second_cluster })).await;
    assert_eq!(
        launched["Steps"].as_array().map(Vec::len),
        Some(2),
        "RunJobFlow must persist one step per submitted StepConfig, got {launched}"
    );

    let out = emr
        .ok(
            "AddJobFlowSteps",
            json!({
                "JobFlowId": &cluster_id,
                "Steps": submitted.clone(),
                "ExecutionRoleArn": "arn:aws:iam::000000000000:role/StepRunner",
            }),
        )
        .await;
    let step_ids: Vec<String> = out["StepIds"]
        .as_array()
        .unwrap_or_else(|| panic!("AddJobFlowSteps must answer StepIds, got {out}"))
        .iter()
        .map(|id| id.as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        step_ids.len(),
        submitted.len(),
        "one StepId per submitted step, got {out}"
    );
    for id in &step_ids {
        assert_minted_id("AddJobFlowSteps", "s-", id);
    }

    // ListSteps reports the cluster's steps newest first, so reversing the submission
    // order must line up with the reported order.
    let listed = emr.list_steps(json!({ "ClusterId": &cluster_id })).await;
    let reported: Vec<Value> = listed["Steps"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        reported.len(),
        submitted.len(),
        "ListSteps must report every persisted step, got {listed}"
    );
    let reported_ids: Vec<&str> = reported
        .iter()
        .map(|step| step["Id"].as_str().unwrap_or_default())
        .collect();
    let newest_first: Vec<&str> = step_ids.iter().rev().map(String::as_str).collect();
    assert_eq!(
        reported_ids, newest_first,
        "ListSteps must report the steps newest first, got {listed}"
    );

    for (config, step_id) in submitted.iter().zip(&step_ids) {
        let label = config["Name"].as_str().unwrap_or_default().to_string();
        let out = emr
            .ok(
                "DescribeStep",
                json!({ "ClusterId": &cluster_id, "StepId": step_id }),
            )
            .await;
        let step = &out["Step"];
        assert_eq!(
            step["Id"].as_str(),
            Some(step_id.as_str()),
            "case {label:?}: DescribeStep must answer the step asked for, got {out}"
        );
        assert_eq!(
            step["Name"], config["Name"],
            "case {label:?}: the step name must round-trip, got {out}"
        );
        assert_eq!(
            step["ActionOnFailure"],
            *config.get("ActionOnFailure").unwrap_or(&NULL),
            "case {label:?}: ActionOnFailure must round-trip, got {out}"
        );
        assert_eq!(
            at(step, &["Config", "Jar"]),
            at(config, &["HadoopJarStep", "Jar"]),
            "case {label:?}: the Jar must round-trip, got {out}"
        );
        assert_eq!(
            at(step, &["Config", "Args"]),
            at(config, &["HadoopJarStep", "Args"]),
            "case {label:?}: the Args must round-trip in order, got {out}"
        );
        assert_eq!(
            at(step, &["Config", "MainClass"]),
            at(config, &["HadoopJarStep", "MainClass"]),
            "case {label:?}: the MainClass must round-trip, got {out}"
        );
        let expected_properties: Value = match at(config, &["HadoopJarStep", "Properties"]).as_array()
        {
            Some(entries) => {
                let mut map = serde_json::Map::new();
                for entry in entries {
                    map.insert(
                        entry["Key"].as_str().unwrap_or_default().to_string(),
                        entry["Value"].clone(),
                    );
                }
                Value::Object(map)
            }
            None => json!({}),
        };
        assert_eq!(
            at(step, &["Config", "Properties"]),
            &expected_properties,
            "case {label:?}: the step Properties must round-trip, got {out}"
        );
        assert_eq!(
            step["ExecutionRoleArn"].as_str(),
            Some("arn:aws:iam::000000000000:role/StepRunner"),
            "case {label:?}: the step's runtime role must round-trip, got {out}"
        );

        // The step records when it was created, whatever the cluster's step runner has
        // since made of it: this case is about the submitted definition round-tripping,
        // and the lifecycle rule is the subject of tests/all/glue_metastore_steps.rs.
        assert!(
            at(step, &["Status", "Timeline", "CreationDateTime"])
                .as_f64()
                .is_some(),
            "case {label:?}: the step records when it was created, got {out}"
        );
        // The work of each of these steps — a jar of its own, or a command submitting no
        // statement — is work this emulator does not run, so the runner settles the step in
        // the modelled failure state rather than reporting a success for it. Polled, so the
        // assertion does not depend on how fast this machine is.
        let settled = await_settled_step(&emr, &cluster_id, step_id).await;
        assert_eq!(
            at(&settled, &["Status", "State"]).as_str(),
            Some("FAILED"),
            "case {label:?}: work this emulator does not run fails the step, got {settled}"
        );
        assert!(
            at(&settled, &["Status", "FailureDetails", "Message"])
                .as_str()
                .is_some_and(|message| message.contains("No application was started")),
            "case {label:?}: the failure says no application ran, got {settled}"
        );
        assert!(
            at(&settled, &["Status", "StateChangeReason", "Code"]).is_null(),
            "case {label:?}: the service publishes no code for the state change, got {settled}"
        );

        // The ListSteps summary of the same step agrees with DescribeStep, read at the
        // same point in the step's life.
        let listed_now = emr
            .list_steps(json!({ "ClusterId": &cluster_id, "StepIds": [step_id] }))
            .await;
        let summary = &listed_now["Steps"][0];
        for member in ["Name", "Config", "ActionOnFailure", "Status"] {
            assert_eq!(
                summary[member], settled[member],
                "case {label:?}: the ListSteps {member} must agree with DescribeStep, got \
                 {listed_now}"
            );
        }
    }
}

/// Poll `DescribeStep` until the step has left the states a further transition is owed
/// from, answering the step as it settled.
///
/// `pub(crate)` so every suite in this test target waits for a step the same way.
pub(crate) async fn await_settled_step(emr: &Emr, cluster_id: &str, step_id: &str) -> Value {
    const SETTLED: [&str; 4] = ["COMPLETED", "CANCELLED", "FAILED", "INTERRUPTED"];
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let out = emr
            .ok(
                "DescribeStep",
                json!({ "ClusterId": cluster_id, "StepId": step_id }),
            )
            .await;
        let step = out["Step"].clone();
        let state = at(&step, &["Status", "State"])
            .as_str()
            .unwrap_or_default()
            .to_string();
        if SETTLED.contains(&state.as_str()) {
            return step;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "step {step_id} must reach a state it never leaves, last saw {state:?} in {out}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// THE RULE: `ListSteps` answers only the steps matching the `StepStates` and `StepIds`
/// filters, and hands out at most one page plus a `Marker` that resumes exactly where
/// the page stopped.
#[tokio::test]
async fn list_steps_honours_its_filters_and_paginates_on_marker() {
    let emr = Emr::new();
    let total = PAGE_SIZE + 10;
    let steps: Vec<Value> = (0..total)
        .map(|index| {
            step_config(
                &format!("step {index}"),
                "s3://jars/paged.jar",
                json!([index.to_string()]),
                Value::Null,
            )
        })
        .collect();
    let cluster_id = emr
        .run_job_flow(json!({
            "Name": "paged",
            "JobFlowRole": INSTANCE_PROFILE, "ServiceRole": SERVICE_ROLE,
            // Every step runs at once, so the runner settles them all within one span and
            // the StepStates case below has settled states to filter on.
            "StepConcurrencyLevel": total,
            "Instances": { "InstanceCount": 1, "MasterInstanceType": "m5.xlarge",
                           "KeepJobFlowAliveWhenNoSteps": true },
            "Steps": steps,
        }))
        .await;

    let all = emr.list_steps(json!({ "ClusterId": &cluster_id })).await;
    let first_page: Vec<String> = all["Steps"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|step| step["Id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        first_page.len(),
        PAGE_SIZE,
        "case \"first page\": a page carries at most {PAGE_SIZE} steps, got {}",
        first_page.len()
    );
    let marker = all["Marker"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"first page\": a full page must carry a Marker, got {all}"))
        .to_string();

    let second = emr
        .list_steps(json!({ "ClusterId": &cluster_id, "Marker": &marker }))
        .await;
    assert_eq!(
        second["Steps"][0]["Id"].as_str(),
        Some(marker.as_str()),
        "case \"marker\": the marker must resume at the first step the page omitted, got {second}"
    );
    let second_page: Vec<String> = second["Steps"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|step| step["Id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        second_page.len(),
        total - PAGE_SIZE,
        "case \"last page\": expected {} steps, got {}",
        total - PAGE_SIZE,
        second_page.len()
    );
    assert!(
        second["Marker"].is_null(),
        "case \"last page\": the final page carries no Marker, got {second}"
    );
    let mut walked = first_page.clone();
    walked.extend(second_page.iter().cloned());
    walked.sort();
    walked.dedup();
    assert_eq!(
        walked.len(),
        total,
        "case \"walk\": paging must visit every step exactly once, saw {}",
        walked.len()
    );

    // StepStates filter: it answers exactly the steps whose State it names. The ten steps
    // asserted on are first waited out to the state they never leave — their work is a jar
    // this emulator does not run, so the runner settles each FAILED
    // (tests/all/glue_metastore_steps.rs owns that rule) — so what the filter must answer
    // is known rather than racing the clock.
    let wanted: Vec<String> = first_page.iter().take(10).cloned().collect();
    for step_id in &wanted {
        let settled = await_settled_step(&emr, &cluster_id, step_id).await;
        assert_eq!(
            at(&settled, &["Status", "State"]).as_str(),
            Some("FAILED"),
            "case \"settled\": work this emulator does not run fails the step, got {settled}"
        );
    }
    for (label, states, expected) in [
        ("states FAILED", json!(["FAILED"]), wanted.len()),
        ("states FAILED+CANCELLED", json!(["FAILED", "CANCELLED"]), wanted.len()),
        ("states COMPLETED+FAILED", json!(["COMPLETED", "FAILED"]), wanted.len()),
        ("states PENDING", json!(["PENDING"]), 0),
        ("states COMPLETED", json!(["COMPLETED"]), 0),
        ("states INTERRUPTED", json!(["INTERRUPTED"]), 0),
    ] {
        let out = emr
            .list_steps(json!({
                "ClusterId": &cluster_id,
                "StepIds": &wanted,
                "StepStates": states.clone(),
            }))
            .await;
        let got: Vec<&str> = out["Steps"]
            .as_array()
            .map(|steps| {
                steps
                    .iter()
                    .map(|step| step["Id"].as_str().unwrap_or_default())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            got.len(),
            expected,
            "case {label:?}: expected {expected} of the ten settled steps, got {out}"
        );
        if expected > 0 {
            let newest_first: Vec<&str> = wanted.iter().map(String::as_str).collect();
            assert_eq!(
                got, newest_first,
                "case {label:?}: expected exactly the settled steps, newest first, got {out}"
            );
        }
    }

    // StepIds filter: exactly the named steps, still newest first.
    for size in [1usize, 2, 10] {
        let label = format!("{size} step ids");
        let wanted: Vec<&String> = first_page.iter().take(size).collect();
        let out = emr
            .list_steps(json!({ "ClusterId": &cluster_id, "StepIds": wanted }))
            .await;
        let got: Vec<&str> = out["Steps"]
            .as_array()
            .map(|steps| {
                steps
                    .iter()
                    .map(|step| step["Id"].as_str().unwrap_or_default())
                    .collect()
            })
            .unwrap_or_default();
        let expected: Vec<&str> = first_page.iter().take(size).map(String::as_str).collect();
        assert_eq!(
            got, expected,
            "case {label:?}: expected exactly the named steps, got {out}"
        );
    }

    // "You can specify a maximum of 10 stepIDs." (ListSteps doc.)
    let eleven: Vec<&String> = first_page.iter().take(11).collect();
    let (status, payload) = emr
        .call(
            "ListSteps",
            json!({ "ClusterId": &cluster_id, "StepIds": eleven }),
        )
        .await;
    assert_refused("11 step ids", status, &payload, "InvalidRequestException");
}

// ---------------------------------------------------------------------------
// ListClusters
// ---------------------------------------------------------------------------

/// THE RULE: `ListClusters` answers a summary for every persisted cluster whose state
/// is among `ClusterStates` and whose creation instant falls inside the
/// `CreatedAfter`/`CreatedBefore` window, one page at a time.
#[tokio::test]
async fn list_clusters_filters_by_state_and_creation_window() {
    let emr = Emr::new();
    let kept_alive = emr.run_job_flow(kept_alive_request("kept alive")).await;
    let transient = emr.run_job_flow(minimal_request("transient")).await;
    let to_terminate = emr.run_job_flow(kept_alive_request("doomed")).await;

    let all = emr.ok("ListClusters", json!({})).await;
    let ids: Vec<&str> = all["Clusters"]
        .as_array()
        .map(|clusters| {
            clusters
                .iter()
                .map(|cluster| cluster["Id"].as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        ids,
        vec![to_terminate.as_str(), transient.as_str(), kept_alive.as_str()],
        "case \"unfiltered\": every cluster, newest first, got {all}"
    );
    let clusters = all["Clusters"].as_array().cloned().unwrap_or_default();
    for cluster in &clusters {
        let id = cluster["Id"].as_str().unwrap_or_default();
        assert_eq!(
            cluster["ClusterArn"].as_str().unwrap_or_default(),
            format!("arn:aws:elasticmapreduce:us-east-1:000000000000:cluster/{id}"),
            "case \"unfiltered\": the summary must carry the cluster's ARN, got {cluster}"
        );
        assert!(
            at(cluster, &["Status", "Timeline", "CreationDateTime"])
                .as_f64()
                .is_some(),
            "case \"unfiltered\": the summary must carry the creation instant, got {cluster}"
        );
    }

    // Before the transition is due every cluster is STARTING.
    for (label, states, expected) in [
        ("STARTING", json!(["STARTING"]), 3),
        ("RUNNING+WAITING", json!(["RUNNING", "WAITING"]), 0),
        ("TERMINATED", json!(["TERMINATED"]), 0),
    ] {
        let out = emr
            .ok("ListClusters", json!({ "ClusterStates": states }))
            .await;
        assert_eq!(
            out["Clusters"].as_array().map(Vec::len),
            Some(expected),
            "case {label:?} (before the transition): expected {expected} clusters, got {out}"
        );
    }

    // Each cluster is waited for by its own CONDITION rather than by a span: the two kept
    // alive resting in WAITING, and the transient one having run its (empty) queue to the
    // terminal TERMINATED it is owed.
    await_cluster_state(&emr, &kept_alive, "kept alive", "WAITING").await;
    await_cluster_state(&emr, &to_terminate, "doomed", "WAITING").await;
    await_cluster_state(&emr, &transient, "transient", "TERMINATED").await;
    emr.ok("TerminateJobFlows", json!({ "JobFlowIds": [&to_terminate] }))
        .await;
    await_cluster_state(&emr, &to_terminate, "doomed", "TERMINATED").await;

    for (label, states, expected) in [
        ("WAITING", json!(["WAITING"]), vec![kept_alive.as_str()]),
        // The transient cluster does not rest in RUNNING: with no steps to run its queue
        // was complete the instant it became ready, so the shutdown it was owed has since
        // carried it to TERMINATED.
        ("RUNNING", json!(["RUNNING"]), vec![]),
        (
            "RUNNING+WAITING",
            json!(["RUNNING", "WAITING"]),
            vec![kept_alive.as_str()],
        ),
        (
            "TERMINATED",
            json!(["TERMINATED"]),
            vec![to_terminate.as_str(), transient.as_str()],
        ),
        ("STARTING", json!(["STARTING"]), vec![]),
    ] {
        let out = emr
            .ok("ListClusters", json!({ "ClusterStates": states }))
            .await;
        let got: Vec<&str> = out["Clusters"]
            .as_array()
            .map(|clusters| {
                clusters
                    .iter()
                    .map(|cluster| cluster["Id"].as_str().unwrap_or_default())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            got, expected,
            "case {label:?} (after the transition): wrong clusters in {out}"
        );
    }

    // The creation-time window, spanning both bounds and both sides of each.
    let now = epoch_secs(SystemTime::now());
    let before = now - 3600.0;
    let after = now + 3600.0;
    for (label, filter, expected) in [
        ("after an hour ago", json!({ "CreatedAfter": before }), 3),
        ("after an hour hence", json!({ "CreatedAfter": after }), 0),
        ("before an hour hence", json!({ "CreatedBefore": after }), 3),
        ("before an hour ago", json!({ "CreatedBefore": before }), 0),
        (
            "inside the window",
            json!({ "CreatedAfter": before, "CreatedBefore": after }),
            3,
        ),
        (
            "window entirely in the future",
            json!({ "CreatedAfter": after, "CreatedBefore": after + 60.0 }),
            0,
        ),
    ] {
        let out = emr.ok("ListClusters", filter.clone()).await;
        assert_eq!(
            out["Clusters"].as_array().map(Vec::len),
            Some(expected),
            "case {label:?}: expected {expected} clusters for {filter}, got {out}"
        );
    }
}

/// THE RULE: `ListClusters` carries at most one page, and its `Marker` resumes exactly
/// where that page stopped.
#[tokio::test]
async fn list_clusters_paginates_on_marker() {
    let emr = Emr::new();
    let total = PAGE_SIZE + 5;
    let mut minted = Vec::new();
    for index in 0..total {
        minted.push(
            emr.run_job_flow(minimal_request(&format!("cluster {index}")))
                .await,
        );
    }

    let first = emr.ok("ListClusters", json!({})).await;
    let page_one: Vec<String> = first["Clusters"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|cluster| cluster["Id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        page_one.len(),
        PAGE_SIZE,
        "case \"first page\": at most {PAGE_SIZE} clusters per call, got {}",
        page_one.len()
    );
    let marker = first["Marker"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"first page\": a full page must carry a Marker, got {first}"))
        .to_string();

    let second = emr.ok("ListClusters", json!({ "Marker": &marker })).await;
    let page_two: Vec<String> = second["Clusters"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|cluster| cluster["Id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        page_two.first().map(String::as_str),
        Some(marker.as_str()),
        "case \"second page\": the marker must resume at the next cluster, got {second}"
    );
    assert_eq!(
        page_two.len(),
        total - PAGE_SIZE,
        "case \"second page\": expected {} clusters, got {}",
        total - PAGE_SIZE,
        page_two.len()
    );
    assert!(
        second["Marker"].is_null(),
        "case \"second page\": the final page carries no Marker, got {second}"
    );
    let mut walked = page_one;
    walked.extend(page_two);
    walked.sort();
    walked.dedup();
    assert_eq!(
        walked.len(),
        total,
        "case \"walk\": paging must visit every cluster exactly once, saw {}",
        walked.len()
    );
}

// ---------------------------------------------------------------------------
// TerminateJobFlows
// ---------------------------------------------------------------------------

/// THE RULE: `TerminateJobFlows` moves each named cluster to `TERMINATING` with a
/// `USER_REQUEST` state-change reason, the clock then owes it `TERMINATED` with an
/// `EndDateTime`, and a batch naming a termination-protected cluster is refused in whole
/// and shuts nothing down (the batch rule itself lives in
/// `tests/all/terminate_job_flows_protection.rs`).
#[tokio::test]
async fn terminate_job_flows_shuts_each_named_cluster_down() {
    let emr = Emr::new();
    let doomed = emr.run_job_flow(kept_alive_request("doomed")).await;
    let survivor = emr.run_job_flow(kept_alive_request("survivor")).await;
    let protected = emr
        .run_job_flow(json!({
            "Name": "protected",
            "JobFlowRole": INSTANCE_PROFILE, "ServiceRole": SERVICE_ROLE,
            "Instances": {
                "InstanceCount": 1,
                "MasterInstanceType": "m5.xlarge",
                "KeepJobFlowAliveWhenNoSteps": true,
                "TerminationProtected": true,
            },
        }))
        .await;

    // "Indicates whether Amazon EMR will lock the cluster to prevent the Amazon EC2
    // instances from being terminated by an API call" (Cluster$TerminationProtected): the
    // batch that names the locked cluster is refused, and the cluster beside it in the
    // same batch is not shut down either.
    let (status, payload) = emr
        .call("TerminateJobFlows", json!({ "JobFlowIds": [&doomed, &protected] }))
        .await;
    assert_refused(
        "a batch naming a protected cluster",
        status,
        &payload,
        "ValidationException",
    );
    let locked = emr.describe_cluster(&protected).await;
    assert_eq!(
        at(&locked, &["Status", "State"]).as_str(),
        Some("STARTING"),
        "case \"protected cluster\": an API call must not terminate it, got {locked}"
    );
    assert!(
        at(&locked, &["Status", "StateChangeReason"]).is_null(),
        "case \"protected cluster\": nothing changed its state, got {locked}"
    );
    let spared = emr.describe_cluster(&doomed).await;
    assert!(
        at(&spared, &["Status", "StateChangeReason"]).is_null(),
        "case \"a batch naming a protected cluster\": the cluster beside it must not be \
         shut down, got {spared}"
    );

    emr.ok("TerminateJobFlows", json!({ "JobFlowIds": [&doomed] }))
        .await;

    let cluster = emr.describe_cluster(&doomed).await;
    assert_eq!(
        at(&cluster, &["Status", "State"]).as_str(),
        Some("TERMINATING"),
        "case \"terminated cluster\": the transient state must be observable, got {cluster}"
    );
    assert_eq!(
        at(&cluster, &["Status", "StateChangeReason", "Code"]).as_str(),
        Some("USER_REQUEST"),
        "case \"terminated cluster\": the reason must be USER_REQUEST, got {cluster}"
    );
    assert!(
        at(&cluster, &["Status", "Timeline", "EndDateTime"]).is_null(),
        "case \"terminated cluster\": it has not ended yet, got {cluster}"
    );

    tokio::time::sleep(PAST_TRANSITION).await;

    let cluster = emr.describe_cluster(&doomed).await;
    assert_eq!(
        at(&cluster, &["Status", "State"]).as_str(),
        Some("TERMINATED"),
        "case \"terminated cluster\": it must reach TERMINATED, got {cluster}"
    );
    let ended = at(&cluster, &["Status", "Timeline", "EndDateTime"])
        .as_f64()
        .unwrap_or_else(|| panic!("case \"terminated cluster\": no EndDateTime in {cluster}"));
    let created = at(&cluster, &["Status", "Timeline", "CreationDateTime"])
        .as_f64()
        .unwrap_or_default();
    assert!(
        ended > created,
        "case \"terminated cluster\": EndDateTime {ended} must follow CreationDateTime {created}"
    );

    // Terminating one cluster leaves the others alone: the survivor is still coming up,
    // which is where the clock had it when this batch named another cluster.
    let untouched = emr.describe_cluster(&survivor).await;
    assert_eq!(
        at(&untouched, &["Status", "State"]).as_str(),
        Some("STARTING"),
        "case \"survivor\": an unnamed cluster must not be terminated, got {untouched}"
    );

    // Repeating the call is idempotent: the cluster stays terminated with the same end.
    emr.ok("TerminateJobFlows", json!({ "JobFlowIds": [&doomed] }))
        .await;
    let again = emr.describe_cluster(&doomed).await;
    assert_eq!(
        again, cluster,
        "case \"repeat terminate\": a terminated cluster must not change"
    );

    // A step cannot be added to a cluster that is no longer running.
    let (status, payload) = emr
        .call(
            "AddJobFlowSteps",
            json!({
                "JobFlowId": &doomed,
                "Steps": [step_config("late", "s3://jars/late.jar", Value::Null, Value::Null)],
            }),
        )
        .await;
    assert_refused(
        "add a step to a terminated cluster",
        status,
        &payload,
        "ValidationException",
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// THE RULE: an id no resource carries is refused with the operation's client error,
/// never an empty 200 — including a step id that belongs to ANOTHER cluster.
#[tokio::test]
async fn unknown_ids_are_refused_rather_than_answered_empty() {
    let emr = Emr::new();
    let first = emr.run_job_flow(kept_alive_request("first")).await;
    let second = emr.run_job_flow(kept_alive_request("second")).await;
    let step_ids = emr
        .ok(
            "AddJobFlowSteps",
            json!({
                "JobFlowId": &first,
                "Steps": [step_config("only", "s3://jars/only.jar", Value::Null, Value::Null)],
            }),
        )
        .await["StepIds"]
        .clone();
    let step_of_first = step_ids[0].as_str().unwrap_or_default().to_string();

    for (label, operation, body, code) in [
        (
            "DescribeCluster with an unknown id",
            "DescribeCluster",
            json!({ "ClusterId": "j-DOESNOTEXIST" }),
            "InvalidRequestException",
        ),
        (
            "ListSteps with an unknown cluster",
            "ListSteps",
            json!({ "ClusterId": "j-DOESNOTEXIST" }),
            "InvalidRequestException",
        ),
        (
            "DescribeStep with an unknown cluster",
            "DescribeStep",
            json!({ "ClusterId": "j-DOESNOTEXIST", "StepId": &step_of_first }),
            "InvalidRequestException",
        ),
        (
            "DescribeStep with an unknown step",
            "DescribeStep",
            json!({ "ClusterId": &first, "StepId": "s-DOESNOTEXIST" }),
            "InvalidRequestException",
        ),
        (
            "DescribeStep with a step of another cluster",
            "DescribeStep",
            json!({ "ClusterId": &second, "StepId": &step_of_first }),
            "InvalidRequestException",
        ),
        (
            "AddJobFlowSteps with an unknown job flow",
            "AddJobFlowSteps",
            json!({
                "JobFlowId": "j-DOESNOTEXIST",
                "Steps": [step_config("s", "s3://jars/s.jar", Value::Null, Value::Null)],
            }),
            "ValidationException",
        ),
        (
            "TerminateJobFlows with an unknown job flow",
            "TerminateJobFlows",
            json!({ "JobFlowIds": ["j-DOESNOTEXIST"] }),
            "ValidationException",
        ),
        (
            "ListSteps with an unknown marker",
            "ListSteps",
            json!({ "ClusterId": &first, "Marker": "s-NOPE" }),
            "InvalidRequestException",
        ),
        (
            "ListClusters with an unknown marker",
            "ListClusters",
            json!({ "Marker": "j-NOPE" }),
            "InvalidRequestException",
        ),
    ] {
        let (status, payload) = emr.call(operation, body).await;
        assert_refused(label, status, &payload, code);
    }

    // A refused TerminateJobFlows terminated nothing.
    for (label, id) in [("first", &first), ("second", &second)] {
        let cluster = emr.describe_cluster(id).await;
        assert_ne!(
            at(&cluster, &["Status", "State"]).as_str(),
            Some("TERMINATING"),
            "case {label:?}: a refused request must not have terminated it, got {cluster}"
        );
    }
}

/// THE RULE: a request outside a limit the service documents is refused with a 400,
/// and one at the limit is accepted. Each row names the documented bound it probes.
#[tokio::test]
async fn documented_limits_are_enforced_at_their_bounds() {
    let emr = Emr::new();

    // "The RunJobFlow request can contain InstanceFleets parameters or InstanceGroups
    // parameters, but not both." (RunJobFlow doc.)
    let (status, payload) = emr
        .call(
            "RunJobFlow",
            json!({
                "Name": "both",
                "JobFlowRole": INSTANCE_PROFILE, "ServiceRole": SERVICE_ROLE,
                "Instances": {
                    "InstanceGroups": [{
                        "InstanceRole": "MASTER",
                        "InstanceType": "m5.xlarge",
                        "InstanceCount": 1,
                    }],
                    "InstanceFleets": [{
                        "InstanceFleetType": "MASTER",
                        "InstanceTypeConfigs": [{ "InstanceType": "m5.xlarge" }],
                    }],
                },
            }),
        )
        .await;
    assert_refused(
        "InstanceGroups and InstanceFleets together",
        status,
        &payload,
        "ValidationException",
    );

    // Either one alone is accepted.
    for (label, instances) in [
        (
            "InstanceGroups alone",
            json!({ "InstanceGroups": [{
                "InstanceRole": "MASTER", "InstanceType": "m5.xlarge", "InstanceCount": 1 }] }),
        ),
        (
            "InstanceFleets alone",
            json!({ "InstanceFleets": [{
                "InstanceFleetType": "MASTER",
                "InstanceTypeConfigs": [{ "InstanceType": "m5.xlarge" }] }] }),
        ),
    ] {
        let (status, payload) = emr
            .call(
                "RunJobFlow",
                json!({
                    "Name": label,
                    "JobFlowRole": INSTANCE_PROFILE, "ServiceRole": SERVICE_ROLE,
                    "Instances": instances,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200, got {status} with {payload}"
        );
    }

    // "Specifies the number of steps that can be executed concurrently. The default
    // value is 1. The maximum value is 256." (RunJobFlow$StepConcurrencyLevel doc.)
    for (level, expected) in [(0i64, 400u16), (1, 200), (256, 200), (257, 400)] {
        let label = format!("StepConcurrencyLevel {level}");
        let mut request = minimal_request(&label);
        request["StepConcurrencyLevel"] = json!(level);
        let (status, payload) = emr.call("RunJobFlow", request).await;
        assert_eq!(
            status, expected,
            "case {label:?}: expected HTTP {expected}, got {status} with {payload}"
        );
        if expected == 400 {
            assert_refused(&label, status, &payload, "ValidationException");
        }
    }

    // "A maximum of 256 steps are allowed in each job flow." (RunJobFlow doc.) Probed
    // at the bound and one past it, and again through AddJobFlowSteps on a cluster
    // that already holds steps.
    for (count, expected) in [(256usize, 200u16), (257, 400)] {
        let label = format!("{count} steps at launch");
        let steps: Vec<Value> = (0..count)
            .map(|index| {
                step_config(
                    &format!("step {index}"),
                    "s3://jars/limit.jar",
                    Value::Null,
                    Value::Null,
                )
            })
            .collect();
        let mut request = minimal_request(&label);
        request["Steps"] = Value::Array(steps);
        let (status, payload) = emr.call("RunJobFlow", request).await;
        assert_eq!(
            status, expected,
            "case {label:?}: expected HTTP {expected}, got {status} with {payload}"
        );
    }

    let cluster_id = emr
        .run_job_flow(json!({
            "Name": "nearly full",
            "JobFlowRole": INSTANCE_PROFILE, "ServiceRole": SERVICE_ROLE,
            "Instances": { "InstanceCount": 1, "MasterInstanceType": "m5.xlarge",
                           "KeepJobFlowAliveWhenNoSteps": true },
            "Steps": (0..255).map(|index| step_config(
                &format!("step {index}"), "s3://jars/limit.jar", Value::Null, Value::Null))
                .collect::<Vec<_>>(),
        }))
        .await;
    for (label, expected) in [
        ("AddJobFlowSteps reaching exactly 256 steps", 200u16),
        ("AddJobFlowSteps asking for a 257th step", 400u16),
    ] {
        let (status, payload) = emr
            .call(
                "AddJobFlowSteps",
                json!({
                    "JobFlowId": &cluster_id,
                    "Steps": [step_config(label, "s3://jars/limit.jar", Value::Null, Value::Null)],
                }),
            )
            .await;
        assert_eq!(
            status, expected,
            "case {label:?}: expected HTTP {expected}, got {status} with {payload}"
        );
    }

    // "The maximum number of clusters allowed is 10." (TerminateJobFlows doc.)
    let mut ids = Vec::new();
    for index in 0..11 {
        ids.push(emr.run_job_flow(minimal_request(&format!("t{index}"))).await);
    }
    for (count, expected) in [(10usize, 200u16), (11, 400)] {
        let label = format!("TerminateJobFlows with {count} ids");
        let named: Vec<&String> = ids.iter().take(count).collect();
        let (status, payload) = emr
            .call("TerminateJobFlows", json!({ "JobFlowIds": named }))
            .await;
        assert_eq!(
            status, expected,
            "case {label:?}: expected HTTP {expected}, got {status} with {payload}"
        );
    }
}

/// THE RULE: the account holds a bounded number of clusters, so a launch past that bound
/// is refused rather than accepted into unbounded growth — probed at the bound and one
/// past it. A cluster record outlives its cluster (`ListClusters` reports a terminated
/// one), so terminating does not make room either.
#[tokio::test]
async fn the_cluster_ceiling_is_enforced_at_its_bound() {
    let emr = Emr::new();

    // Up to and including the bound every launch is accepted.
    for held in 0..MAX_CLUSTERS {
        let label = format!("cluster {} of {MAX_CLUSTERS}", held + 1);
        let (status, payload) = emr.call("RunJobFlow", minimal_request(&label)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200, got {status} with {payload}"
        );
    }

    // One past it is refused.
    let (status, payload) = emr
        .call("RunJobFlow", minimal_request("one past the ceiling"))
        .await;
    assert_refused(
        "one past the ceiling",
        status,
        &payload,
        "ValidationException",
    );

    // The refusal stands after the clusters terminate: a terminated cluster is still a
    // record this account holds.
    let listed = emr.ok("ListClusters", json!({})).await;
    let first = listed["Clusters"][0]["Id"]
        .as_str()
        .expect("ListClusters must report the clusters this account holds")
        .to_string();
    emr.ok("TerminateJobFlows", json!({ "JobFlowIds": [first] }))
        .await;
    let (status, payload) = emr
        .call("RunJobFlow", minimal_request("after a termination"))
        .await;
    assert_refused(
        "after a termination",
        status,
        &payload,
        "ValidationException",
    );
}
