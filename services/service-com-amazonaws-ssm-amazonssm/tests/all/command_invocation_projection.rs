// SPDX-License-Identifier: CC0-1.0

//! What a READ of one node's command invocation reports of the COMMAND it belongs to, driven
//! end to end with Systems Manager, Amazon EC2 and IAM on one engine.
//!
//! RULE under test: RUN COMMAND REPORTS A COMMAND'S WHOLE CONFIGURATION ON EVERY READ OF IT —
//! so the members an invocation carries of its command are reported whether or not the send
//! configured them: an unset scalar as the empty string and an unset configuration structure as
//! the object with its own members at their defaults, rather than omitted. That is what lets a
//! client read the echo to confirm what the command was accepted with instead of having to tell
//! "the service did not report it" from "the service dropped it", and it is the same projection
//! the command reads already answer with (`tests/all/run_command.rs`).
//!
//! Every case goes through the front door: `AmazonSSM.SendCommand`,
//! `AmazonSSM.GetCommandInvocation` and `AmazonSSM.ListCommandInvocations` over `awsJson1_1`,
//! with the instance launched over EC2's `ec2Query` wire and its role and instance profile
//! created over IAM's `awsQuery` wire.
//!
//! The rows span what the rule generalises over: WHICH of those members the send configured
//! (none of them, the comment alone, the CloudWatch output alone, both), and the two readers
//! that project them.

use serde_json::{json, Value};

use super::ec2_instance_managed_nodes::{
    launched_long_enough_ago, Emulator, Grant, AL2023_NAME_PATTERN, REGION, SSM_CORE_POLICY_ARN,
};
use super::run_command_in_the_guest::{finish_due, finished};

/// The published command document every case runs.
const DOCUMENT: &str = "AWS-RunShellScript";

/// The script it runs, which is beside the point of this suite: one line the guest's shell
/// performs.
const SCRIPT: &str = "echo projected";

/// The plugin the document invokes, which is the step name a read reports and the one
/// `GetCommandInvocation` accepts back on input (`GetCommandInvocationRequest$PluginName`).
const PLUGIN: &str = "aws:runShellScript";

/// Every member of `com.amazonaws.ssm#GetCommandInvocationResult`, in the model's member order
/// (`smithy_query(query="operation",operation="GetCommandInvocation")`). A finished invocation
/// reports all of them.
const RESULT_MEMBERS: &[&str] = &[
    "CommandId",
    "InstanceId",
    "Comment",
    "DocumentName",
    "DocumentVersion",
    "PluginName",
    "ResponseCode",
    "ExecutionStartDateTime",
    "ExecutionElapsedTime",
    "ExecutionEndDateTime",
    "Status",
    "StatusDetails",
    "StandardOutputContent",
    "StandardOutputUrl",
    "StandardErrorContent",
    "StandardErrorUrl",
    "CloudWatchOutputConfig",
];

/// The members `com.amazonaws.ssm#CommandInvocation` carries OF ITS COMMAND
/// (`smithy_query(query="shape",shape="CommandInvocation")`), each of which the listing
/// reports.
const INVOCATION_COMMAND_MEMBERS: &[&str] = &[
    "Comment",
    "DocumentName",
    "DocumentVersion",
    "ServiceRole",
    "NotificationConfig",
    "CloudWatchOutputConfig",
];

/// One row: what the send configured, and what the two readers must report for it.
struct ProjectionCase {
    case: &'static str,
    /// The members the send carries beyond the document, the node and the script.
    configured: Value,
    /// The `Comment` both readers report.
    comment: &'static str,
    /// The `CloudWatchOutputConfig` both readers report.
    cloud_watch: Value,
}

/// Launch an instance that becomes a managed node and answer its id.
async fn node(emulator: &Emulator, case: &str) -> String {
    let image = emulator.image(case, AL2023_NAME_PATTERN).await;
    let profile = emulator
        .profile(
            case,
            "invocation-projection-role",
            &Grant::Attached(SSM_CORE_POLICY_ARN),
        )
        .await;
    let instance = emulator.launch(case, &image, Some(&profile)).await;
    emulator.launched_ago(&instance, launched_long_enough_ago());
    let reported = emulator.nodes(case, REGION).await;
    assert!(
        reported.contains_key(&instance),
        "case {case:?}: the launched instance must become a managed node first, got {:?}",
        reported.keys().collect::<Vec<_>>()
    );
    instance
}

/// Send one command carrying `configured` beside the document, the node and the script, and
/// answer the command id.
async fn send(emulator: &Emulator, case: &str, instance: &str, configured: &Value) -> String {
    let mut body = json!({
        "DocumentName": DOCUMENT,
        "InstanceIds": [instance],
        "Parameters": {"commands": [SCRIPT]},
    });
    for (member, value) in configured.as_object().into_iter().flatten() {
        body[member] = value.clone();
    }
    let (status, answered) = emulator.ssm(REGION, "SendCommand", &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the send must be served, got {status}: {answered}"
    );
    answered["Command"]["CommandId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the send must mint a command id: {answered}"))
        .to_string()
}

