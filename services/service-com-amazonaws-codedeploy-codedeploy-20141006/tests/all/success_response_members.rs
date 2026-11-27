// SPDX-License-Identifier: CC0-1.0

//! What a successful CodeDeploy answer carries for an output member whose value
//! the call already knows, driven over the wire.
//!
//! RULE under test: a success SERIALIZES every output member whose value the
//! call knows, the known-empty value included, rather than leaving the member
//! out — so a client reads one member instead of telling absent from empty:
//!
//! * a batch reader's batch-level `errorMessage`
//!   (`smithy_query(query="shape",shape="BatchGetApplicationRevisionsOutput")`
//!   and `...shape="BatchGetDeploymentInstancesOutput"`: "Information about
//!   errors that might have occurred during the API call") is the EMPTY string
//!   on a batch no part of which failed;
//! * the `hooksNotCleanedUp` list of the two deployment-group writers that
//!   report it (`smithy_query(query="shape",shape="DeleteDeploymentGroupOutput")`
//!   and `...shape="UpdateDeploymentGroupOutput"`) is the EMPTY list when no
//!   Auto Scaling lifecycle hook was left behind;
//! * a deployment group a read answers reports only the members its modelled
//!   shape declares.
//!
//! The rows of each table differ along what the rule generalises over — the
//! SHAPE and SIZE of the batch, which members the group was created with, and
//! the compute platform of the application — and every case goes through the
//! same front door a client uses, the shared [`Emulator`] harness of
//! [`super::codedeploy_lifecycle`].

use serde_json::{json, Value};

use service_com_amazonaws_codedeploy_codedeploy_20141006 as codedeploy;

use crate::codedeploy_lifecycle::{s3_revision, Emulator};
use crate::deployment_targets::deployable;
use crate::list_deployment_targets::{deploy, group_with, register};

/// The service role every deployment group here is created with.
const ROLE: &str = "arn:aws:iam::123456789012:role/CodeDeployRole";

/// The batch-level no-error text: the member is present and empty whenever the
/// batch itself was answered.
const NO_BATCH_ERROR: &str = "";

/// Assert that `body` reports the no-error `errorMessage`, as a string rather
/// than an absent member.
fn assert_no_batch_error(body: &Value, case: &str) {
    assert_eq!(
        body["errorMessage"], NO_BATCH_ERROR,
        "case {case:?}: expected errorMessage {NO_BATCH_ERROR:?}, got {:?} in {body:?}",
        body["errorMessage"]
    );
    assert!(
        body["errorMessage"].is_string(),
        "case {case:?}: expected errorMessage to be reported as a string, got {:?} in {body:?}",
        body["errorMessage"]
    );
}

/// Assert that `body` reports `hooksNotCleanedUp` as the empty list.
fn assert_no_hooks_left(body: &Value, case: &str) {
    assert_eq!(
        body["hooksNotCleanedUp"],
        json!([]),
        "case {case:?}: expected hooksNotCleanedUp [], got {:?} in {body:?}",
        body["hooksNotCleanedUp"]
    );
}

/// RULE: a `BatchGetApplicationRevisions` that was answered reports the
/// batch-level `errorMessage` as the empty string.
///
/// Rows differ along the SHAPE and SIZE of the batch — one revision, several, a
/// repeated one, one the application never registered, a mix, and a batch right
/// at the request-size quota — which is what the rule generalises over: a batch
/// that was answered carries the no-error text whatever it asked for.
#[tokio::test]
async fn an_answered_revision_batch_reports_the_no_error_message() {
    let emulator = Emulator::new();
    let application = "batch-error-message";
    emulator
        .ok("CreateApplication", json!({"applicationName": application}))
        .await;

    let registered = s3_revision("artifacts", "app/one.zip");
    let also_registered = s3_revision("artifacts", "app/two.zip");
    for revision in [&registered, &also_registered] {
        emulator
            .ok(
                "RegisterApplicationRevision",
                json!({"applicationName": application, "revision": revision}),
            )
            .await;
    }
    let unregistered = s3_revision("artifacts", "app/never-registered.zip");
    // The largest batch this service answers, from the quota its
    // `BatchLimitExceededException` states (`codedeploy::state::MAX_BATCH_SIZE`).
    let at_the_quota: Vec<Value> = (0..codedeploy::state::MAX_BATCH_SIZE)
        .map(|index| s3_revision("artifacts", &format!("app/bulk-{index:02}.zip")))
        .collect();

    let cases: &[(&str, Vec<Value>)] = &[
        ("one registered revision", vec![registered.clone()]),
        (
            "two registered revisions",
            vec![registered.clone(), also_registered.clone()],
        ),
        (
            "the same revision twice",
            vec![registered.clone(), registered.clone()],
        ),
        ("a revision never registered", vec![unregistered.clone()]),
        (
            "a registered and an unregistered revision",
            vec![registered.clone(), unregistered.clone()],
        ),
        ("a batch at the request-size quota", at_the_quota),
    ];

    for (case, revisions) in cases {
        let body = emulator
            .ok(
                "BatchGetApplicationRevisions",
                json!({"applicationName": application, "revisions": revisions}),
            )
            .await;
        assert_no_batch_error(&body, case);
        // The answer still carries one entry per revision asked about, so the
        // no-error text is not standing in for a batch that answered nothing.
        let answered = body["revisions"].as_array().map(Vec::len).unwrap_or_default();
        assert_eq!(
            answered,
            revisions.len(),
            "case {case:?}: expected {} revision entries, got {answered} in {body:?}",
            revisions.len()
        );
    }
}

