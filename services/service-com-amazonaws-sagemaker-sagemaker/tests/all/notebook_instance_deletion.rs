// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's `DeleteNotebookInstance` and
//! `DeleteNotebookInstanceLifecycleConfig`, read back through the describes that resolve
//! what each of them removes.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so what
//! these tests exercise is what a real SDK client gets. The harness is this crate's shared
//! one, from `tests/all/inference_deployment_lifecycle.rs`, and the notebook-instance
//! request shape comes from the suite that creates them.
//!
//! The tests assert the RULES over a TABLE of inputs rather than one recorded call:
//!
//! * a notebook instance is deleted from a status in which nothing is running for it, and
//!   refused from every other — "before you can delete a notebook instance, you must call
//!   the StopNotebookInstance API" (`DeleteNotebookInstance`'s own doc), which makes the
//!   deletable statuses `Stopped` and `Failed` and every other one a refusal naming the
//!   status it is in;
//! * neither delete touches anything but the record it names, and a name the store does not
//!   hold is refused with the one sentence this service words that fact with;
//! * a lifecycle configuration is removed whatever it declares, and a second delete of it is
//!   refused.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

// The harness this crate's suites share, rather than another copy of it.
use super::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};
// The notebook-instance request shape is stated once, in the suite that creates them.
use super::notebook_instance_lifecycle::create_body;

/// How long a case waits for a status the clock already owes.
///
/// GENEROUS on purpose: what is under test is WHICH status the instance reaches, never how
/// fast it gets there, so each case polls the reader for the condition it needs and this is
/// only the deadline that stops a broken emulator from hanging the suite.
const STATUS_DEADLINE: Duration = Duration::from_secs(30);

/// How long a case waits between two reads while polling for a status.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// The ARN a notebook instance of `name` carries in `region`.
fn notebook_instance_arn(region: &str, name: &str) -> String {
    format!("arn:aws:sagemaker:{region}:{ACCOUNT}:notebook-instance/{name}")
}

/// The ARN a notebook instance lifecycle configuration of `name` carries in `region`.
fn lifecycle_config_arn(region: &str, name: &str) -> String {
    format!("arn:aws:sagemaker:{region}:{ACCOUNT}:notebook-instance-lifecycle-config/{name}")
}

