// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetExecutionFlowSnapshot`
//! (`GET /flows/{flowIdentifier}/aliases/{flowAliasIdentifier}/executions/{executionIdentifier}/flowsnapshot`),
//! the flow definition snapshot one flow execution was started against.
//!
//! THE RULES UNDER TEST:
//!
//! * the snapshot reports the flow metadata and definition AS THEY WERE when the execution
//!   started — the definition the version its alias routed to packaged, that thing's own
//!   execution role and customer managed key, and the version itself — so a flow edited,
//!   re-roled or re-prepared afterwards leaves the snapshot of an execution already started
//!   exactly where it was;
//! * `definition` carries that definition as text following the structure of a
//!   `FlowDefinition` object, so reading it back as JSON yields the definition the flow was
//!   created with, node for node and connection for connection;
//! * the ids it reports are the ones the execution resolved to, whichever form the request's
//!   three identifiers took, and reading the snapshot changes nothing a later caller sees;
//! * an `executionIdentifier` naming no execution of the addressed alias is the modelled
//!   `ResourceNotFoundException` (404) with the fixed sentence this plane states for an
//!   unresolved flow resource.
//!
//! The tables span what those rules generalise over: the shape of the definition the
//! execution ran (a single hop, a fan-out, a path into a node kind nothing here runs, with
//! its nested unions), whether the flow names a customer managed key, what the alias routes
//! to (the draft, a published numbered version), the form each identifier takes, whether the
//! run has finished, and why nothing resolves.

use serde_json::{json, Value};

use crate::agent_and_flow_invocations::{
    execution_arn, execution_not_found, fan_out_definition, label, passthrough_definition,
    prompt_node_definition,
};
use crate::agent_memory::{Emulator, Reply, ACCOUNT_KEY, NOT_FOUND};

/// The `Version` an alias routes to when it routes to the flow's working draft, as the
/// build-time plane spells it.
const DRAFT_VERSION: &str = "DRAFT";

/// One IAM service role a flow here runs with, in the form the modelled
/// `FlowExecutionRoleArn` spells one (pattern
/// `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/(service-role/)?.+$`).
const FIRST_ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForFlows_first";

/// A SECOND such role, so a flow re-roled after an execution started differs from the one
/// that execution runs with.
const SECOND_ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForFlows_second";

/// A customer managed KMS key ARN, in the form the modelled `KmsKeyArn` spells one (pattern
/// `^arn:aws(|-cn|-us-gov):kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$`).
const KEY_ARN: &str = "arn:aws:kms:us-west-2:000000000000:key/11111111-2222-3333-4444-555555555555";

impl Emulator {
    /// One `GetExecutionFlowSnapshot` call over the modelled URI.
    async fn get_execution_flow_snapshot(
        &self,
        flow: &str,
        alias: &str,
        execution: &str,
    ) -> Reply {
        self.bedrock(
            ACCOUNT_KEY,
            "GET",
            &format!(
                "/flows/{}/aliases/{}/executions/{}/flowsnapshot",
                label(flow),
                label(alias),
                label(execution)
            ),
            None,
        )
        .await
    }

    /// A flow created through the build-time plane's own `CreateFlow` with the role and the
    /// customer managed key this suite wants on it: its `(flowId, flowArn)`.
    async fn create_roled_flow(
        &self,
        name: &str,
        definition: Value,
        role: &str,
        key: Option<&str>,
    ) -> (String, String) {
        let mut body = json!({
            "name": name,
            "executionRoleArn": role,
            "definition": definition,
        });
        if let Some(key) = key {
            body["customerEncryptionKeyArn"] = json!(key);
        }
        let reply = self.bedrock(ACCOUNT_KEY, "POST", "/flows/", Some(body)).await;
        assert_eq!(
            reply.status,
            201,
            "setup {name:?}: CreateFlow must answer 201, got {} {:?}",
            reply.status,
            reply.json(name)
        );
        let created = reply.json(name);
        (
            created["id"].as_str().expect("a minted flowId").to_string(),
            created["arn"]
                .as_str()
                .expect("a minted flow ARN")
                .to_string(),
        )
    }

