// SPDX-License-Identifier: CC0-1.0

//! WHOSE permissions decide whether a cluster's log files land under its `LogUri`, end to end
//! over the wire.
//!
//! THE RULE: the files a cluster's logs are pushed to Amazon S3 by its NODES — "LogUri: the
//! location in Amazon S3 to write the log files of the job flow" (`RunJobFlow$LogUri` doc),
//! written with what those instances hold, which is the role of the instance profile the launch
//! named ("Also called instance profile and Amazon EC2 role. An IAM role for an Amazon EMR
//! cluster. The Amazon EC2 instances of the cluster assume this role",
//! `RunJobFlow$JobFlowRole` doc). So the permissions in force on THAT role are what a file's
//! write is decided against: a statement denying `s3:PutObject` on the key a file would be
//! written under suppresses it — the prefix stays empty however many calls report the cluster —
//! while a role no readable statement denies it for gets its files. The step itself runs and
//! ends either way: what the role governs is the log delivery, not the work.
//!
//! The rows differ along what the rule generalises over: the statement in force on the push
//! role (none at all, a grant, an explicit deny of the object write over the cluster's own log
//! prefix, that same deny beside a grant of everything, a deny over ANOTHER cluster's prefix,
//! and a deny of every action of the service over the whole bucket).
//!
//! IAM, Amazon S3 and Amazon EMR are mounted on ONE engine: each profile is built the way a
//! client builds one (`CreateRole`, `PutRolePolicy`, `CreateInstanceProfile`,
//! `AddRoleToInstanceProfile`), and every log object is read back through S3's own
//! `ListObjectsV2` — so what is asserted is what a client's own reads find.
//!
//! Nothing here waits a fixed time: a cluster's phases and its step's span are stored due
//! instants, so every wait LOOKS AGAIN until the condition holds or the generous [`DEADLINE`]
//! passes. The rows whose files must NOT appear are polled only after every row that must get
//! its files has them, so each refused delivery has had at least as many rounds as a delivered
//! one — which is what makes "the prefix is empty" an answer rather than a race.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_emr_elasticmapreduce::state::STEP_LOG_OBJECTS;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every service is addressed in: the one whose `CreateBucket` needs no location
/// constraint in its body.
const REGION: &str = "us-east-1";

/// The bucket every row's log files would land in.
const BUCKET: &str = "emr-log-permission-boundary";

/// The service role every launch names. Amazon EMR resolves it against IAM once the launch has
/// been accepted, so the setup below writes it in IAM
/// (`crate::step_input_read_authorization::emr_service_role`).
const SERVICE_ROLE: &str = "EMR_DefaultRole";

/// How long a poll waits for a condition: both provisioning phases of six clusters, their
/// steps' spans and the deliveries that follow, with room to spare on a loaded host.
const DEADLINE: Duration = Duration::from_secs(180);

/// How long one look-again cycle pauses before asking again.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The states a cluster never leaves (`com.amazonaws.emr#ClusterState`).
const TERMINAL_CLUSTER: [&str; 2] = ["TERMINATED", "TERMINATED_WITH_ERRORS"];

/// What the step of every row prints, which is the whole of its work.
const MARKER: &str = "emr-log-permission-boundary";

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
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20261109/{REGION}/\
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
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20261109/{REGION}/s3/\
                 aws4_request, SignedHeaders=host, Signature=00"
            ),
        )
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let (parts, answered) = response.into_parts();
    (parts.status.as_u16(), answered.to_vec())
}

/// The keys of every object under `prefix`, from `ListObjectsV2`.
async fn keys_under(router: &MultiServiceRouter, prefix: &str) -> Vec<String> {
    let (status, listing) = s3(
        router,
        "GET",
        &format!("/{BUCKET}?list-type=2&prefix={prefix}"),
        &[],
    )
    .await;
    let listing = String::from_utf8_lossy(&listing).to_string();
    assert_eq!(
        status, 200,
        "listing s3://{BUCKET}/{prefix} must succeed, got {status} with {listing}"
    );
    listing
        .split("<Key>")
        .skip(1)
        .filter_map(|rest| rest.split_once("</Key>").map(|(key, _)| key.to_string()))
        .collect()
}

/// One awsQuery IAM call, answered as `(status, body)`.
async fn iam(router: &MultiServiceRouter, action: &str, params: &[(&str, String)]) -> (u16, String) {
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
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20261109/{REGION}/iam/\
                 aws4_request, SignedHeaders=host, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8(response.body().to_vec()).expect("a UTF-8 response body");
    (status, body)
}

