// SPDX-License-Identifier: CC0-1.0

//! The `DeploymentInfo` members a reader SUMMARISES or DEFAULTS, over the wire.
//!
//! RULE under test: every `DeploymentInfo` a reader answers carries, for the
//! deployment it answers about,
//!
//! * `deploymentOverview`, "a summary of the deployment status of the instances
//!   in the deployment"
//!   (`smithy_query(query="doc",shape="DeploymentInfo",member="deploymentOverview")`)
//!   — all six counts, each `@default 0`
//!   (`smithy_query(query="shape",shape="DeploymentOverview")`), counting the
//!   deployment's OWN targets under the status they hold, so the summary never
//!   disagrees with the target readers;
//! * `deploymentStatusMessages`, as the list the deployment has — the empty list
//!   for a deployment carrying no status message — rather than an absent member;
//! * `fileExistsBehavior`, the policy the deployment runs under: the value its
//!   `CreateDeployment` asked for, or `DISALLOW`, which the model documents as
//!   "also the default behavior if no option is specified".
//!
//! Both readers of a deployment record answer the same projection, so every case
//! here is asserted through `GetDeployment` AND `BatchGetDeployments`.
//!
//! The harness — the router, the `X-Amz-Target` request, the fleet helpers and
//! the terminal-status poll — is the one `codedeploy_lifecycle` and
//! `list_deployment_targets` drive the rest of this service with; only the cases
//! here are new.

use serde_json::{json, Value};

use service_com_amazonaws_codedeploy_codedeploy_20141006 as codedeploy;

use crate::codedeploy_lifecycle::{s3_revision, terminal_deployment, Emulator};
use crate::list_deployment_targets::{deploy, force_status, group_with, register};

/// The service role every deployment group here is created with; CodeDeploy only
/// stores it.
const SERVICE_ROLE: &str = "arn:aws:iam::123456789012:role/CodeDeployRole";

/// The one deployment group name the fleet helpers create.
const GROUP: &str = "dg";

/// The policy a deployment that asked for none runs under: the model documents
/// this value as "also the default behavior if no option is specified"
/// (`smithy_query(query="doc",shape="DeploymentInfo",member="fileExistsBehavior")`).
const DEFAULT_FILE_POLICY: &str = "DISALLOW";

/// The six counts of an overview, as the model names them on the wire
/// (`smithy_query(query="shape",shape="DeploymentOverview")`).
const OVERVIEW_COUNTS: &[&str] = &[
    "Pending",
    "InProgress",
    "Succeeded",
    "Failed",
    "Skipped",
    "Ready",
];

/// The summary a deployment of `targets` targets has when every one of them is
/// in the state `bucket` names: that count under `bucket`, and 0 under each of
/// the other five.
fn overview_of(bucket: &str, targets: usize) -> Value {
    let mut expected = Value::Object(serde_json::Map::new());
    for count in OVERVIEW_COUNTS {
        expected[*count] = json!(0);
    }
    expected[bucket] = json!(targets);
    expected
}

/// A selector every on-premises instance tagged `Fleet=<fleet>` satisfies, and
/// nothing else.
fn fleet_filter(fleet: &str) -> Value {
    json!({
        "onPremisesInstanceTagFilters": [
            {"Key": "Fleet", "Value": fleet, "Type": "KEY_AND_VALUE"},
        ]
    })
}

/// Register `hosts` on-premises instances in a fleet of this application's own
/// and create an EC2/on-premises application and deployment group selecting
/// exactly that fleet.
async fn given_fleet(emulator: &Emulator, application: &str, hosts: usize) {
    for index in 0..hosts {
        register(
            emulator,
            &format!("{application}-host-{index}"),
            &[("Fleet", application)],
        )
        .await;
    }
    group_with(emulator, application, "Server", &fleet_filter(application)).await;
}

