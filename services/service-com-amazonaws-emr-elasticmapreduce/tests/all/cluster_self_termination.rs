// SPDX-License-Identifier: CC0-1.0

//! A cluster that was not asked to stay up TERMINATES ITSELF, and one that was does not.
//!
//! THE RULE under test: a cluster launched with `KeepJobFlowAliveWhenNoSteps` false
//! ("specifies whether the cluster should remain available after completing all steps.
//! Defaults to false", `JobFlowInstancesConfig$KeepJobFlowAliveWhenNoSteps`, which is what
//! `Cluster$AutoTerminate` reports back) walks the modelled cluster-state machine
//! (`smithy_query(query="shape", shape="com.amazonaws.emr#ClusterState")`) from `STARTING`
//! through `BOOTSTRAPPING` and `RUNNING` and, once its step list has completed, through
//! `TERMINATING` into the terminal `TERMINATED` — carrying the modelled state-change reason
//! code `ALL_STEPS_COMPLETED`
//! (`smithy_query(query="shape", shape="com.amazonaws.emr#ClusterStateChangeReasonCode")`)
//! and a `Timeline.EndDateTime` at the instant it ended. A step list that is EMPTY has
//! completed, which is the whole of a transient cluster launched with no steps: it needs no
//! caller and no `TerminateJobFlows` to finish. A cluster launched with the flag TRUE rests
//! in `WAITING` instead, whatever its queue did.
//!
//! The rows span what the rule generalises over: whether the launch asked to stay up, how
//! many steps the launch submitted (none, one, two) and whether a step is added after the
//! cluster came up — the axis the defect this suite pins lived on, since a cluster with an
//! empty queue used to rest in `RUNNING` for ever.
//!
//! Every wait here waits for the CONDITION (`DescribeCluster` reporting a state) under a
//! deadline far above every span this service owes a cluster, so a slow host delays a row
//! rather than failing it.

use std::time::Duration;

use serde_json::{json, Value};

use crate::job_flow_lifecycle::{at, kept_alive_request, step_config, Emr, INSTANCE_PROFILE, SERVICE_ROLE};

/// How long a row waits for a state the clock owes before it gives up: far above the launch
/// spans and the step span a cluster of this service walks.
const DEADLINE: Duration = Duration::from_secs(90);

/// One poll of `DescribeCluster` inside [`DEADLINE`].
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The terminal states of the modelled machine a cluster of this suite can reach.
const TERMINATED: &str = "TERMINATED";

/// The state a cluster that was asked to stay up rests in.
const WAITING: &str = "WAITING";

/// One `StepConfig` naming the jar every step of this suite runs, with no arguments: what
/// the step DOES is another suite's concern (`cluster_step_execution.rs`), and what this one
/// asserts is that the cluster waits for it before shutting itself down.
fn step(name: &str) -> Value {
    step_config(name, "s3://emr/jars/noop.jar", Value::Null, Value::Null)
}

/// One cluster the rule is asserted over.
struct Row {
    case: &'static str,
    /// `KeepJobFlowAliveWhenNoSteps`: whether the launch asked the cluster to stay up.
    kept_alive: bool,
    /// How many steps the LAUNCH submits.
    launched_steps: usize,
    /// Whether a further step is submitted once the cluster has come up.
    step_added_later: bool,
    /// The state the cluster must come to rest in with nothing else called.
    resting_state: &'static str,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "transient, no steps at all",
            kept_alive: false,
            launched_steps: 0,
            step_added_later: false,
            resting_state: TERMINATED,
        },
        Row {
            case: "transient, one launched step",
            kept_alive: false,
            launched_steps: 1,
            step_added_later: false,
            resting_state: TERMINATED,
        },
        Row {
            case: "transient, two launched steps",
            kept_alive: false,
            launched_steps: 2,
            step_added_later: false,
            resting_state: TERMINATED,
        },
        Row {
            case: "transient, a step added after the launch",
            kept_alive: false,
            launched_steps: 0,
            step_added_later: true,
            resting_state: TERMINATED,
        },
        Row {
            case: "kept alive, no steps at all",
            kept_alive: true,
            launched_steps: 0,
            step_added_later: false,
            resting_state: WAITING,
        },
        Row {
            case: "kept alive, one launched step",
            kept_alive: true,
            launched_steps: 1,
            step_added_later: false,
            resting_state: WAITING,
        },
    ]
}

/// The `RunJobFlow` request one row launches: the transient form the rule is about, or the
/// kept-alive form, carrying `steps` steps.
fn launch(row: &Row) -> Value {
    let steps: Vec<Value> = (0..row.launched_steps)
        .map(|index| step(&format!("step-{index}")))
        .collect();
    let mut request = if row.kept_alive {
        kept_alive_request(row.case)
    } else {
        json!({
            "Name": row.case,
            "ReleaseLabel": "emr-6.15.0",
            "JobFlowRole": INSTANCE_PROFILE,
            "ServiceRole": SERVICE_ROLE,
            "Instances": {
                "InstanceCount": 1,
                "MasterInstanceType": "m5.xlarge",
                "SlaveInstanceType": "m5.xlarge",
                // The flag the rule is about, named explicitly on the transient rows.
                "KeepJobFlowAliveWhenNoSteps": false,
                "TerminationProtected": false,
            },
        })
    };
    if !steps.is_empty() {
        request["Steps"] = Value::Array(steps);
    }
    request
}