    /// Cut a numbered version of the prepared draft of `flow` through the build-time plane's
    /// own `CreateFlowVersion`, and answer the `NumericalVersion` it cut.
    async fn publish_flow_version(&self, case: &str, flow: &str) -> String {
        let reply = self
            .bedrock(
                ACCOUNT_KEY,
                "POST",
                &format!("/flows/{flow}/versions"),
                Some(json!({})),
            )
            .await;
        assert_eq!(
            reply.status,
            201,
            "case {case:?}: CreateFlowVersion must answer 201, got {} {:?}",
            reply.status,
            reply.json(case)
        );
        reply.json(case)["version"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateFlowVersion answered no version"))
            .to_string()
    }

    /// Move the DRAFT of `flow` to another definition AND another role through the
    /// build-time plane's own `UpdateFlow`, then prepare it again: this is the flow being
    /// edited after an execution has started.
    async fn reroled_flow(&self, case: &str, flow: &str, name: &str, definition: Value) {
        let reply = self
            .bedrock(
                ACCOUNT_KEY,
                "PUT",
                &format!("/flows/{flow}/"),
                Some(json!({
                    "name": name,
                    "executionRoleArn": SECOND_ROLE,
                    "definition": definition,
                })),
            )
            .await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: UpdateFlow must answer 200, got {} {:?}",
            reply.status,
            reply.json(case)
        );
        self.prepare_flow(case, flow).await;
    }
}

/// The `definition` a snapshot reports, read back as the JSON object its text follows the
/// structure of.
fn reported_definition(case: &str, body: &Value) -> Value {
    let text = body["definition"].as_str().unwrap_or_else(|| {
        panic!("case {case:?}: definition must be a string, got {body:?}")
    });
    serde_json::from_str(text)
        .unwrap_or_else(|error| panic!("case {case:?}: definition must be JSON, got {text:?} ({error})"))
}

/// RULE: the snapshot of an execution reports the definition that execution ran, as text
/// following the structure of a `FlowDefinition` object, beside the flow, alias and version
/// it resolved to and the role and customer managed key the thing packaging that definition
/// carries.
///
/// The rows differ along what the rule generalises over: the shape of the definition (one
/// hop, a fan-out to two output nodes, a path into a node kind nothing here runs, whose
/// configuration nests two more unions), whether the run of it delivered anything, and
/// whether the flow names a customer managed key.
#[tokio::test]
async fn the_snapshot_reports_the_definition_the_execution_ran() {
    /// One flow, as it was created, and the execution started on it.
    struct Row {
        case: &'static str,
        definition: Value,
        /// The customer managed key the flow names, if any.
        key: Option<&'static str>,
    }

    let rows = [
        Row {
            case: "a single hop, under a customer managed key",
            definition: passthrough_definition(),
            key: Some(KEY_ARN),
        },
        Row {
            case: "a fan-out to two output nodes, under no key of the caller's",
            definition: fan_out_definition(),
            key: None,
        },
        Row {
            case: "a path into a node kind nothing here runs",
            definition: prompt_node_definition(),
            key: Some(KEY_ARN),
        },
    ];

    let emulator = Emulator::new();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let name = format!("snapshot-flow-{index}");
        let (flow, _) = emulator
            .create_roled_flow(&name, row.definition.clone(), FIRST_ROLE, row.key)
            .await;
        emulator.prepare_flow(case, &flow).await;
        let (alias, _) = emulator.create_flow_alias(case, &flow, DRAFT_VERSION).await;
        let started = emulator
            .start_flow_execution(
                &flow,
                &alias,
                "FlowInput",
                json!({ "data": "the snapshot is the flow as it was" }),
                Some("snapshot-run"),
            )
            .await;
        assert_eq!(
            started.status,
            200,
            "case {case:?}: StartFlowExecution must answer 200, got {} {:?}",
            started.status,
            started.json(case)
        );

        let reply = emulator
            .get_execution_flow_snapshot(&flow, &alias, "snapshot-run")
            .await;
        let body = reply.json(case);
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected the snapshot to be served, got {} {body:?}",
            reply.status
        );
        assert_eq!(
            (
                body["flowIdentifier"].as_str(),
                body["flowAliasIdentifier"].as_str(),
                body["flowVersion"].as_str(),
                body["executionRoleArn"].as_str(),
            ),
            (
                Some(flow.as_str()),
                Some(alias.as_str()),
                Some(DRAFT_VERSION),
                Some(FIRST_ROLE),
            ),
            "case {case:?}: expected the ids and the role the execution resolved to, got \
             {body:?}"
        );
        assert_eq!(
            body.get("customerEncryptionKeyArn").and_then(Value::as_str),
            row.key,
            "case {case:?}: expected the key the flow names ({:?}), got {body:?}",
            row.key
        );
        assert_eq!(
            reported_definition(case, &body),
            row.definition,
            "case {case:?}: expected the definition the flow was created with, got {body:?}"
        );
    }
}