/// [`given_fleet`] with a deployment to that group; answers the deployment id.
async fn deployment_over(emulator: &Emulator, application: &str, hosts: usize) -> String {
    given_fleet(emulator, application, hosts).await;
    deploy(emulator, application).await
}

/// An `appSpecContent` revision carrying `content`: the form an Lambda
/// deployment's AppSpec travels in.
fn app_spec_revision(content: &str) -> Value {
    json!({"revisionType": "AppSpecContent", "appSpecContent": {"content": content}})
}

/// The AppSpec of an Lambda deployment, as the AWS CodeDeploy User Guide writes
/// one: one `Resources` entry naming the function, its alias and the two versions
/// traffic moves between.
fn app_spec_yaml(function: &str) -> String {
    format!(
        "version: 0.0\n\
         Resources:\n\
         \x20 - {function}:\n\
         \x20     Type: AWS::Lambda::Function\n\
         \x20     Properties:\n\
         \x20       Name: {function}\n\
         \x20       Alias: live\n\
         \x20       CurrentVersion: \"1\"\n\
         \x20       TargetVersion: \"2\"\n"
    )
}

/// The hex SHA-256 of `content`, as a client computes the digest of the revision
/// it sent.
fn digest_of(content: &str) -> String {
    use ::sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    ::hex::encode(hasher.finalize())
}

/// Create an Lambda application with one all-at-once deployment group and deploy
/// `revision` to it; answers the deployment id.
async fn lambda_deployment(emulator: &Emulator, application: &str, revision: Value) -> String {
    emulator
        .ok(
            "CreateApplication",
            json!({"applicationName": application, "computePlatform": "Lambda"}),
        )
        .await;
    emulator
        .ok(
            "CreateDeploymentGroup",
            json!({
                "applicationName": application,
                "deploymentGroupName": GROUP,
                "deploymentConfigName": "CodeDeployDefault.LambdaAllAtOnce",
                "serviceRoleArn": SERVICE_ROLE,
            }),
        )
        .await;
    emulator
        .ok(
            "CreateDeployment",
            json!({
                "applicationName": application,
                "deploymentGroupName": GROUP,
                "revision": revision,
            }),
        )
        .await["deploymentId"]
        .as_str()
        .expect("CreateDeployment answers a deployment id")
        .to_string()
}

/// The `deploymentInfo` of `deployment_id` as each reader of a deployment record
/// answers it, named by the reader, so one case asserts both.
async fn read_by_every_reader(
    emulator: &Emulator,
    deployment_id: &str,
) -> Vec<(&'static str, Value)> {
    let single = emulator
        .ok("GetDeployment", json!({"deploymentId": deployment_id}))
        .await["deploymentInfo"]
        .clone();
    let batch = emulator
        .ok(
            "BatchGetDeployments",
            json!({"deploymentIds": [deployment_id]}),
        )
        .await["deploymentsInfo"][0]
        .clone();
    vec![("GetDeployment", single), ("BatchGetDeployments", batch)]
}

/// Assert that `info` reports the summary `expected`, with all six counts as
/// numbers.
fn assert_overview(info: &Value, expected: &Value, case: &str) {
    assert_eq!(
        &info["deploymentOverview"], expected,
        "case {case:?}: expected deploymentOverview {expected}, got {:?} in {info:?}",
        info["deploymentOverview"]
    );
    for count in OVERVIEW_COUNTS {
        assert!(
            info["deploymentOverview"][*count].is_number(),
            "case {case:?}: expected deploymentOverview.{count} to be reported as a number, got \
             {:?}",
            info["deploymentOverview"][*count]
        );
    }
}

/// Assert that `info` reports `deploymentStatusMessages` as a list, empty for a
/// deployment carrying no status message.
fn assert_no_status_messages(info: &Value, case: &str) {
    assert_eq!(
        info["deploymentStatusMessages"],
        json!([]),
        "case {case:?}: expected deploymentStatusMessages [], got {:?} in {info:?}",
        info["deploymentStatusMessages"]
    );
}

