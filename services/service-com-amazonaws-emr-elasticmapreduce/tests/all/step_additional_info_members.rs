// SPDX-License-Identifier: CC0-1.0

//! The MEMBER SET a step's `AdditionalInfo` is served as.
//!
//! THE RULE: a step read serves `AdditionalInfo` as the structure the service models for it —
//! exactly the members that shape declares, each at its modelled type — for every step whose
//! execution reached the cluster's resource manager, however the step was submitted and
//! whatever its command runs, and serves no `AdditionalInfo` at all for a step whose execution
//! has not started.
//!
//! The rows differ along what the rule generalises over: how the step entered the queue (with
//! the launch, or added to a running cluster afterwards) and what its command is (a jar with
//! arguments, a jar with a main class, and a `spark-submit` command line) — the three classes
//! this service reads a step's work off differently.
//!
//! Every case drives the real HTTP path — `POST /` with `X-Amz-Target: ElasticMapReduce.<Op>`
//! and an `awsJson1_1` body — and reads each outcome back through `DescribeStep` and
//! `ListSteps`, so nothing here is evidence of anything but the persisted state.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::job_flow_lifecycle::{at, await_settled_step, kept_alive_request, step_config, Emr};

/// The members `StepAdditionalInfo` declares, from
/// `smithy_query(query="shape", shape="StepAdditionalInfo")`: the YARN application ids
/// associated with the step, and the address of the resource manager that processed it.
const MODELLED_MEMBERS: [&str; 2] = ["ResourceManagerApplicationIds", "ResourceManager"];

/// The resource manager an Amazon EMR cluster schedules every step's execution through.
const RESOURCE_MANAGER: &str = "YARN";

/// The jar every step below submits. No bytes of it exist here, so each step ends in the
/// modelled failure state — which is beside the point: what is under test is the SHAPE of the
/// metadata a step whose execution ran reports.
const STEP_JAR: &str = "s3://jars/additional-info.jar";

/// One submitted step: how it enters the queue and what its command is.
struct Case {
    label: &'static str,
    /// Added through `AddJobFlowSteps` after the launch rather than named by it.
    added_later: bool,
    config: fn(&str) -> Value,
}

fn jar_with_args(name: &str) -> Value {
    step_config(name, STEP_JAR, json!(["--input", "s3://in/data"]), Value::Null)
}

fn jar_with_main_class(name: &str) -> Value {
    step_config(
        name,
        STEP_JAR,
        json!(["10"]),
        json!({ "MainClass": "org.example.Count" }),
    )
}

fn spark_submit(name: &str) -> Value {
    step_config(
        name,
        "command-runner.jar",
        json!(["spark-submit", "--deploy-mode", "cluster", "s3://jobs/job.py"]),
        Value::Null,
    )
}

fn cases() -> [Case; 6] {
    [
        Case {
            label: "launched jar with args",
            added_later: false,
            config: jar_with_args,
        },
        Case {
            label: "launched jar with main class",
            added_later: false,
            config: jar_with_main_class,
        },
        Case {
            label: "launched spark-submit",
            added_later: false,
            config: spark_submit,
        },
        Case {
            label: "added jar with args",
            added_later: true,
            config: jar_with_args,
        },
        Case {
            label: "added jar with main class",
            added_later: true,
            config: jar_with_main_class,
        },
        Case {
            label: "added spark-submit",
            added_later: true,
            config: spark_submit,
        },
    ]
}

/// The `AdditionalInfo` `ListSteps` reports for one step of `cluster_id`.
async fn listed_additional_info(emr: &Emr, cluster_id: &str, step_id: &str) -> Value {
    let listed = emr
        .list_steps(json!({ "ClusterId": cluster_id, "StepIds": [step_id] }))
        .await;
    at(&listed, &["Steps"])
        .as_array()
        .and_then(|steps| steps.first().cloned())
        .map(|step| step["AdditionalInfo"].clone())
        .unwrap_or(Value::Null)
}