/// RULE: the snapshot is the flow as it stood when the execution STARTED — "even if the flow
/// is edited after an execution starts, the snapshot connected to the execution remains
/// unchanged" — and what it is a snapshot OF is the thing the alias routed to: the flow's own
/// draft, or the numbered version that packaged a definition, a role and a key of its own.
///
/// The rows differ along what the alias routes to; in both the flow is given a new
/// definition and a new role after the execution has started.
#[tokio::test]
async fn an_edit_of_the_flow_leaves_an_already_started_snapshot_where_it_was() {
    /// One alias, and what the execution started through it is a snapshot of.
    struct Row {
        case: &'static str,
        /// Whether the alias routes to a numbered version cut from the prepared draft,
        /// rather than to the draft itself.
        published: bool,
    }

    let rows = [
        Row {
            case: "an alias routing to the draft",
            published: false,
        },
        Row {
            case: "an alias routing to a published numbered version",
            published: true,
        },
    ];

    let emulator = Emulator::new();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let name = format!("snapshot-edited-flow-{index}");
        let (flow, _) = emulator
            .create_roled_flow(&name, passthrough_definition(), FIRST_ROLE, Some(KEY_ARN))
            .await;
        emulator.prepare_flow(case, &flow).await;
        let routed = if row.published {
            emulator.publish_flow_version(case, &flow).await
        } else {
            DRAFT_VERSION.to_string()
        };
        let (alias, _) = emulator.create_flow_alias(case, &flow, &routed).await;
        let started = emulator
            .start_flow_execution(
                &flow,
                &alias,
                "FlowInput",
                json!({ "data": "started before the edit" }),
                Some("before-the-edit"),
            )
            .await;
        assert_eq!(
            started.status,
            200,
            "case {case:?}: StartFlowExecution must answer 200, got {} {:?}",
            started.status,
            started.json(case)
        );
        let before = emulator
            .get_execution_flow_snapshot(&flow, &alias, "before-the-edit")
            .await
            .json(case);

        // The flow MOVES: another definition, another role, prepared again.
        emulator
            .reroled_flow(case, &flow, &name, fan_out_definition())
            .await;

        let reply = emulator
            .get_execution_flow_snapshot(&flow, &alias, "before-the-edit")
            .await;
        let after = reply.json(case);
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected the snapshot to be served after the edit, got {} \
             {after:?}",
            reply.status
        );
        assert_eq!(
            after, before,
            "case {case:?}: expected the edit to leave the snapshot where it was, got \
             {after:?} after {before:?}"
        );
        assert_eq!(
            (
                after["flowVersion"].as_str(),
                after["executionRoleArn"].as_str(),
                after
                    .get("customerEncryptionKeyArn")
                    .and_then(Value::as_str),
            ),
            (Some(routed.as_str()), Some(FIRST_ROLE), Some(KEY_ARN)),
            "case {case:?}: expected the version, role and key of the thing the alias routed \
             to at the start, got {after:?}"
        );
        assert_eq!(
            reported_definition(case, &after),
            passthrough_definition(),
            "case {case:?}: expected the definition the execution started against, got \
             {after:?}"
        );
    }
}

