// SPDX-License-Identifier: CC0-1.0

//! What a canary RUN reports, and when a started canary stops itself — end to end over the
//! wire, with the services that own the resources a run uses mounted beside Synthetics.
//!
//! The rules asserted here, each over a SET of inputs rather than over one recorded call:
//!
//! 1. **A run's verdict is the outcome of the work the run performed.** The work this
//!    emulator performs for a run is the artifact write the run is judged by: the run PASSES
//!    when its artifact was stored at the canary's `ArtifactS3Location` — and the object is
//!    then readable through Amazon S3's own API — and FAILS with `EXECUTION_FAILURE` when it
//!    could not be, which is the reason code the model documents for exactly that failure
//!    ("a non-critical failure occurred such as failing to save generated debug artifacts",
//!    `smithy_query(query="doc",shape="CanaryRunStatus",member="StateReasonCode")`). Whether
//!    the run MAY write is IAM's answer about the canary's execution role.
//!
//! 2. **A started canary stops itself once its schedule's bound on how long it runs has
//!    passed.** `CanaryScheduleInput$Expression`: "Specifying `rate(0 minute)` or
//!    `rate(0 hour)` is a special value that causes the canary to run only once when it is
//!    started", so such a canary settles to `STOPPED` once that single run has finished,
//!    while a schedule with no such bound holds `RUNNING` until `StopCanary`.
//!
//! 3. **A state-transition refusal names the transition and the canary's current state, and
//!    no identifier.** One sentence for `StartCanary`, `StopCanary` and `DeleteCanary`.

use serde_json::{json, Value};

use crate::canary_lifecycle::{poll_until, Answer, Synthetics};
use crate::group_membership::{error_of, ok};

/// The Region every call in this suite is signed for, and the Region the bucket and the
/// role below live in.
const REGION: &str = "us-east-1";

/// The bucket a canary's artifacts are written to, created through Amazon S3's own API.
const BUCKET: &str = "canary-artifacts";

/// The object a run's report is stored as. The layout is the emulator's own and is stated
/// where it is chosen (`src/state/canary_run.rs`).
const REPORT_OBJECT: &str = "SyntheticsReport.json";

/// The action a run needs on the artifacts it writes.
const ARTIFACT_WRITE_ACTION: &str = "s3:PutObject";

/// The service principal a canary's execution role is assumed by.
const SYNTHETICS_PRINCIPAL: &str = "synthetics.amazonaws.com";

/// A trust policy that lets CloudWatch Synthetics assume the role.
fn trust_document() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": SYNTHETICS_PRINCIPAL },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// Create `role_name` through IAM itself, with `statements` as its one inline permissions
/// policy (none at all when the list is empty), and answer the ARN IAM minted for it.
async fn create_role(iam: &::aws_sdk_iam::Client, role_name: &str, statements: Value) -> String {
    let arn = iam
        .create_role()
        .role_name(role_name)
        .assume_role_policy_document(trust_document())
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup: IAM must create the role {role_name}: {error:?}"))
        .role()
        .map(|role| role.arn().to_string())
        .unwrap_or_else(|| panic!("setup: IAM must report an ARN for {role_name}"));
    if statements.as_array().is_some_and(|list| !list.is_empty()) {
        iam.put_role_policy()
            .role_name(role_name)
            .policy_name("canary-run")
            .policy_document(
                json!({ "Version": "2012-10-17", "Statement": statements }).to_string(),
            )
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!("setup: IAM must attach the policy of {role_name}: {error:?}")
            });
    }
    arn
}

/// One policy statement: `effect` the `actions` on `resources`.
fn statement(effect: &str, actions: Value, resources: Value) -> Value {
    json!({ "Effect": effect, "Action": actions, "Resource": resources })
}

/// Create the suite's bucket through Amazon S3's own API, so the objects asserted on below
/// exist only because S3 holds them.
async fn create_bucket(s3: &::aws_sdk_s3::Client) {
    s3.create_bucket()
        .bucket(BUCKET)
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup: the bucket {BUCKET} must be created: {error:?}"));
}

/// Every object key the store holds under `prefix`.
async fn keys_under(s3: &::aws_sdk_s3::Client, prefix: &str) -> Vec<String> {
    s3.list_objects_v2()
        .bucket(BUCKET)
        .prefix(prefix)
        .send()
        .await
        .unwrap_or_else(|error| panic!("the object store must answer a listing: {error:?}"))
        .contents()
        .iter()
        .filter_map(|object| object.key().map(str::to_string))
        .collect()
}