/// The one entry `ListCommandInvocations` reports for `command_id`.
async fn listed(emulator: &Emulator, case: &str, command_id: &str) -> Value {
    let (status, answered) = emulator
        .ssm(
            REGION,
            "ListCommandInvocations",
            &json!({"CommandId": command_id}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the listing must be served, got {status}: {answered}"
    );
    let entries = answered["CommandInvocations"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        entries.len(),
        1,
        "case {case:?}: the command was sent to one node, got {answered}"
    );
    entries.into_iter().next().unwrap_or(Value::Null)
}

/// RULE: both readers of an invocation report the members it carries of its command, each as
/// what the send configured or as that member's own default — never omitted.
#[tokio::test]
async fn an_invocation_reports_its_commands_whole_configuration() {
    let case = "the projection table";
    let emulator = Emulator::new();
    let instance = node(&emulator, case).await;
    let rows: Vec<ProjectionCase> = vec![
        ProjectionCase {
            case: "a send that configured none of them",
            configured: json!({}),
            comment: "",
            cloud_watch: json!({"CloudWatchLogGroupName": "", "CloudWatchOutputEnabled": false}),
        },
        ProjectionCase {
            case: "a send that carried a comment alone",
            configured: json!({"Comment": "patch window"}),
            comment: "patch window",
            cloud_watch: json!({"CloudWatchLogGroupName": "", "CloudWatchOutputEnabled": false}),
        },
        ProjectionCase {
            case: "a send that asked for CloudWatch output alone",
            configured: json!({
                "CloudWatchOutputConfig": {
                    "CloudWatchLogGroupName": "/aws/ssm/projected",
                    "CloudWatchOutputEnabled": true,
                },
            }),
            comment: "",
            cloud_watch: json!({
                "CloudWatchLogGroupName": "/aws/ssm/projected",
                "CloudWatchOutputEnabled": true,
            }),
        },
        ProjectionCase {
            case: "a send that enabled CloudWatch output without naming a log group",
            configured: json!({
                "Comment": "both of them",
                "CloudWatchOutputConfig": {"CloudWatchOutputEnabled": true},
            }),
            comment: "both of them",
            cloud_watch: json!({"CloudWatchLogGroupName": "", "CloudWatchOutputEnabled": true}),
        },
    ];
    for row in &rows {
        let command_id = send(&emulator, row.case, &instance, &row.configured).await;
        finish_due(&emulator, &command_id);
        let read = finished(&emulator, row.case, &command_id, &instance).await;
        for member in RESULT_MEMBERS {
            assert!(
                !read[*member].is_null(),
                "case {:?}: the read reports {member}, got {read}",
                row.case
            );
        }
        assert_eq!(
            read["Comment"].as_str(),
            Some(row.comment),
            "case {:?}: expected the comment the send carried, got {read}",
            row.case
        );
        assert_eq!(
            read["CloudWatchOutputConfig"], row.cloud_watch,
            "case {:?}: expected {}, got {read}",
            row.case, row.cloud_watch
        );
        assert_eq!(
            read["PluginName"].as_str(),
            Some(PLUGIN),
            "case {:?}: expected the plugin the document invokes, got {read}",
            row.case
        );

        let entry = listed(&emulator, row.case, &command_id).await;
        for member in INVOCATION_COMMAND_MEMBERS {
            assert!(
                !entry[*member].is_null(),
                "case {:?}: the listing reports {member}, got {entry}",
                row.case
            );
        }
        assert_eq!(
            entry["Comment"].as_str(),
            Some(row.comment),
            "case {:?}: expected the listing to report the same comment, got {entry}",
            row.case
        );
        assert_eq!(
            entry["CloudWatchOutputConfig"], row.cloud_watch,
            "case {:?}: expected the listing to report the same output configuration, got {entry}",
            row.case
        );
        assert_eq!(
            entry["ServiceRole"].as_str(),
            Some(""),
            "case {:?}: a send that named no service role reports none by name, got {entry}",
            row.case
        );
        assert_eq!(
            entry["NotificationConfig"],
            json!({"NotificationArn": "", "NotificationEvents": [], "NotificationType": ""}),
            "case {:?}: a send that asked for no notification reports that structure at its \
             defaults, got {entry}",
            row.case
        );
    }
}

/// RULE: the plugin name a read reports is the step the document invokes, which is the name the
/// same read accepts back on input (`GetCommandInvocationRequest$PluginName`), so a client can
/// feed what it read back and be answered the same invocation.
#[tokio::test]
async fn the_plugin_name_a_read_reports_is_one_it_accepts_back() {
    let case = "the plugin name round trip";
    let emulator = Emulator::new();
    let instance = node(&emulator, case).await;
    let command_id = send(&emulator, case, &instance, &json!({})).await;
    finish_due(&emulator, &command_id);
    let read = finished(&emulator, case, &command_id, &instance).await;
    let reported = read["PluginName"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the read must report the plugin: {read}"))
        .to_string();
    assert_eq!(
        reported, PLUGIN,
        "case {case:?}: expected the step the document invokes, got {reported:?}"
    );
    let (status, answered) = emulator
        .ssm(
            REGION,
            "GetCommandInvocation",
            &json!({
                "CommandId": command_id,
                "InstanceId": instance,
                "PluginName": reported,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the name the read reported must be accepted back, got {status}: \
         {answered}"
    );
    assert_eq!(
        answered["PluginName"].as_str(),
        Some(PLUGIN),
        "case {case:?}: expected the same step reported for the name it was asked about, got \
         {answered}"
    );
    assert_eq!(
        answered["StandardOutputContent"].as_str(),
        read["StandardOutputContent"].as_str(),
        "case {case:?}: expected the same invocation, got {answered}"
    );
}