/// RULE: the three identifiers of the request each admit more than one form, and every form
/// naming the same execution reads the same snapshot; the ids the answer reports are the ones
/// the execution resolved to rather than the forms the request carried.
#[tokio::test]
async fn every_identifier_form_reads_the_same_snapshot() {
    let emulator = Emulator::new();
    let case = "the forms of one execution's three identifiers";
    let (flow, flow_arn, alias, alias_arn) = emulator
        .executable_flow(case, "snapshot-identifier-forms", passthrough_definition())
        .await;
    let started = emulator
        .start_flow_execution(
            &flow,
            &alias,
            "FlowInput",
            json!({ "data": "read under every form" }),
            Some("forms"),
        )
        .await;
    assert_eq!(
        started.status,
        200,
        "setup: StartFlowExecution must answer 200, got {} {:?}",
        started.status,
        started.json(case)
    );
    let arn = execution_arn(&flow, &alias, "forms");

    let expected = emulator
        .get_execution_flow_snapshot(&flow, &alias, "forms")
        .await
        .json(case);
    assert_eq!(
        expected["flowIdentifier"].as_str(),
        Some(flow.as_str()),
        "case {case:?}: expected the flow id the execution resolved to, got {expected:?}"
    );

    for (flow_form, flow_identifier) in [("the id", flow.as_str()), ("the ARN", flow_arn.as_str())]
    {
        for (alias_form, alias_identifier) in
            [("the id", alias.as_str()), ("the ARN", alias_arn.as_str())]
        {
            for (execution_form, execution_identifier) in
                [("the name", "forms"), ("the ARN", arn.as_str())]
            {
                let case = format!(
                    "flow by {flow_form}, alias by {alias_form}, execution by {execution_form}"
                );
                let case = case.as_str();
                let reply = emulator
                    .get_execution_flow_snapshot(
                        flow_identifier,
                        alias_identifier,
                        execution_identifier,
                    )
                    .await;
                let body = reply.json(case);
                assert_eq!(
                    (reply.status, &body),
                    (200, &expected),
                    "case {case:?}: expected the same snapshot under every identifier form"
                );
            }
        }
    }
}

/// RULE: the snapshot is a REPORT — it is there for as long as the execution is, before and
/// after the run has finished, and reading it moves nothing a later caller sees, neither the
/// snapshot itself nor the execution's own status.
#[tokio::test]
async fn reading_the_snapshot_moves_nothing_and_outlives_the_run() {
    let emulator = Emulator::new();
    let case = "a snapshot read while the run is in flight and after it has finished";
    let (flow, _, alias, _) = emulator
        .executable_flow(case, "snapshot-outlives-run", passthrough_definition())
        .await;
    let started = emulator
        .start_flow_execution(
            &flow,
            &alias,
            "FlowInput",
            json!({ "data": "read twice" }),
            Some("outlives"),
        )
        .await;
    assert_eq!(
        started.status,
        200,
        "setup: StartFlowExecution must answer 200, got {} {:?}",
        started.status,
        started.json(case)
    );

    let first = emulator
        .get_execution_flow_snapshot(&flow, &alias, "outlives")
        .await
        .json(case);
    let second = emulator
        .get_execution_flow_snapshot(&flow, &alias, "outlives")
        .await
        .json(case);
    assert_eq!(
        second, first,
        "case {case:?}: expected a second read to report the same snapshot, got {second:?} \
         after {first:?}"
    );
    // Reading the snapshot is not watching the run: the execution is still inside its span.
    let running = emulator.get_flow_execution(&flow, &alias, "outlives").await;
    assert_eq!(
        (running.status, running.json(case)["status"].as_str()),
        (200, Some("Running")),
        "case {case:?}: expected the read of the snapshot to leave the execution Running, got \
         {:?}",
        running.json(case)
    );

    // The clock brings the run to its end; the snapshot of what it ran stays.
    emulator.finish_running_executions();
    let finished = emulator.get_flow_execution(&flow, &alias, "outlives").await;
    assert_eq!(
        (finished.status, finished.json(case)["status"].as_str()),
        (200, Some("Succeeded")),
        "case {case:?}: expected the finished run to have succeeded, got {:?}",
        finished.json(case)
    );
    let reply = emulator
        .get_execution_flow_snapshot(&flow, &alias, "outlives")
        .await;
    let after = reply.json(case);
    assert_eq!(
        (reply.status, &after),
        (200, &first),
        "case {case:?}: expected the snapshot of a finished execution to be the same one"
    );
}