/// The body of one object, or `None` when the store serves no such object.
async fn object_body(s3: &::aws_sdk_s3::Client, key: &str) -> Option<Value> {
    let object = s3.get_object().bucket(BUCKET).key(key).send().await.ok()?;
    let bytes = object.body.collect().await.ok()?.into_bytes();
    ::serde_json::from_slice(&bytes).ok()
}

/// A `CreateCanary` body: every required member, with the row's own role, artifact location
/// and schedule.
fn canary_body(name: &str, role_arn: &str, location: &str, schedule: &str) -> Value {
    json!({
        "Name": name,
        "Code": { "Handler": "index.handler", "S3Bucket": "scripts", "S3Key": "canary.zip" },
        "ArtifactS3Location": location,
        "ExecutionRoleArn": role_arn,
        "Schedule": { "Expression": schedule },
        "RuntimeVersion": "syn-nodejs-puppeteer-9.0",
    })
}

/// Create the canary and wait for the provisioning transition the clock owes it, so the
/// start below is not refused for a canary that is still `CREATING`.
async fn ready_canary(aws: &Synthetics, case: &str, body: Value) {
    let name = body["Name"].as_str().unwrap_or_default().to_string();
    ok(&format!("{case}/create"), aws.create(body).await);
    let canary = poll_until(
        &format!("{case}/ready"),
        || aws.get(&name),
        |answer| answer.string_at("Canary.Status.State") == "READY",
    )
    .await;
    assert_eq!(
        canary.string_at("Canary.Status.State"),
        "READY",
        "case {case}/ready: expected READY, got {:?}",
        canary.at("Canary.Status")
    );
}

/// Start the canary and wait until its newest run is no longer `RUNNING`, then answer that
/// run.
async fn finished_run(aws: &Synthetics, case: &str, name: &str) -> Value {
    ok(&format!("{case}/start"), aws.start(name).await);
    let answer = poll_until(
        &format!("{case}/runs"),
        || aws.runs(name, json!({})),
        |answer| {
            answer.at("CanaryRuns")[0]["Status"]["State"]
                .as_str()
                .is_some_and(|state| state != "RUNNING")
        },
    )
    .await;
    answer.at("CanaryRuns")[0].clone()
}

/// RULE: a run passes when the artifacts it is judged by were stored at the canary's
/// artifact location, and the object the run wrote is then readable through Amazon S3.
///
/// The rows differ along what the rule generalises over: the form of the artifact location
/// (a bare bucket, one key prefix, a nested prefix) and the form of the grant the execution
/// role carries (every action, the action on every object of the bucket, the action on the
/// objects under the location's own prefix).
#[tokio::test]
async fn a_run_passes_and_stores_its_artifact_when_the_role_permits_the_write() {
    let (aws, iam, s3) = Synthetics::with_account_services(REGION);
    create_bucket(&s3).await;

    let rows: [(&str, &str, String, Value); 4] = [
        (
            "bucket only, every action allowed",
            "bare-bucket",
            format!("s3://{BUCKET}"),
            json!([statement("Allow", json!("*"), json!("*"))]),
        ),
        (
            "one prefix, the write allowed on the bucket's objects",
            "one-prefix",
            format!("s3://{BUCKET}/canary"),
            json!([statement(
                "Allow",
                json!(ARTIFACT_WRITE_ACTION),
                json!(format!("arn:aws:s3:::{BUCKET}/*")),
            )]),
        ),
        (
            "nested prefix, the write allowed under that prefix",
            "nested-prefix",
            format!("s3://{BUCKET}/team/canaries"),
            json!([statement(
                "Allow",
                json!(ARTIFACT_WRITE_ACTION),
                json!(format!("arn:aws:s3:::{BUCKET}/team/canaries/*")),
            )]),
        ),
        (
            "prefix with a trailing slash, several actions allowed",
            "trailing-slash",
            format!("s3://{BUCKET}/trailing/"),
            json!([statement(
                "Allow",
                json!([ARTIFACT_WRITE_ACTION, "logs:PutLogEvents"]),
                json!(format!("arn:aws:s3:::{BUCKET}/*")),
            )]),
        ),
    ];

    for (case, name, location, statements) in rows {
        let role = create_role(&iam, &format!("{name}-role"), statements).await;
        ready_canary(&aws, case, canary_body(name, &role, &location, "rate(5 minutes)")).await;
        let run = finished_run(&aws, case, name).await;

        assert_eq!(
            run["Status"]["State"].as_str(),
            Some("PASSED"),
            "case {case}: expected PASSED, got {:?}",
            run["Status"]
        );
        assert!(
            run["Status"]["StateReason"].is_null() && run["Status"]["StateReasonCode"].is_null(),
            "case {case}: a passing run carries no failure reason, got {:?}",
            run["Status"]
        );

        // The artifact exists because Amazon S3 holds it: one object, under the location's
        // own prefix, in a folder of the canary and the run.
        let run_id = run["Id"].as_str().unwrap_or_default().to_string();
        let prefix = location
            .strip_prefix(&format!("s3://{BUCKET}"))
            .unwrap_or_default()
            .trim_start_matches('/')
            .trim_end_matches('/');
        let folder = if prefix.is_empty() {
            format!("{name}/{run_id}")
        } else {
            format!("{prefix}/{name}/{run_id}")
        };
        let keys = keys_under(&s3, &folder).await;
        assert_eq!(
            keys,
            vec![format!("{folder}/{REPORT_OBJECT}")],
            "case {case}: expected the run's one artifact under {folder}, got {keys:?}"
        );
        let document = object_body(&s3, &format!("{folder}/{REPORT_OBJECT}"))
            .await
            .unwrap_or_else(|| panic!("case {case}: the object store must serve the artifact"));
        assert_eq!(
            document["canaryName"].as_str(),
            Some(name),
            "case {case}: expected the artifact to report its canary, got {document}"
        );
        assert_eq!(
            document["runId"].as_str(),
            Some(run_id.as_str()),
            "case {case}: expected the artifact to report its run, got {document}"
        );
    }
}