/// One IAM call of a row's setup, which must be served.
async fn iam_ok(router: &MultiServiceRouter, case: &str, action: &str, params: &[(&str, String)]) {
    let (status, body) = iam(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture {action} must succeed, got {status}: {body}"
    );
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC 3986's
/// unreserved characters literal.
fn form_encoded(value: &str) -> String {
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
fn trust_policy() -> String {
    "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\"Principal\":\
     {\"Service\":\"ec2.amazonaws.com\"},\"Action\":\"sts:AssumeRole\"}]}"
        .to_string()
}

/// One statement, as an inline identity policy document.
fn statement(effect: &str, action: &str, resource: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"{effect}\",\
         \"Action\":\"{action}\",\"Resource\":\"{resource}\"}}]}}"
    )
}

/// A grant of every object write over the bucket, beside a deny of the one over `prefix`:
/// IAM's order, where an explicit deny is final.
fn granted_then_denied(prefix: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[\
         {{\"Effect\":\"Allow\",\"Action\":\"s3:PutObject\",\
         \"Resource\":\"arn:aws:s3:::{BUCKET}/*\"}},\
         {{\"Effect\":\"Deny\",\"Action\":\"s3:PutObject\",\
         \"Resource\":\"arn:aws:s3:::{BUCKET}/{prefix}/*\"}}]}}"
    )
}

/// One row: the key prefix a cluster's `LogUri` names, the statement in force on the role its
/// nodes push with, and whether its log files have to land.
struct Row {
    case: &'static str,
    /// The prefix inside [`BUCKET`], which is also the name the row's role and profile take.
    prefix: &'static str,
    /// The inline identity policy the push role carries, or `None` for a role carrying none.
    policy: Option<String>,
    /// Whether the step's log directory has to appear under the prefix.
    delivered: bool,
}

/// The table, in the order the clusters are launched.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a role carrying no permissions policy at all",
            prefix: "no-policy",
            policy: None,
            delivered: true,
        },
        Row {
            case: "a role granted the object write over the bucket",
            prefix: "granted",
            policy: Some(statement(
                "Allow",
                "s3:PutObject",
                &format!("arn:aws:s3:::{BUCKET}/*"),
            )),
            delivered: true,
        },
        Row {
            case: "a role denied the object write over its own log prefix",
            prefix: "denied-own",
            policy: Some(statement(
                "Deny",
                "s3:PutObject",
                &format!("arn:aws:s3:::{BUCKET}/denied-own/*"),
            )),
            delivered: false,
        },
        Row {
            case: "a role granted every object write and denied its own log prefix",
            prefix: "denied-beside-a-grant",
            policy: Some(granted_then_denied("denied-beside-a-grant")),
            delivered: false,
        },
        Row {
            case: "a role denied the object write over ANOTHER cluster's log prefix",
            prefix: "denied-elsewhere",
            policy: Some(statement(
                "Deny",
                "s3:PutObject",
                &format!("arn:aws:s3:::{BUCKET}/some-other-prefix/*"),
            )),
            delivered: true,
        },
        Row {
            case: "a role denied every action of the service over the bucket",
            prefix: "denied-wholesale",
            policy: Some(statement(
                "Deny",
                "s3:*",
                &format!("arn:aws:s3:::{BUCKET}/*"),
            )),
            delivered: false,
        },
    ]
}

