// SPDX-License-Identifier: CC0-1.0

//! WHOSE permissions decide what a step's own command line may read and write in Amazon S3,
//! end to end over the wire.
//!
//! THE RULE: a step's work runs on the cluster's Amazon EC2 instances — "Also called instance
//! profile and Amazon EC2 role. An IAM role for an Amazon EMR cluster. The Amazon EC2
//! instances of the cluster assume this role" (`RunJobFlow$JobFlowRole` doc) — so every AWS
//! request that work issues is made with a credential of THAT role and is authorized against
//! the permissions in force on it, where an explicit `Deny` is final. A request the policies
//! refuse is not performed: the command that issued it exits non-zero, so "for a step to be
//! considered complete, the main function must exit with a zero exit code"
//! (`AddJobFlowSteps` doc) settles that step in `FAILED` with `FailureDetails`, and the
//! stages its command line chained after that command never run — their objects do not
//! exist.
//!
//! A role this emulator holds no identity policy for is the one case that says NOTHING: an
//! empty policy set is the routine state of a role provisioned out of band, so it bounds
//! nothing and the work runs, exactly as the log-delivery boundary reads such a role.
//!
//! The rows span what the rule generalises over: the statement in force on the role (none at
//! all, a grant of the whole service, a grant beside an explicit deny of the object READ over
//! one input prefix, that same deny over a prefix no step reads, and a grant beside an
//! explicit deny of the object WRITE of one marker), and within each row the two steps of the
//! SAME cluster differ along the prefix they read — so a refusal is told apart from "the deny
//! was ignored" by the other half of the same cluster still completing.
//!
//! IAM, Amazon S3 and Amazon EMR are mounted on ONE engine: each instance profile is built the
//! way a client builds one (`CreateRole`, `PutRolePolicy`, `CreateInstanceProfile`,
//! `AddRoleToInstanceProfile`), every input object is written through S3's own `PutObject`,
//! and every marker a step would have written is read back through S3's own `GetObject` — so
//! what is asserted is what a client's own reads find.
//!
//! Nothing here waits a fixed time: a cluster's phases and its steps' spans are stored due
//! instants, so every wait LOOKS AGAIN until the condition holds or the generous [`DEADLINE`]
//! passes.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_emr_elasticmapreduce::state::COMMAND_FAILED_REASON;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every service is addressed in: the one whose `CreateBucket` needs no location
/// constraint in its body.
const REGION: &str = "us-east-1";

/// The bucket every row's steps read their input from and write their markers to.
const BUCKET: &str = "emr-step-input-read-authorization";

/// The two input prefixes a row's steps read, and the bytes both hold.
const DENIED_PREFIX: &str = "input-denied";
const ALLOWED_PREFIX: &str = "input-allowed";
const INPUT_BODY: &str = "input bytes\n";

/// What a step's chained stage writes into its marker, and the object it holds afterwards.
const MARKER: &str = "ok";
const MARKER_BODY: &str = "ok\n";

/// The service role every launch names. Amazon EMR resolves it against IAM once the launch
/// has been accepted — a cluster whose `ServiceRole` names no role of the account is settled
/// `TERMINATED_WITH_ERRORS` and runs none of its steps
/// (`crate::launch_reference_validation`) — so every row's setup writes it in IAM
/// ([`emr_service_role`]).
const SERVICE_ROLE: &str = "EMR_DefaultRole";

/// The release every cluster of this suite runs.
const RELEASE_LABEL: &str = "emr-6.15.0";

/// How long a poll waits for a condition: both provisioning phases of every row's cluster,
/// the spans of its two steps and the commands they perform, with room to spare on a loaded
/// host.
const DEADLINE: Duration = Duration::from_secs(180);

/// How long one look-again cycle pauses before asking again.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The states a step never leaves (`com.amazonaws.emr#StepState`).
const TERMINAL: [&str; 4] = ["COMPLETED", "CANCELLED", "FAILED", "INTERRUPTED"];

/// Amazon EMR, Amazon S3 and IAM on one engine: the cluster, the objects and the instance
/// profiles belong to ONE account, and one engine is what makes the three views that account.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_emr_elasticmapreduce::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router
}

/// One `awsJson1_1` Amazon EMR call that must be served.
async fn emr_ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let host = format!("elasticmapreduce.{REGION}.amazonaws.com");
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", host)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("ElasticMapReduce.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20261124/{REGION}/\
                 elasticmapreduce/aws4_request, SignedHeaders=host, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "{operation} must answer 200, got {status} with {payload}"
    );
    payload
}