/// RULE: a run whose artifacts could not be stored fails with the `EXECUTION_FAILURE`
/// reason code and stores nothing — whichever way the execution role withholds the write.
///
/// The rows differ along what the rule generalises over: a role that states no permissions
/// at all, one whose permissions are about something else, one that allows the write on
/// another bucket's objects, and one that denies it outright.
#[tokio::test]
async fn a_run_fails_with_an_execution_failure_when_the_role_withholds_the_write() {
    let (aws, iam, s3) = Synthetics::with_account_services(REGION);
    create_bucket(&s3).await;

    let rows: [(&str, &str, Value); 4] = [
        ("no permissions at all", "no-grant", json!([])),
        (
            "permissions about another service",
            "other-service",
            json!([statement("Allow", json!("logs:PutLogEvents"), json!("*"))]),
        ),
        (
            "the write allowed on another bucket",
            "other-bucket",
            json!([statement(
                "Allow",
                json!(ARTIFACT_WRITE_ACTION),
                json!("arn:aws:s3:::somebody-elses-bucket/*"),
            )]),
        ),
        (
            "the write denied outright",
            "denied",
            json!([
                statement("Allow", json!("*"), json!("*")),
                statement(
                    "Deny",
                    json!(ARTIFACT_WRITE_ACTION),
                    json!(format!("arn:aws:s3:::{BUCKET}/*")),
                ),
            ]),
        ),
    ];

    for (case, name, statements) in rows {
        let role = create_role(&iam, &format!("{name}-role"), statements).await;
        let location = format!("s3://{BUCKET}/{name}");
        ready_canary(&aws, case, canary_body(name, &role, &location, "rate(5 minutes)")).await;
        let run = finished_run(&aws, case, name).await;

        assert_eq!(
            run["Status"]["State"].as_str(),
            Some("FAILED"),
            "case {case}: expected FAILED, got {:?}",
            run["Status"]
        );
        assert_eq!(
            run["Status"]["StateReasonCode"].as_str(),
            Some("EXECUTION_FAILURE"),
            "case {case}: expected EXECUTION_FAILURE, got {:?}",
            run["Status"]
        );
        assert!(
            run["Status"]["StateReason"]
                .as_str()
                .is_some_and(|reason| !reason.is_empty()),
            "case {case}: expected a reason for the failure, got {:?}",
            run["Status"]
        );
        let keys = keys_under(&s3, name).await;
        assert!(
            keys.is_empty(),
            "case {case}: expected nothing stored for a run that could not write, got {keys:?}"
        );
    }
}