/// Build the instance profile one row's cluster runs as, the way a client builds one, and
/// answer the name the launch names it by.
async fn instance_profile(router: &MultiServiceRouter, row: &Row) -> String {
    let name = format!("emr-ec2-{}", row.prefix);
    iam_ok(
        router,
        row.case,
        "CreateRole",
        &[("RoleName", name.clone()), ("AssumeRolePolicyDocument", trust_policy())],
    )
    .await;
    if let Some(document) = row.policy.as_ref() {
        iam_ok(
            router,
            row.case,
            "PutRolePolicy",
            &[
                ("RoleName", name.clone()),
                ("PolicyName", "log-delivery".to_string()),
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
        &[("InstanceProfileName", name.clone()), ("RoleName", name.clone())],
    )
    .await;
    name
}

/// Launch one row's cluster: a trivial command step, its own `LogUri`, and no reason to stay
/// up once the queue has run.
async fn launch(router: &MultiServiceRouter, row: &Row, profile: &str) -> String {
    let launched = emr_ok(
        router,
        "RunJobFlow",
        json!({
            "Name": row.case,
            "ReleaseLabel": "emr-6.15.0",
            "LogUri": format!("s3://{BUCKET}/{}/", row.prefix),
            "JobFlowRole": profile,
            "ServiceRole": SERVICE_ROLE,
            "Steps": [{
                "Name": "print a line",
                "ActionOnFailure": "CONTINUE",
                "HadoopJarStep": {
                    "Jar": "command-runner.jar",
                    "Args": ["bash", "-c", format!("echo {MARKER}")],
                },
            }],
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
        .expect("RunJobFlow answers a JobFlowId")
        .to_string()
}

/// Poll `DescribeCluster` until `cluster_id` is terminal, answering the state it settled in.
///
/// Describing the cluster is also what a client does while it waits, and what the delivery is
/// owed by, so this is the same traffic the real wait makes.
async fn terminated(case: &str, router: &MultiServiceRouter, cluster_id: &str) -> String {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let described = emr_ok(router, "DescribeCluster", json!({ "ClusterId": cluster_id })).await;
        let state = described["Cluster"]["Status"]["State"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if TERMINAL_CLUSTER.contains(&state.as_str()) {
            return state;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the cluster to reach a terminal state, got {state:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Poll until the step log directory of `cluster_id` holds every file one holds, answering the
/// keys it holds then.
async fn delivered(case: &str, router: &MultiServiceRouter, cluster_id: &str, prefix: &str) -> Vec<String> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let keys = keys_under(router, prefix).await;
        if keys.len() >= STEP_LOG_OBJECTS.len() {
            return keys;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the step's log files under {prefix}, got {keys:?}"
        );
        // A client learns the files are there by reading the cluster, which is what the
        // delivery is owed by; the listing is then what reports them.
        emr_ok(router, "DescribeCluster", json!({ "ClusterId": cluster_id })).await;
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The state every step of `cluster_id` settled in, in submission order.
async fn step_states(router: &MultiServiceRouter, cluster_id: &str) -> Vec<String> {
    let listed = emr_ok(router, "ListSteps", json!({ "ClusterId": cluster_id })).await;
    let mut steps: Vec<Value> = listed["Steps"].as_array().cloned().unwrap_or_default();
    steps.reverse();
    steps
        .iter()
        .map(|step| {
            step["Status"]["State"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Every row: a cluster's log files land under its `LogUri` exactly when the permissions in
/// force on the role its nodes push with do not deny the object write — and its step runs and
/// ends either way.
#[tokio::test]
async fn the_push_roles_permissions_decide_whether_a_clusters_logs_land() {
    let router = emulator();
    let rows = rows();

    crate::step_input_read_authorization::emr_service_role(&router, SERVICE_ROLE).await;

    let (status, answered) = s3(&router, "PUT", &format!("/{BUCKET}"), &[]).await;
    assert!(
        (200..300).contains(&status),
        "CreateBucket must succeed, got {status} {}",
        String::from_utf8_lossy(&answered)
    );

    // Every row's cluster is launched first, so each row's delivery gets every round the
    // waits below make.
    let mut launched: Vec<(usize, String)> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let profile = instance_profile(&router, row).await;
        launched.push((index, launch(&router, row, &profile).await));
    }

    for (index, cluster_id) in &launched {
        let row = &rows[*index];
        let state = terminated(row.case, &router, cluster_id).await;
        assert!(
            TERMINAL_CLUSTER.contains(&state.as_str()),
            "case {:?}: expected a terminal cluster state, got {state:?}",
            row.case
        );
        // The role governs the log delivery, not the work: the step ran and completed
        // whatever its permissions say about the object write.
        let states = step_states(&router, cluster_id).await;
        assert_eq!(
            states,
            vec!["COMPLETED".to_string()],
            "case {:?}: expected the submitted step to complete, got {states:?}",
            row.case
        );
    }

    // The rows whose files must land are waited for FIRST: by the time the last of them has
    // its directory, every refused delivery has had at least as many rounds.
    for (index, cluster_id) in &launched {
        let row = &rows[*index];
        if !row.delivered {
            continue;
        }
        let prefix = format!("{}/{cluster_id}/", row.prefix);
        let keys = delivered(row.case, &router, cluster_id, &prefix).await;
        assert_eq!(
            keys.len(),
            STEP_LOG_OBJECTS.len(),
            "case {:?}: expected exactly {} files under {prefix}, got {keys:?}",
            row.case,
            STEP_LOG_OBJECTS.len()
        );
        for name in STEP_LOG_OBJECTS {
            assert!(
                keys.iter().any(|key| key.ends_with(name)),
                "case {:?}: expected a {name} under {prefix}, got {keys:?}",
                row.case
            );
        }
    }

    for (index, cluster_id) in &launched {
        let row = &rows[*index];
        if row.delivered {
            continue;
        }
        // One more round for each refused cluster, so its absence is not merely a delivery
        // that has not been reached yet.
        emr_ok(&router, "DescribeCluster", json!({ "ClusterId": cluster_id })).await;
        let prefix = format!("{}/{cluster_id}/", row.prefix);
        let keys = keys_under(&router, &prefix).await;
        assert!(
            keys.is_empty(),
            "case {:?}: expected nothing under {prefix}, got {keys:?}",
            row.case
        );
    }

    // Nor does a refused delivery leak into another row's prefix: the whole bucket holds
    // exactly the files of the rows whose write was not denied.
    let held = keys_under(&router, "").await;
    let expected = rows.iter().filter(|row| row.delivered).count() * STEP_LOG_OBJECTS.len();
    assert_eq!(
        held.len(),
        expected,
        "expected {expected} log objects in the bucket, got {held:?}"
    );
}