/// RULE: an `executionIdentifier` naming no execution of the addressed alias has no snapshot
/// to report, whatever makes it name none, and is refused with the modelled
/// `ResourceNotFoundException` (404) carrying this plane's fixed sentence for an unresolved
/// flow resource.
///
/// The rows differ along WHY nothing resolves: a name nothing started, a flow or an alias the
/// execution does not belong to, an execution ARN of another alias, and one of another
/// account.
#[tokio::test]
async fn an_execution_nothing_started_has_no_snapshot() {
    let emulator = Emulator::new();
    let case = "the setup the refusals below are addressed through";
    let (flow, _, alias, _) = emulator
        .executable_flow(case, "snapshot-not-found", passthrough_definition())
        .await;
    let (other_flow, _, other_alias, _) = emulator
        .executable_flow(case, "snapshot-not-found-other", passthrough_definition())
        .await;
    let started = emulator
        .start_flow_execution(
            &flow,
            &alias,
            "FlowInput",
            json!({ "data": "the only execution here" }),
            Some("held"),
        )
        .await;
    assert_eq!(
        started.status,
        200,
        "setup: StartFlowExecution must answer 200, got {} {:?}",
        started.status,
        started.json(case)
    );

    // (why nothing resolves, the flow, the alias, the execution the request addresses)
    let rows: Vec<(&str, &str, &str, String)> = vec![
        (
            "a name nothing started on this alias",
            flow.as_str(),
            alias.as_str(),
            "never-started".to_string(),
        ),
        (
            "an execution of another flow's alias",
            other_flow.as_str(),
            other_alias.as_str(),
            "held".to_string(),
        ),
        (
            "an execution addressed through an alias it does not belong to",
            flow.as_str(),
            other_alias.as_str(),
            "held".to_string(),
        ),
        (
            "an execution ARN naming another alias",
            flow.as_str(),
            alias.as_str(),
            execution_arn(&flow, &other_alias, "held"),
        ),
        (
            "an execution ARN naming another account",
            flow.as_str(),
            alias.as_str(),
            format!(
                "arn:aws:bedrock:us-west-2:111122223333:flow/{flow}/alias/{alias}/execution/held"
            ),
        ),
    ];

    for (case, flow_identifier, alias_identifier, execution_identifier) in rows {
        let reply = emulator
            .get_execution_flow_snapshot(flow_identifier, alias_identifier, &execution_identifier)
            .await;
        assert_eq!(
            (reply.status, reply.error_code(case).as_str()),
            (NOT_FOUND.1, NOT_FOUND.0),
            "case {case:?}: expected a 404 {}, got {} {:?}",
            NOT_FOUND.0,
            reply.status,
            reply.json(case)
        );
        assert_eq!(
            reply.message(case),
            execution_not_found(),
            "case {case:?}: expected this plane's sentence for an unresolved flow resource, \
             got {:?}",
            reply.json(case)
        );
    }
}