/// RULE: a started canary whose schedule runs it ONCE settles to `STOPPED` on its own once
/// that single run has finished; a schedule with no such bound holds `RUNNING` until
/// `StopCanary`.
///
/// The rows differ along the schedule expression: both spellings of the run-once value, and
/// the rate and cron forms that keep the canary running.
#[tokio::test]
async fn a_canary_stops_itself_only_when_its_schedule_runs_it_once() {
    let aws = Synthetics::new(REGION);
    let role = "arn:aws:iam::123456789012:role/CanaryRole";

    let rows: [(&str, &str, &str, bool); 5] = [
        ("run once, minutes", "once-minute", "rate(0 minute)", true),
        ("run once, hours", "once-hour", "rate(0 hour)", true),
        ("every minute", "every-minute", "rate(1 minute)", false),
        ("every five minutes", "every-five", "rate(5 minutes)", false),
        ("a cron schedule", "cron-schedule", "cron(0/10 * * * ? *)", false),
    ];

    for (case, name, schedule, stops_itself) in rows {
        ready_canary(
            &aws,
            case,
            canary_body(name, role, &format!("s3://{BUCKET}/{name}"), schedule),
        )
        .await;
        // The single run has to be finished before the canary can be: the run's outcome is
        // the condition the stop waits on.
        let run = finished_run(&aws, case, name).await;
        assert!(
            run["Timeline"]["Completed"].is_number(),
            "case {case}: expected the run to have completed, got {run}"
        );

        if stops_itself {
            let canary = poll_until(
                &format!("{case}/stopped"),
                || aws.get(name),
                |answer| answer.string_at("Canary.Status.State") == "STOPPED",
            )
            .await;
            assert_eq!(
                canary.string_at("Canary.Status.State"),
                "STOPPED",
                "case {case}: expected a run-once canary to settle to STOPPED, got {:?}",
                canary.at("Canary.Status")
            );
            assert!(
                canary.at("Canary.Timeline.LastStopped").is_number(),
                "case {case}: expected LastStopped to be set, got {:?}",
                canary.at("Canary.Timeline")
            );
            // A canary that has stopped itself cannot be stopped again, and can be started
            // again.
            let stop = aws.stop(name).await;
            assert_eq!(
                stop.status, 409,
                "case {case}: expected a stop of a stopped canary to conflict, got {} ({:?})",
                stop.status, stop.body
            );
            ok(&format!("{case}/restart"), aws.start(name).await);
        } else {
            let canary = ok(&format!("{case}/running"), aws.get(name).await);
            assert_eq!(
                canary.string_at("Canary.Status.State"),
                "RUNNING",
                "case {case}: expected a recurring canary to hold RUNNING until it is \
                 stopped, got {:?}",
                canary.at("Canary.Status")
            );
            ok(&format!("{case}/stop"), aws.stop(name).await);
        }
    }
}

/// RULE: a state-transition refusal is ONE sentence naming the transition it will not make
/// and the canary's current state, and it names no identifier — the canary's own name is no
/// part of it.
///
/// The rows differ along the operation refused and the state that refuses it, including the
/// two states a transition is owed by the clock from.
#[tokio::test]
async fn a_state_transition_refusal_names_the_transition_and_the_state() {
    let aws = Synthetics::new(REGION);
    let role = "arn:aws:iam::123456789012:role/CanaryRole";
    let body = |name: &str| canary_body(name, role, &format!("s3://{BUCKET}/{name}"), "rate(5 minutes)");

    // A canary still being provisioned: the rows below act on it immediately, before the
    // transition out of CREATING comes due.
    ok("create/creating", aws.create(body("creating-canary")).await);
    let creating: [(&str, Answer, &str); 3] = [
        ("start-while-creating", aws.start("creating-canary").await, "started"),
        ("stop-while-creating", aws.stop("creating-canary").await, "stopped"),
        (
            "delete-while-creating",
            aws.delete("creating-canary").await,
            "deleted",
        ),
    ];

    // A canary that is READY, and one that is RUNNING.
    ready_canary(&aws, "ready", body("ready-canary")).await;
    ready_canary(&aws, "running", body("running-canary")).await;
    ok("start/running", aws.start("running-canary").await);

    let mut rows: Vec<(&str, Answer, &str, &str)> = creating
        .into_iter()
        .map(|(case, answer, transition)| (case, answer, transition, "CREATING"))
        .collect();
    rows.push((
        "stop-while-ready",
        aws.stop("ready-canary").await,
        "stopped",
        "READY",
    ));
    rows.push((
        "start-while-running",
        aws.start("running-canary").await,
        "started",
        "RUNNING",
    ));
    rows.push((
        "delete-while-running",
        aws.delete("running-canary").await,
        "deleted",
        "RUNNING",
    ));

    for (case, answer, transition, state) in rows {
        let message = error_of(case, &answer, 409, "ConflictException")
            .unwrap_or_else(|| panic!("case {case}: expected a Message, got {:?}", answer.body));
        assert_eq!(
            message,
            format!("Canary is in a state that can't be {transition}: {state}"),
            "case {case}: expected the state-conflict sentence, got {message:?}"
        );
        assert!(
            !message.contains("canary-"),
            "case {case}: expected no canary name in the refusal, got {message:?}"
        );
    }
}