/// Assert `info` is the modelled structure: exactly the modelled members, each at its modelled
/// type, and the application ids it carries.
fn modelled_members(case: &str, info: &Value) -> Vec<String> {
    let members = info
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: AdditionalInfo must be a structure, got {info}"));
    let served: BTreeSet<&str> = members.keys().map(String::as_str).collect();
    let modelled: BTreeSet<&str> = MODELLED_MEMBERS.into_iter().collect();
    assert_eq!(
        served, modelled,
        "case {case:?}: AdditionalInfo must carry exactly the members the shape declares, \
         got {info}"
    );
    assert_eq!(
        members["ResourceManager"].as_str(),
        Some(RESOURCE_MANAGER),
        "case {case:?}: ResourceManager must name the manager the step ran under, got {info}"
    );
    let ids = members["ResourceManagerApplicationIds"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: ResourceManagerApplicationIds must be a list, got {info}")
        });
    ids.iter()
        .map(|id| {
            id.as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every application id must be a string, got {info}")
                })
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn a_step_serves_its_additional_info_as_the_modelled_structure() {
    let cases = cases();
    let emr = Emr::new();

    // One cluster runs the whole table, with a slot per step so every row's execution starts.
    let mut launch = kept_alive_request("additional info members");
    launch["StepConcurrencyLevel"] = json!(i32::try_from(cases.len()).unwrap_or(1));
    launch["Steps"] = Value::Array(
        cases
            .iter()
            .filter(|case| !case.added_later)
            .map(|case| (case.config)(case.label))
            .collect(),
    );
    let cluster_id = emr.run_job_flow(launch).await;

    // The steps the launch did not name enter the queue through `AddJobFlowSteps`.
    let added: Vec<Value> = cases
        .iter()
        .filter(|case| case.added_later)
        .map(|case| (case.config)(case.label))
        .collect();
    let added_count = added.len();
    let out = emr
        .ok(
            "AddJobFlowSteps",
            json!({ "JobFlowId": &cluster_id, "Steps": added }),
        )
        .await;
    assert_eq!(
        out["StepIds"].as_array().map(Vec::len),
        Some(added_count),
        "AddJobFlowSteps must answer one id per submitted step, got {out}"
    );

    // Every step of the cluster, by the name its case submitted it under.
    let listed = emr.list_steps(json!({ "ClusterId": &cluster_id })).await;
    let queue = listed["Steps"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("ListSteps must answer the cluster's queue, got {listed}"));
    assert_eq!(
        queue.len(),
        cases.len(),
        "every submitted step must be queued, got {listed}"
    );

    // A step whose execution has not started has reached no resource manager, so neither read
    // serves it any `AdditionalInfo`.
    for step in &queue {
        if at(step, &["Status", "State"]).as_str() != Some("PENDING") {
            continue;
        }
        let name = at(step, &["Name"]).as_str().unwrap_or_default().to_string();
        assert!(
            step["AdditionalInfo"].is_null(),
            "case {name:?}: a step whose execution has not started serves no AdditionalInfo, \
             got {step}"
        );
        let step_id = at(step, &["Id"]).as_str().unwrap_or_default().to_string();
        assert!(
            listed_additional_info(&emr, &cluster_id, &step_id)
                .await
                .is_null(),
            "case {name:?}: ListSteps must agree that a queued step serves no AdditionalInfo"
        );
    }

    let mut seen: BTreeSet<String> = BTreeSet::new();
    for step in &queue {
        let name = at(step, &["Name"]).as_str().unwrap_or_default().to_string();
        let case = cases
            .iter()
            .find(|case| case.label == name)
            .unwrap_or_else(|| panic!("the queue must hold only the table's steps, got {name:?}"));
        let label = case.label;
        let step_id = at(step, &["Id"]).as_str().unwrap_or_default().to_string();

        let settled = await_settled_step(&emr, &cluster_id, &step_id).await;
        let info = settled["AdditionalInfo"].clone();
        let ids = modelled_members(label, &info);
        assert_eq!(
            ids.len(),
            1,
            "case {label:?}: one execution submits one application, got {ids:?}"
        );
        assert!(
            ids.iter().all(|id| !id.is_empty()),
            "case {label:?}: every application id must be named, got {ids:?}"
        );
        assert!(
            seen.insert(ids[0].clone()),
            "case {label:?}: two steps must not share an application id, got {ids:?} among \
             {seen:?}"
        );

        // The same structure on the listing, and the same again on a second read.
        assert_eq!(
            listed_additional_info(&emr, &cluster_id, &step_id).await,
            info,
            "case {label:?}: ListSteps must serve the step's AdditionalInfo as DescribeStep does"
        );
        let reread = emr
            .ok(
                "DescribeStep",
                json!({ "ClusterId": &cluster_id, "StepId": &step_id }),
            )
            .await["Step"]["AdditionalInfo"]
            .clone();
        assert_eq!(
            reread, info,
            "case {label:?}: a step's AdditionalInfo must not change between reads"
        );
    }
}