/// One path-style Amazon S3 request, answered as `(status, body)`.
async fn s3(router: &MultiServiceRouter, method: &str, uri: &str, body: &[u8]) -> (u16, Vec<u8>) {
    let host = format!("s3.{REGION}.amazonaws.com");
    let request = ::http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{uri}"))
        .header("host", host.clone())
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20261124/{REGION}/s3/\
                 aws4_request, SignedHeaders=host, Signature=00"
            ),
        )
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let (parts, answered) = response.into_parts();
    (parts.status.as_u16(), answered.to_vec())
}

/// Create the bucket every row uses.
async fn create_bucket(router: &MultiServiceRouter) {
    let (status, body) = s3(router, "PUT", &format!("/{BUCKET}"), &[]).await;
    assert!(
        (200..300).contains(&status),
        "CreateBucket must succeed, got {status} {}",
        String::from_utf8_lossy(&body)
    );
}

/// Put `body` at `key`, which must be served.
async fn put_object(router: &MultiServiceRouter, key: &str, body: &[u8]) {
    let (status, answered) = s3(router, "PUT", &format!("/{BUCKET}/{key}"), body).await;
    assert!(
        (200..300).contains(&status),
        "PutObject {key} must succeed, got {status} {}",
        String::from_utf8_lossy(&answered)
    );
}

/// The object at `key`, or `None` when the store holds none.
async fn object(router: &MultiServiceRouter, key: &str) -> Option<String> {
    let (status, body) = s3(router, "GET", &format!("/{BUCKET}/{key}"), &[]).await;
    (200..300)
        .contains(&status)
        .then(|| String::from_utf8_lossy(&body).to_string())
}

/// One awsQuery IAM call of a row's setup, which must be served.
///
/// `pub(crate)` because the catalog-authorization suite builds its instance profiles the same
/// way (`crate::step_catalog_authorization`): the fixtures of this crate's tests are one
/// binary, so one reading of an IAM call serves both rather than a copy each.
pub(crate) async fn iam_ok(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(&str, String)],
) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encoded(name));
        body.push('=');
        body.push_str(&form_encoded(value));
    }
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20261124/{REGION}/iam/\
                 aws4_request, SignedHeaders=host, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let answered = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture {action} must succeed, got {status}: {answered}"
    );
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC 3986's
/// unreserved characters literal. `pub(crate)` for the reason [`iam_ok`] is.
pub(crate) fn form_encoded(value: &str) -> String {
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

/// The trust policy every fixture role carries: an instance obtains a credential for its
/// profile's role only where that role admits the Amazon EC2 service principal.
/// `pub(crate)` for the reason [`iam_ok`] is.
pub(crate) fn trust_policy() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "ec2.amazonaws.com" },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// One identity-policy document holding `statements`. `pub(crate)` for the reason [`iam_ok`]
/// is.
pub(crate) fn policy(statements: Vec<Value>) -> String {
    json!({ "Version": "2012-10-17", "Statement": statements }).to_string()
}

/// A statement of `effect` over `action` and `resource`. `pub(crate)` for the reason
/// [`iam_ok`] is.
pub(crate) fn statement(effect: &str, action: &str, resource: Value) -> Value {
    json!({ "Effect": effect, "Action": action, "Resource": resource })
}

/// The role Amazon EMR itself acts under for a cluster of this suite, written in IAM so the
/// resolution a launch's `ServiceRole` gets there finds it.
///
/// It carries the trust the service principal needs and no permissions policy: the clusters of
/// these suites create no Amazon EC2 resource that needs one, and the permissions under test
/// are the INSTANCE profile's. `pub(crate)` for the reason [`iam_ok`] is — every suite that
/// mounts IAM beside Amazon EMR needs this same fixture.
pub(crate) async fn emr_service_role(router: &MultiServiceRouter, name: &str) {
    iam_ok(
        router,
        name,
        "CreateRole",
        &[
            ("RoleName", name.to_string()),
            (
                "AssumeRolePolicyDocument",
                json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": { "Service": "elasticmapreduce.amazonaws.com" },
                        "Action": "sts:AssumeRole",
                    }],
                })
                .to_string(),
            ),
        ],
    )
    .await;
}

/// A grant of every action of Amazon S3 over the bucket and everything in it, which is what
/// the rows that deny ONE request are written against.
fn grant_of_the_service() -> Value {
    statement(
        "Allow",
        "s3:*",
        json!([
            format!("arn:aws:s3:::{BUCKET}"),
            format!("arn:aws:s3:::{BUCKET}/*"),
        ]),
    )
}

/// The ARN of one object of the bucket: an Amazon S3 object ARN names no Region and no
/// account, so it is the bucket and the key alone.
fn object_arn(key: &str) -> Value {
    json!(format!("arn:aws:s3:::{BUCKET}/{key}"))
}