/// RULE: the other batch reader of the same member,
/// `BatchGetDeploymentInstances`, reports it the same way.
///
/// Rows differ along the SHAPE of the batch over one deployment's instances:
/// every instance, a subset, reordered, repeated, and mixed with an id the
/// deployment holds nothing under.
#[tokio::test]
async fn an_answered_instance_batch_reports_the_no_error_message() {
    let emulator = Emulator::new();
    let application = "batch-instance-error-message";
    for instance in ["host-one", "host-two"] {
        register(&emulator, instance, &[("Fleet", "deployable")]).await;
    }
    register(&emulator, "host-three", &[("Fleet", "retired")]).await;
    group_with(&emulator, application, "Server", &deployable()).await;
    let deployment_id = deploy(&emulator, application).await;

    let cases: &[(&str, &[&str])] = &[
        ("every resolved instance", &["host-one", "host-two"]),
        ("one of them", &["host-two"]),
        ("reordered", &["host-two", "host-one"]),
        ("the same instance twice", &["host-one", "host-one"]),
        (
            "mixed with an instance the deployment holds nothing under",
            &["host-one", "host-three"],
        ),
    ];

    for (case, instance_ids) in cases {
        let body = emulator
            .ok(
                "BatchGetDeploymentInstances",
                json!({"deploymentId": deployment_id, "instanceIds": instance_ids}),
            )
            .await;
        assert_no_batch_error(&body, case);
        assert!(
            body["instancesSummary"].is_array(),
            "case {case:?}: expected instancesSummary to be reported as a list, got {body:?}"
        );
    }
}

/// One deployment group to write and read: the case name, the application's
/// compute platform, and the members `CreateDeploymentGroup` is given beyond the
/// name and the service role.
type GroupCase<'a> = (&'a str, &'a str, Value);

/// The group rows every table below spans: the platform and the members a group
/// was created with are what the rules generalise over.
fn group_cases() -> Vec<GroupCase<'static>> {
    vec![
        ("nothing beyond the required members, Server", "Server", json!({})),
        ("nothing beyond the required members, Lambda", "Lambda", json!({})),
        ("nothing beyond the required members, ECS", "ECS", json!({})),
        (
            "on-premises tag filters",
            "Server",
            json!({"onPremisesInstanceTagFilters": [{"Key": "Fleet", "Value": "blue", "Type": "KEY_AND_VALUE"}]}),
        ),
        (
            "EC2 tag filters and a trigger",
            "Server",
            json!({
                "ec2TagFilters": [{"Key": "Stage", "Value": "prod", "Type": "KEY_AND_VALUE"}],
                "triggerConfigurations": [{
                    "triggerName": "all-events",
                    "triggerTargetArn": "arn:aws:sns:us-east-1:123456789012:codedeploy",
                    "triggerEvents": ["DeploymentSuccess"],
                }],
            }),
        ),
        (
            "Auto Scaling groups",
            "Server",
            json!({"autoScalingGroups": ["asg-one", "asg-two"]}),
        ),
        (
            "a blue/green style with traffic control",
            "Server",
            json!({
                "deploymentStyle": {
                    "deploymentType": "BLUE_GREEN",
                    "deploymentOption": "WITH_TRAFFIC_CONTROL",
                },
            }),
        ),
    ]
}

/// Create the application and the deployment group of one row, answering the
/// group's name.
async fn seed_group(emulator: &Emulator, application: &str, platform: &str, extras: &Value) -> String {
    emulator
        .ok(
            "CreateApplication",
            json!({"applicationName": application, "computePlatform": platform}),
        )
        .await;
    let group = format!("{application}-dg");
    let mut request = json!({
        "applicationName": application,
        "deploymentGroupName": group,
        "serviceRoleArn": ROLE,
    });
    for (member, value) in extras.as_object().cloned().unwrap_or_default() {
        request[member] = value;
    }
    crate::codedeploy_lifecycle::require_platform_members(platform, &mut request);
    emulator.ok("CreateDeploymentGroup", request).await;
    group
}