/// Read the notebook instance `name` until it reports `wanted`, and answer the reading that
/// did.
///
/// The CONDITION is what is waited for, never a span: an instance the clock owes a
/// transition is read again until it reports the status the rule says it reaches.
async fn notebook_instance_resting_in(
    emulator: &Emulator,
    case: &str,
    name: &str,
    wanted: &str,
) -> Value {
    let deadline = Instant::now() + STATUS_DEADLINE;
    loop {
        let described = emulator
            .ok(
                "DescribeNotebookInstance",
                &json!({ "NotebookInstanceName": name }),
            )
            .await;
        let status = str_field(&described, "NotebookInstanceStatus");
        if status == wanted {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the instance to reach {wanted:?}, got {status:?} in \
             {described:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// `DeleteNotebookInstance` for `name`, in `region`.
async fn delete_notebook_instance(emulator: &Emulator, region: &str, name: &str) -> (u16, Value) {
    emulator
        .call_in(
            region,
            "DeleteNotebookInstance",
            &json!({ "NotebookInstanceName": name }),
        )
        .await
}

/// RULE: a notebook instance is deleted from a status in which no ML compute instance is
/// running for it — `Stopped`, and `Failed` for one whose provisioning never completed — and
/// a later `DescribeNotebookInstance` no longer resolves it, while every other instance of
/// the account is untouched.
#[tokio::test]
async fn deleting_a_stopped_notebook_instance_removes_only_that_instance() {
    // The rows differ along what the rule generalises over: the region the request is
    // signed for, and how much configuration the instance carries. Every row stops the
    // instance first, because that is what the rule requires; the other deletable status
    // (`Failed`, for an instance whose provisioning never completed) needs an IAM that
    // answers about the role, so it is exercised where one is mounted
    // (`core/router/tests/all/cloudformation_provisions_sagemaker_notebook_estate.rs`).
    let rows: Vec<(&str, &str, &str, Value)> = vec![
        ("a stopped instance", "Stopped", "us-east-1", json!({})),
        (
            "a stopped instance carrying a full configuration",
            "Stopped",
            "eu-west-1",
            json!({
                "SubnetId": "subnet-0abc1234",
                "SecurityGroupIds": ["sg-00000001"],
                "VolumeSizeInGB": 120,
                "RootAccess": "Disabled",
                "DirectInternetAccess": "Disabled",
            }),
        ),
        (
            "a stopped instance with a code repository and a lifecycle configuration",
            "Stopped",
            "ap-southeast-2",
            json!({
                "DefaultCodeRepository": "https://example.invalid/notebooks.git",
                "VolumeSizeInGB": 16384,
            }),
        ),
    ];

    for (index, (case, status_before_delete, region, extra)) in rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("deleted-notebook-{index}");
        // A second instance of the same account, which the delete must leave alone.
        let bystander = format!("kept-notebook-{index}");
        for instance in [name.as_str(), bystander.as_str()] {
            let (created_status, created) = emulator
                .call_in(
                    region,
                    "CreateNotebookInstance",
                    &create_body(instance, &extra),
                )
                .await;
            assert_eq!(
                created_status, 200,
                "case {case:?}: expected HTTP 200 from CreateNotebookInstance for \
                 {instance:?}, got {created_status} with {created}"
            );
        }
        if status_before_delete == "Stopped" {
            // The instance has to be stopped before SageMaker will take it away, which is
            // the rule under test: the stop is asked for and the clock's `Stopped`
            // transition is waited for by reading the instance back.
            let (stopped_status, stopped) = emulator
                .call_in(
                    region,
                    "StopNotebookInstance",
                    &json!({ "NotebookInstanceName": name }),
                )
                .await;
            assert_eq!(
                stopped_status, 200,
                "case {case:?}: expected HTTP 200 from StopNotebookInstance, got \
                 {stopped_status} with {stopped}"
            );
        }
        let before = notebook_instance_resting_in(&emulator, case, &name, status_before_delete)
            .await;
        assert_eq!(
            str_field(&before, "NotebookInstanceArn"),
            notebook_instance_arn(region, &name),
            "case {case:?}: expected the instance's own ARN before the delete, got {before:?}"
        );

        let (status, deleted) = delete_notebook_instance(&emulator, region, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteNotebookInstance, got {status} with \
             {deleted}"
        );

        // The record is gone: the reader refuses the name with the one sentence this
        // service words a notebook instance it does not hold with.
        let (status, gone) = emulator
            .call_in(
                region,
                "DescribeNotebookInstance",
                &json!({ "NotebookInstanceName": name }),
            )
            .await;
        assert_eq!(
            (status, error_code(&gone).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected the deleted instance to be gone, got {status} with {gone}"
        );
        assert!(
            message(&gone).contains(&notebook_instance_arn(region, &name)),
            "case {case:?}: expected the refusal to name the instance's ARN, got {:?}",
            message(&gone)
        );

        // ...and the instance the delete did not name is still there.
        let (status, kept) = emulator
            .call_in(
                region,
                "DescribeNotebookInstance",
                &json!({ "NotebookInstanceName": bystander }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the other instance to survive, got {status} with {kept}"
        );
    }
}

/// RULE: a notebook instance with an ML compute instance still running for it is NOT
/// deleted — "before you can delete a notebook instance, you must call the
/// StopNotebookInstance API" — so the delete is refused with a 400 `ValidationException`
/// naming the status it is in and the instance stays exactly as it was.
#[tokio::test]
async fn deleting_a_notebook_instance_that_is_still_running_is_refused() {
    let region = "us-east-1";
    // The rows are the statuses a delete is refused from, each reached the way a client
    // reaches it: an instance still coming up, one that is serving, and one whose stop is
    // still in flight.
    let rows: &[(&str, &str)] = &[
        ("an instance that is still coming up", "Pending"),
        ("an instance that is serving", "InService"),
        ("an instance whose stop is in flight", "Stopping"),
    ];

    for (index, (case, status_before_delete)) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("running-notebook-{index}");
        let (created_status, created) = emulator
            .call_in(
                region,
                "CreateNotebookInstance",
                &create_body(&name, &json!({})),
            )
            .await;
        assert_eq!(
            created_status, 200,
            "case {case:?}: expected HTTP 200 from CreateNotebookInstance, got \
             {created_status} with {created}"
        );
        if *status_before_delete == "Stopping" {
            // A stop asked for while the instance is still `Pending` leaves it `Stopping`
            // without it ever having come up, which is the third status a delete is refused
            // from.
            let (stopped_status, stopped) = emulator
                .call_in(
                    region,
                    "StopNotebookInstance",
                    &json!({ "NotebookInstanceName": name }),
                )
                .await;
            assert_eq!(
                stopped_status, 200,
                "case {case:?}: expected HTTP 200 from StopNotebookInstance, got \
                 {stopped_status} with {stopped}"
            );
        }
        if *status_before_delete == "InService" {
            notebook_instance_resting_in(&emulator, case, &name, "InService").await;
        }

        let (status, refused) = delete_notebook_instance(&emulator, region, &name).await;
        // The status the instance is in when the delete lands is read from the instance
        // itself: `Pending` is a status the clock moves on, so the refusal is judged
        // against whichever of the running statuses the reader reports.
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected the delete to be refused, got {status} with {refused}"
        );
        assert!(
            message(&refused).contains("not in (Stopped, Failed)")
                && message(&refused).contains("Deleting")
                && message(&refused).contains(&notebook_instance_arn(region, &name)),
            "case {case:?}: expected the refusal to name the statuses a delete needs, the \
             transition it could not make and the instance, got {:?}",
            message(&refused)
        );

        // The instance is untouched: it still resolves, and in a status no delete moved.
        let (status, still_there) = emulator
            .call_in(
                region,
                "DescribeNotebookInstance",
                &json!({ "NotebookInstanceName": name }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the refused delete to leave the instance, got {status} \
             with {still_there}"
        );
        assert_ne!(
            str_field(&still_there, "NotebookInstanceStatus"),
            "Deleting",
            "case {case:?}: expected no status moved by a refused delete, got {still_there:?}"
        );
    }
}

/// RULE: `DeleteNotebookInstance` and `DeleteNotebookInstanceLifecycleConfig` each refuse a
/// name their store holds nothing under, with a 400 `ValidationException` carrying the one
/// sentence this service words that fact with — whatever the name's shape.
#[tokio::test]
async fn deleting_a_record_that_is_not_there_is_refused() {
    let emulator = Emulator::new();
    let region = "us-west-2";
    // The names span the shapes the modelled patterns admit: the shortest, one carrying
    // hyphens, and the longest the `@length(max: 63)` bound allows.
    let longest = "n".repeat(63);
    for name in ["a", "never-created-record", longest.as_str()] {
        let (status, gone) = delete_notebook_instance(&emulator, region, name).await;
        assert_eq!(
            (status, error_code(&gone).as_str()),
            (400, "ValidationException"),
            "case {name:?}: expected a 400 for an absent notebook instance, got {status} with \
             {gone}"
        );
        assert!(
            message(&gone).contains(&notebook_instance_arn(region, name)),
            "case {name:?}: expected the refusal to name the instance's ARN, got {:?}",
            message(&gone)
        );

        let (status, no_config) = emulator
            .call_in(
                region,
                "DeleteNotebookInstanceLifecycleConfig",
                &json!({ "NotebookInstanceLifecycleConfigName": name }),
            )
            .await;
        assert_eq!(
            (status, error_code(&no_config).as_str()),
            (400, "ValidationException"),
            "case {name:?}: expected a 400 for an absent lifecycle configuration, got \
             {status} with {no_config}"
        );
        assert!(
            message(&no_config).contains(&lifecycle_config_arn(region, name)),
            "case {name:?}: expected the refusal to name the configuration's ARN, got {:?}",
            message(&no_config)
        );
    }
}

/// RULE: a notebook instance lifecycle configuration is removed whatever it declares, so a
/// later `DescribeNotebookInstanceLifecycleConfig` no longer resolves it and a second delete
/// of it is refused — while every other configuration of the account is untouched.
#[tokio::test]
async fn deleting_a_lifecycle_configuration_removes_only_that_configuration() {
    let region = "us-east-1";
    // The rows differ along what the rule generalises over: which of the two hook lists the
    // configuration declares, and whether it declares any at all.
    let rows: &[(&str, Value)] = &[
        ("both hooks", json!({
            "OnCreate": [{ "Content": "ZWNobyBvbi1jcmVhdGU=" }],
            "OnStart": [{ "Content": "ZWNobyBvbi1zdGFydA==" }],
        })),
        ("one hook", json!({ "OnStart": [{ "Content": "ZWNobyBvbi1zdGFydA==" }] })),
        ("no hook at all", json!({})),
    ];

    for (index, (case, hooks)) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("deleted-config-{index}");
        let bystander = format!("kept-config-{index}");
        for config in [name.as_str(), bystander.as_str()] {
            let mut body = json!({ "NotebookInstanceLifecycleConfigName": config });
            for (member, value) in hooks.as_object().expect("a JSON object of hooks") {
                body[member] = value.clone();
            }
            let (status, created) = emulator
                .call_in(region, "CreateNotebookInstanceLifecycleConfig", &body)
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from \
                 CreateNotebookInstanceLifecycleConfig for {config:?}, got {status} with \
                 {created}"
            );
        }

        let (status, deleted) = emulator
            .call_in(
                region,
                "DeleteNotebookInstanceLifecycleConfig",
                &json!({ "NotebookInstanceLifecycleConfigName": name }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from \
             DeleteNotebookInstanceLifecycleConfig, got {status} with {deleted}"
        );

        let (status, gone) = emulator
            .call_in(
                region,
                "DescribeNotebookInstanceLifecycleConfig",
                &json!({ "NotebookInstanceLifecycleConfigName": name }),
            )
            .await;
        assert_eq!(
            (status, error_code(&gone).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected the deleted configuration to be gone, got {status} with \
             {gone}"
        );
        // A second delete of the same configuration has nothing left to remove, and is
        // refused with the same sentence the reader answers.
        let (status, again) = emulator
            .call_in(
                region,
                "DeleteNotebookInstanceLifecycleConfig",
                &json!({ "NotebookInstanceLifecycleConfigName": name }),
            )
            .await;
        assert_eq!(
            (status, error_code(&again).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a second delete to be refused, got {status} with {again}"
        );
        assert_eq!(
            message(&again),
            message(&gone),
            "case {case:?}: expected the delete and the read to word the absence the same \
             way, got {:?} and {:?}",
            message(&again),
            message(&gone)
        );

        // ...and the configuration the delete did not name still resolves, with the hooks
        // it was created with.
        let (status, kept) = emulator
            .call_in(
                region,
                "DescribeNotebookInstanceLifecycleConfig",
                &json!({ "NotebookInstanceLifecycleConfigName": bystander }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the other configuration to survive, got {status} with \
             {kept}"
        );
        assert_eq!(
            str_field(&kept, "NotebookInstanceLifecycleConfigArn"),
            lifecycle_config_arn(region, &bystander),
            "case {case:?}: expected the surviving configuration's own ARN, got {kept:?}"
        );
        for (member, value) in hooks.as_object().expect("a JSON object of hooks") {
            assert_eq!(
                &kept[member], value,
                "case {case:?}: expected the surviving configuration to keep its {member} \
                 hook, got {kept:?}"
            );
        }
    }
}