/// What one step of a row must come to.
#[derive(Clone, Copy)]
struct Expected {
    /// The `StepState` it must settle in.
    state: &'static str,
    /// Whether the marker its chained stage writes must exist afterwards.
    marker: bool,
}

/// One row: the statement in force on the role the cluster's instances run as, and what each
/// of its two steps must come to.
struct Row {
    case: &'static str,
    /// Names the row's role, its instance profile and the keys of its markers.
    name: &'static str,
    /// The inline identity policy the role carries, or `None` for a role carrying none.
    policy: Option<String>,
    /// The step reading the prefix the rows deny the read over.
    reading_denied_prefix: Expected,
    /// The step reading the other prefix, on the SAME cluster.
    reading_allowed_prefix: Expected,
}

/// The key the marker of the step that read `prefix` is written to, under the row's own name.
fn marker_key(name: &str, prefix: &str) -> String {
    format!("{name}/{prefix}-ok.txt")
}

/// The table, in the order the clusters are launched.
fn rows() -> Vec<Row> {
    let completed = |marker| Expected {
        state: "COMPLETED",
        marker,
    };
    let failed = Expected {
        state: "FAILED",
        marker: false,
    };
    vec![
        Row {
            case: "a role granted every action of the service over the bucket",
            name: "granted",
            policy: Some(policy(vec![grant_of_the_service()])),
            reading_denied_prefix: completed(true),
            reading_allowed_prefix: completed(true),
        },
        Row {
            case: "a role granted the service and denied the object read over one input prefix",
            name: "read-denied",
            policy: Some(policy(vec![
                grant_of_the_service(),
                statement(
                    "Deny",
                    "s3:GetObject",
                    object_arn(&format!("{DENIED_PREFIX}/*")),
                ),
            ])),
            reading_denied_prefix: failed,
            reading_allowed_prefix: completed(true),
        },
        Row {
            case: "a role denied the object read over a prefix neither step reads",
            name: "denied-elsewhere",
            policy: Some(policy(vec![
                grant_of_the_service(),
                statement("Deny", "s3:GetObject", object_arn("some-other-prefix/*")),
            ])),
            reading_denied_prefix: completed(true),
            reading_allowed_prefix: completed(true),
        },
        Row {
            case: "a role granted the service and denied the object write of one marker",
            name: "write-denied",
            policy: Some(policy(vec![
                grant_of_the_service(),
                statement(
                    "Deny",
                    "s3:PutObject",
                    object_arn(&marker_key("write-denied", DENIED_PREFIX)),
                ),
            ])),
            reading_denied_prefix: failed,
            reading_allowed_prefix: completed(true),
        },
        Row {
            case: "a role carrying no identity policy at all",
            name: "no-policy",
            policy: None,
            reading_denied_prefix: completed(true),
            reading_allowed_prefix: completed(true),
        },
    ]
}

/// Build the instance profile one row's cluster runs as, the way a client builds one, and
/// answer the name the launch names it by.
async fn instance_profile(router: &MultiServiceRouter, row: &Row) -> String {
    let name = format!("emr-ec2-{}", row.name);
    iam_ok(
        router,
        row.case,
        "CreateRole",
        &[
            ("RoleName", name.clone()),
            ("AssumeRolePolicyDocument", trust_policy()),
        ],
    )
    .await;
    if let Some(document) = row.policy.as_ref() {
        iam_ok(
            router,
            row.case,
            "PutRolePolicy",
            &[
                ("RoleName", name.clone()),
                ("PolicyName", "step-input".to_string()),
                ("PolicyDocument", document.clone()),
            ],
        )
        .await;
    }
    iam_ok(
        router,
        row.case,
        "CreateInstanceProfile",
        &[("InstanceProfileName", name.clone())],
    )
    .await;
    iam_ok(
        router,
        row.case,
        "AddRoleToInstanceProfile",
        &[
            ("InstanceProfileName", name.clone()),
            ("RoleName", name.clone()),
        ],
    )
    .await;
    name
}

/// The command line one step submits: read the input of `prefix` through the cluster's own
/// Hadoop filesystem shell, and only then write the marker that says the read happened.
fn step_config(name: &str, prefix: &str) -> Value {
    let program = format!(
        "hadoop fs -cat s3://{BUCKET}/{prefix}/data.txt > /tmp/{prefix} && \
         echo {MARKER} | aws s3 cp - s3://{BUCKET}/{}",
        marker_key(name, prefix)
    );
    json!({
        "Name": format!("read-{prefix}"),
        "ActionOnFailure": "CONTINUE",
        "HadoopJarStep": {
            "Jar": "command-runner.jar",
            "Args": ["bash", "-c", program],
        },
    })
}