/// Assert that `info` reports `fileExistsBehavior` as `expected`.
fn assert_file_policy(info: &Value, expected: &str, case: &str) {
    assert_eq!(
        info["fileExistsBehavior"], expected,
        "case {case:?}: expected fileExistsBehavior {expected:?}, got {:?} in {info:?}",
        info["fileExistsBehavior"]
    );
}

/// How many target ids `ListDeploymentTargets` answers for a started deployment:
/// the targets the deployment's record holds, which the summary counts.
async fn listed_targets(emulator: &Emulator, deployment_id: &str) -> usize {
    emulator
        .ok(
            "ListDeploymentTargets",
            json!({"deploymentId": deployment_id}),
        )
        .await["targetIds"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

/// RULE: the summary counts the deployment's own targets under the status they
/// hold, with every other count 0 — whichever reader answers.
///
/// Rows differ along what the rule generalises over: how many targets the
/// deployment's group resolved (none, one, several) and which `DeploymentStatus`
/// the record holds, the statuses real CodeDeploy reaches that no call this
/// emulator serves produces included.
#[tokio::test]
async fn a_deployment_summarises_its_own_targets_under_their_status() {
    let emulator = Emulator::new();

    // (case, the stored DeploymentStatus, whether it has started, the count the
    // targets are summarised under)
    let statuses: &[(&str, &str, bool, &str)] = &[
        ("created, not started yet", "Created", false, "Pending"),
        ("queued, not started yet", "Queued", false, "Pending"),
        ("in progress", "InProgress", true, "InProgress"),
        ("baking", "Baking", true, "InProgress"),
        ("succeeded", "Succeeded", true, "Succeeded"),
        ("failed", "Failed", true, "Failed"),
        ("stopped by a caller", "Stopped", true, "Skipped"),
    ];

    for hosts in [0_usize, 1, 2, 3] {
        let application = format!("overview-fleet-{hosts}");
        let deployment_id = deployment_over(&emulator, &application, hosts).await;

        for (status_case, status, started, bucket) in statuses {
            force_status(&emulator, &deployment_id, status, *started);
            let expected = overview_of(bucket, hosts);
            for (reader, info) in read_by_every_reader(&emulator, &deployment_id).await {
                let case = format!("{hosts} target(s), {status_case}, read by {reader}");
                assert_overview(&info, &expected, &case);
                assert_no_status_messages(&info, &case);
            }
            // The summary is one view of the target state the target readers
            // answer from, so its counts add up to the targets the deployment
            // holds. A deployment that has not started is refused by that
            // reader, which is another suite's rule.
            if *started {
                let listed = listed_targets(&emulator, &deployment_id).await;
                assert_eq!(
                    listed, hosts,
                    "case {hosts} target(s), {status_case}: expected the summary to count the \
                     listed targets, got {listed} listed"
                );
            }
        }
    }
}

/// RULE: the summary a deployment ends with counts the targets it actually
/// reached, with nothing forced into the record.
///
/// Rows differ along the compute platform and the fleet the group resolved: an
/// EC2/on-premises group selecting several instances, one selecting none (which
/// is why CodeDeploy fails such a deployment), and an Lambda group, whose
/// deployment has exactly one target — the function its AppSpec names.
#[tokio::test]
async fn the_summary_a_terminal_deployment_reports_counts_the_targets_it_reached() {
    let emulator = Emulator::new();

    let two_hosts = deployment_over(&emulator, "overview-end-two", 2).await;
    let no_host = deployment_over(&emulator, "overview-end-none", 0).await;
    let app_spec = app_spec_yaml("overview-end-function");
    let lambda = lambda_deployment(
        &emulator,
        "overview-end-lambda",
        app_spec_revision(&app_spec),
    )
    .await;

    // (case, deployment id, the status it ends in, the summary it ends with)
    let cases: &[(&str, &str, &str, Value)] = &[
        (
            "two on-premises instances deployed to",
            &two_hosts,
            "Succeeded",
            overview_of("Succeeded", 2),
        ),
        (
            "a group that selected no instance",
            &no_host,
            "Failed",
            overview_of("Failed", 0),
        ),
        (
            "an Lambda deployment of a function that does not exist",
            &lambda,
            "Failed",
            overview_of("Failed", 1),
        ),
    ];

    for (case, deployment_id, status, expected) in cases {
        let polled = terminal_deployment(&emulator, deployment_id, case).await;
        assert_eq!(
            polled["status"], *status,
            "case {case:?}: expected the deployment to end as {status:?}, got {:?}",
            polled["status"]
        );
        for (reader, info) in read_by_every_reader(&emulator, deployment_id).await {
            let case = format!("{case}, read by {reader}");
            assert_overview(&info, expected, &case);
            assert_no_status_messages(&info, &case);
            // No request here named a file-handling policy, so every one of
            // these deployments runs under the documented default.
            assert_file_policy(&info, DEFAULT_FILE_POLICY, &case);
        }
    }

    // The Lambda deployment's revision carries the digest of the AppSpec the
    // request sent, on every read of that revision.
    let expected = digest_of(&app_spec);
    for (reader, info) in read_by_every_reader(&emulator, &lambda).await {
        assert_eq!(
            info["revision"]["appSpecContent"]["sha256"], expected,
            "case {reader:?}: expected the revision's sha256 {expected:?}, got {:?} in {info:?}",
            info["revision"]["appSpecContent"]["sha256"]
        );
    }
}

/// RULE: a deployment reports the file-handling policy it runs under — the value
/// its `CreateDeployment` asked for, or CodeDeploy's documented default when the
/// request asked for none.
///
/// Rows differ along what the request stated: every value the model's
/// `FileExistsBehavior` enum carries (the generated catalogue
/// `codedeploy::ops::types::FILE_EXISTS_BEHAVIOR_VALUES`, which is where these
/// values come from), and a request that states nothing.
#[tokio::test]
async fn a_deployment_reports_the_file_policy_it_runs_under() {
    let emulator = Emulator::new();

    let mut cases: Vec<(String, Option<&str>, &str)> = vec![(
        "a request that names no policy".to_string(),
        None,
        DEFAULT_FILE_POLICY,
    )];
    for value in codedeploy::ops::types::FILE_EXISTS_BEHAVIOR_VALUES {
        cases.push((format!("a request that names {value}"), Some(value), value));
    }

    for (index, (case, requested, expected)) in cases.iter().enumerate() {
        let application = format!("file-policy-{index:02}");
        // One instance, so the deployment has a target of its own beside the
        // policy under test.
        given_fleet(&emulator, &application, 1).await;
        let mut request = json!({
            "applicationName": application,
            "deploymentGroupName": GROUP,
            "revision": s3_revision("my-bucket", "app.zip"),
        });
        if let Some(requested) = requested {
            request["fileExistsBehavior"] = json!(requested);
        }
        let deployment_id = emulator.ok("CreateDeployment", request).await["deploymentId"]
            .as_str()
            .expect("CreateDeployment answers a deployment id")
            .to_string();

        for (reader, info) in read_by_every_reader(&emulator, &deployment_id).await {
            let read_case = format!("{case}, read by {reader}");
            assert_file_policy(&info, expected, &read_case);
            assert_no_status_messages(&info, &read_case);
        }

        // The policy is carried on the record, so it survives the deployment
        // running to its terminal status rather than being minted by one read.
        let polled = terminal_deployment(&emulator, &deployment_id, case).await;
        assert_file_policy(&polled, expected, &format!("{case}, once terminal"));
        assert_overview(&polled, &overview_of("Succeeded", 1), case);
    }
}