/// Poll `DescribeCluster` until the cluster reports `state`, and answer what it then
/// reported.
async fn await_state(emr: &Emr, case: &str, cluster_id: &str, state: &str) -> Value {
    let deadline = std::time::Instant::now() + DEADLINE;
    let mut last = emr.describe_cluster(cluster_id).await;
    loop {
        if at(&last, &["Status", "State"]).as_str() == Some(state) {
            return last;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the cluster never reached {state}, last {last}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
        last = emr.describe_cluster(cluster_id).await;
    }
}

/// Every state `DescribeCluster` reported for this cluster until it rested, in the order
/// they were observed.
async fn walk_until_resting(emr: &Emr, case: &str, cluster_id: &str, resting: &str) -> Vec<String> {
    let deadline = std::time::Instant::now() + DEADLINE;
    let mut observed: Vec<String> = Vec::new();
    loop {
        let cluster = emr.describe_cluster(cluster_id).await;
        let state = at(&cluster, &["Status", "State"])
            .as_str()
            .unwrap_or_default()
            .to_string();
        if observed.last() != Some(&state) {
            observed.push(state.clone());
        }
        if state == resting {
            return observed;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the cluster never reached {resting}; observed {observed:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[tokio::test]
async fn a_cluster_that_was_not_kept_alive_terminates_itself_once_its_queue_has_run() {
    for row in rows() {
        let case = row.case;
        let emr = Emr::new();
        let cluster_id = emr.run_job_flow(launch(&row)).await;

        // The launch answers while the cluster is still coming up, so the transient state
        // is observable to a caller acting straight after it.
        let starting = emr.describe_cluster(&cluster_id).await;
        assert_eq!(
            at(&starting, &["Status", "State"]).as_str(),
            Some("STARTING"),
            "case {case:?}: a new cluster starts in STARTING, got {starting}"
        );

        if row.step_added_later {
            // A step submitted to a transient cluster holds its shutdown off until that
            // step has run, which is what makes the shutdown a fact of the QUEUE rather
            // than of the launch.
            let added = emr
                .ok(
                    "AddJobFlowSteps",
                    json!({
                        "JobFlowId": &cluster_id,
                        "Steps": [step("added")],
                    }),
                )
                .await;
            assert_eq!(
                added["StepIds"].as_array().map(Vec::len),
                Some(1),
                "case {case:?}: the added step must be accepted, got {added}"
            );
        }

        // Nothing but the clock is called from here on: no TerminateJobFlows, no
        // SetKeepJobFlowAliveWhenNoSteps.
        let states = walk_until_resting(&emr, case, &cluster_id, row.resting_state).await;
        assert!(
            states.first().map(String::as_str) == Some("STARTING"),
            "case {case:?}: the walk must begin at STARTING, observed {states:?}"
        );
        assert!(
            states.contains(&row.resting_state.to_string()),
            "case {case:?}: expected the cluster to reach {}, observed {states:?}",
            row.resting_state
        );
        // Every state observed is one of the modelled machine's, and the walk never goes
        // back on itself: each state is seen once, in order.
        let mut once = states.clone();
        once.dedup();
        assert_eq!(
            once, states,
            "case {case:?}: a cluster never re-enters a state it left, observed {states:?}"
        );

        let rested = await_state(&emr, case, &cluster_id, row.resting_state).await;
        if row.resting_state == TERMINATED {
            // The shutdown names WHY it happened and WHEN it ended.
            assert_eq!(
                at(&rested, &["Status", "StateChangeReason", "Code"]).as_str(),
                Some("ALL_STEPS_COMPLETED"),
                "case {case:?}: the self-shutdown names the modelled reason, got {rested}"
            );
            let ready = at(&rested, &["Status", "Timeline", "ReadyDateTime"])
                .as_f64()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a cluster that ran its queue was ready, got {rested}")
                });
            let ended = at(&rested, &["Status", "Timeline", "EndDateTime"])
                .as_f64()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a terminated cluster records EndDateTime, got {rested}")
                });
            assert!(
                ended >= ready,
                "case {case:?}: EndDateTime {ended} must follow ReadyDateTime {ready}"
            );
            // Every step of the queue ended, and none was left behind by the shutdown.
            let steps = emr
                .list_steps(json!({ "ClusterId": &cluster_id }))
                .await;
            let submitted = row.launched_steps + usize::from(row.step_added_later);
            assert_eq!(
                steps["Steps"].as_array().map(Vec::len),
                Some(submitted),
                "case {case:?}: the cluster keeps every step submitted to it, got {steps}"
            );
            for step in steps["Steps"].as_array().cloned().unwrap_or_default() {
                let state = at(&step, &["Status", "State"])
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                assert!(
                    ["COMPLETED", "FAILED", "CANCELLED", "INTERRUPTED"].contains(&state.as_str()),
                    "case {case:?}: every step of a terminated cluster has ended, got {step}"
                );
            }
        } else {
            // A cluster asked to stay up reports no shutdown at all, however its queue
            // ended: nothing moves it out of WAITING but a caller.
            assert!(
                at(&rested, &["Status", "Timeline", "EndDateTime"]).is_null(),
                "case {case:?}: a cluster kept alive has not ended, got {rested}"
            );
        }

        // The state the cluster rests in is final: reading it again answers the same.
        let again = emr.describe_cluster(&cluster_id).await;
        assert_eq!(
            at(&again, &["Status", "State"]).as_str(),
            Some(row.resting_state),
            "case {case:?}: the resting state must hold across reads, got {again}"
        );
    }
}