/// Launch one row's cluster: the two steps, run under the profile the row built, with no
/// reason to stay up once the queue has run.
async fn launch(router: &MultiServiceRouter, row: &Row, profile: &str) -> String {
    let launched = emr_ok(
        router,
        "RunJobFlow",
        json!({
            "Name": row.case,
            "ReleaseLabel": RELEASE_LABEL,
            "JobFlowRole": profile,
            "ServiceRole": SERVICE_ROLE,
            "Applications": [{ "Name": "Hadoop" }],
            "Steps": [
                step_config(row.name, DENIED_PREFIX),
                step_config(row.name, ALLOWED_PREFIX),
            ],
            "Instances": {
                "InstanceCount": 1,
                "MasterInstanceType": "m5.xlarge",
                "KeepJobFlowAliveWhenNoSteps": false,
            },
        }),
    )
    .await;
    launched["JobFlowId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {:?}: RunJobFlow answers a JobFlowId", row.case))
        .to_string()
}

/// Poll `ListSteps` until every step of `cluster_id` is terminal, answering them in submission
/// order.
async fn drained(case: &str, router: &MultiServiceRouter, cluster_id: &str) -> Vec<Value> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let listed = emr_ok(router, "ListSteps", json!({ "ClusterId": cluster_id })).await;
        let mut steps: Vec<Value> = listed["Steps"].as_array().cloned().unwrap_or_default();
        steps.reverse();
        let states: Vec<String> = steps
            .iter()
            .map(|step| {
                step["Status"]["State"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        if states.len() == 2 && states.iter().all(|state| TERMINAL.contains(&state.as_str())) {
            return steps;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected both steps to reach a terminal state, got {states:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One step of one row: it settled where the permissions in force on the role put it, and its
/// chained marker exists exactly when its command ran through.
async fn assert_step(
    case: &str,
    router: &MultiServiceRouter,
    name: &str,
    prefix: &str,
    expected: Expected,
    step: &Value,
) {
    let state = step["Status"]["State"].as_str().unwrap_or_default();
    assert_eq!(
        state, expected.state,
        "case {case:?}: expected the step reading {prefix} to settle {}, got {step}",
        expected.state
    );
    if expected.state == "FAILED" {
        let reason = step["Status"]["FailureDetails"]["Reason"]
            .as_str()
            .unwrap_or_default();
        assert_eq!(
            reason, COMMAND_FAILED_REASON,
            "case {case:?}: expected the command's own failure reason on the step reading \
             {prefix}, got {step}"
        );
        let message = step["Status"]["FailureDetails"]["Message"]
            .as_str()
            .unwrap_or_default();
        assert!(
            !message.is_empty(),
            "case {case:?}: expected a failure message on the step reading {prefix}, got {step}"
        );
    } else {
        assert!(
            step["Status"]["FailureDetails"].is_null(),
            "case {case:?}: a completed step reports no failure, got {step}"
        );
    }
    let key = marker_key(name, prefix);
    let held = object(router, &key).await;
    let wanted = expected.marker.then(|| MARKER_BODY.to_string());
    assert_eq!(
        held, wanted,
        "case {case:?}: expected {key} to hold {wanted:?}, got {held:?}"
    );
}

/// Every row: a step's Amazon S3 request is authorized against the permissions in force on the
/// role its cluster's instances run as, an explicit deny is final, and a step whose request was
/// refused FAILS with nothing its chained stage would have written.
#[tokio::test]
async fn the_instance_roles_permissions_decide_what_a_steps_command_may_read() {
    let router = emulator();
    let rows = rows();

    emr_service_role(&router, SERVICE_ROLE).await;
    create_bucket(&router).await;
    for prefix in [DENIED_PREFIX, ALLOWED_PREFIX] {
        put_object(
            &router,
            &format!("{prefix}/data.txt"),
            INPUT_BODY.as_bytes(),
        )
        .await;
    }

    // Every row's cluster is launched first, so the rows settle alongside each other rather
    // than one after the next.
    let mut launched: Vec<(usize, String)> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let profile = instance_profile(&router, row).await;
        launched.push((index, launch(&router, row, &profile).await));
    }

    for (index, cluster_id) in &launched {
        let row = &rows[*index];
        let steps = drained(row.case, &router, cluster_id).await;
        assert_step(
            row.case,
            &router,
            row.name,
            DENIED_PREFIX,
            row.reading_denied_prefix,
            &steps[0],
        )
        .await;
        assert_step(
            row.case,
            &router,
            row.name,
            ALLOWED_PREFIX,
            row.reading_allowed_prefix,
            &steps[1],
        )
        .await;
    }
}