/// RULE: both writers that report `hooksNotCleanedUp` answer it as the empty
/// list when no Auto Scaling lifecycle hook was left behind — whatever the group
/// they acted on carried.
#[tokio::test]
async fn a_group_write_reports_an_empty_left_behind_hook_list() {
    let emulator = Emulator::new();
    for (index, (case, platform, extras)) in group_cases().into_iter().enumerate() {
        let application = format!("hook-list-{index:02}");
        let group = seed_group(&emulator, &application, platform, &extras).await;

        let updated = emulator
            .ok(
                "UpdateDeploymentGroup",
                json!({
                    "applicationName": application,
                    "currentDeploymentGroupName": group,
                    "serviceRoleArn": ROLE,
                }),
            )
            .await;
        assert_no_hooks_left(&updated, case);

        let deleted = emulator
            .ok(
                "DeleteDeploymentGroup",
                json!({"applicationName": application, "deploymentGroupName": group}),
            )
            .await;
        assert_no_hooks_left(&deleted, case);

        // Deleting a group nothing holds is the same success with the same
        // member: the empty list is about hooks, not about what was removed.
        let again = emulator
            .ok(
                "DeleteDeploymentGroup",
                json!({"applicationName": application, "deploymentGroupName": group}),
            )
            .await;
        assert_no_hooks_left(&again, case);
    }
}

/// Every member `DeploymentGroupInfo` declares, by the name it travels under.
///
/// The catalog is the shape's own member list, from
/// `smithy_query(query="shape",shape="DeploymentGroupInfo")`.
const MODELLED_GROUP_MEMBERS: &[&str] = &[
    "applicationName",
    "deploymentGroupId",
    "deploymentGroupName",
    "deploymentConfigName",
    "ec2TagFilters",
    "onPremisesInstanceTagFilters",
    "autoScalingGroups",
    "serviceRoleArn",
    "targetRevision",
    "triggerConfigurations",
    "alarmConfiguration",
    "autoRollbackConfiguration",
    "deploymentStyle",
    "outdatedInstancesStrategy",
    "blueGreenDeploymentConfiguration",
    "loadBalancerInfo",
    "lastSuccessfulDeployment",
    "lastAttemptedDeployment",
    "ec2TagSet",
    "onPremisesTagSet",
    "computePlatform",
    "ecsServices",
    "ec2DeploymentOptions",
    "terminationHookEnabled",
];

/// The members a group read reports for EVERY group, whatever it was created
/// with: the ones the create required or defaulted, so the subset check below
/// cannot pass on an answer that reports nothing at all.
const ALWAYS_REPORTED_GROUP_MEMBERS: &[&str] = &[
    "applicationName",
    "deploymentGroupId",
    "deploymentGroupName",
    "deploymentConfigName",
    "serviceRoleArn",
    "computePlatform",
];

/// RULE: a success reports only members the output shape DECLARES — a key no
/// member of that shape carries is reported by no read, so a client decodes the
/// answer against the shape it holds and meets nothing else.
///
/// The rows are the group table above, which spans the compute platform and the
/// members a group was created with, and each row is read both as created and
/// after an update, because an update rewrites the stored group.
#[tokio::test]
async fn a_deployment_group_reports_only_its_modelled_members() {
    let emulator = Emulator::new();

    /// The `deploymentGroupInfo` `GetDeploymentGroup` answers.
    async fn read(emulator: &Emulator, application: &str, group: &str) -> Value {
        emulator
            .ok(
                "GetDeploymentGroup",
                json!({"applicationName": application, "deploymentGroupName": group}),
            )
            .await["deploymentGroupInfo"]
            .clone()
    }

    fn assert_only_modelled(info: &Value, case: &str) {
        let reported = info
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: expected a group document, got {info:?}"));
        for member in reported.keys() {
            assert!(
                MODELLED_GROUP_MEMBERS.contains(&member.as_str()),
                "case {case:?}: {member:?} is no member of DeploymentGroupInfo, got {info:?}"
            );
        }
        for member in ALWAYS_REPORTED_GROUP_MEMBERS {
            assert!(
                reported.contains_key(*member),
                "case {case:?}: expected {member:?} to be reported, got {info:?}"
            );
        }
    }

    for (index, (case, platform, extras)) in group_cases().into_iter().enumerate() {
        let application = format!("group-members-{index:02}");
        let group = seed_group(&emulator, &application, platform, &extras).await;

        let created = read(&emulator, &application, &group).await;
        assert_only_modelled(&created, case);

        let renamed = format!("{group}-renamed");
        emulator
            .ok(
                "UpdateDeploymentGroup",
                json!({
                    "applicationName": application,
                    "currentDeploymentGroupName": group,
                    "newDeploymentGroupName": renamed,
                }),
            )
            .await;
        let after_update = read(&emulator, &application, &renamed).await;
        assert_only_modelled(&after_update, case);
    }
}
