// SPDX-License-Identifier: CC0-1.0

//! The REQUEST contracts of the compute-environment and job planes, and the PARAMETER
//! SUBSTITUTION a job's command carries — end to end over the wire.
//!
//! Four rules are under test, each over a SET of inputs.
//!
//! **Compute resources launched into subnets must name one.** `ComputeResource$subnets` is
//! "the VPC subnets where the compute resources are launched … This parameter is required for
//! compute environments using `EC2`, `SPOT`, `FARGATE`, or `FARGATE_SPOT` compute resources."
//! So a create whose `computeResources` names one of those types and no subnet is the
//! modelled `ClientException` (HTTP 400) and leaves no compute environment behind, while the
//! remaining `CRType` member is not held to the rule.
//!
//! **A job timeout below the documented minimum is refused.**
//! `JobTimeout$attemptDurationSeconds`: "The minimum value for the timeout is 60 seconds." So
//! a submit naming fewer is refused and creates no job in any status; 60 itself is accepted.
//!
//! **A consumable resource a job claims must exist.**
//! `ConsumableResourceRequirement$consumableResource` is "the name or ARN of the consumable
//! resource", so a submit whose list names one this service holds none of is refused with the
//! ARN that reference resolves to, whether the list came from the submit's own
//! `consumableResourcePropertiesOverride` or from the job definition it runs.
//!
//! **Amazon EC2-only compute-resource members cannot be updated on a Fargate environment.**
//! `ComputeResource$instanceRole` — "This parameter isn't applicable to jobs that are running
//! on Fargate resources. Don't specify it." — and the same note stands on every other member
//! of the set AWS Batch names in that refusal. So an `UpdateComputeEnvironment` naming any of
//! them for an environment whose compute resources run on Fargate is refused with the one
//! sentence that names the whole set, while the members that DO apply are accepted on the
//! same environment.
//!
//! **A job's command carries parameter substitution placeholders.**
//! `RegisterJobDefinition$parameters` is "default parameter substitution placeholders to set
//! in the job definition" and `SubmitJob$parameters` is "additional parameters passed to the
//! job that replace parameter substitution placeholders that are set in the job definition …
//! Parameters in a `SubmitJob` request override any corresponding parameter defaults from the
//! job definition". So the program the container is given is its declared command with those
//! values substituted into it, and the status that program exits with is the job's.

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state;

use crate::array_jobs_and_placement::managed_compute_environment;
use crate::job_lifecycle::{
    arn, compute_environment, container_properties, error_code, error_message, job_definition,
    job_queue, until_status, Batch,
};

/// The modelled error every refusal below carries. Source:
/// `smithy_query(query="operation",operation="CreateComputeEnvironment,SubmitJob,UpdateComputeEnvironment")`.
const CLIENT_EXCEPTION: &str = "ClientException";

/// The `CRType` members, spelled out rather than imported so a wrong constant in the
/// implementation cannot make this pass. Source:
/// `smithy_query(query="shape",shape="CRType")`: EC2 | SPOT | FARGATE | FARGATE_SPOT |
/// ECS_MANAGED_INSTANCES.
const COMPUTE_TYPES: [&str; 5] = [
    "EC2",
    "SPOT",
    "FARGATE",
    "FARGATE_SPOT",
    "ECS_MANAGED_INSTANCES",
];

/// The `JobStatus` members this suite names. Source:
/// `smithy_query(query="shape",shape="JobStatus")`.
const PENDING: &str = "PENDING";
const SUCCEEDED: &str = "SUCCEEDED";
const FAILED: &str = "FAILED";

/// The sentence `UpdateComputeEnvironment` refuses an Amazon EC2-only member with, spelled
/// out here rather than imported so a wrong constant in the implementation cannot make this
/// pass. Wholly fixed text: it names the member set that does not apply to Fargate compute
/// resources and nothing of the request.
const FARGATE_PARAMETERS_SENTENCE: &str = "The following parameters can't be updated for \
     Fargate compute environment: allocationStrategy, bidPercentage, ec2Configuration, \
     ec2KeyPair, imageId, instanceRole, instanceTypes, launchTemplate, placementGroup, tags, \
     type, updatePolicy, updateToLatestImageVersion.";

/// The detail a compute environment created over compute resources launched into subnets that
/// names none is refused with, spelled out here for the same reason.
const SUBNETS_REQUIRED_SENTENCE: &str = "Resource subnets are required.";

/// The detail a submit whose `timeout.attemptDurationSeconds` is below the documented minimum
/// is refused with, spelled out here for the same reason.
const ATTEMPT_DURATION_SENTENCE: &str =
    "AttemptDurationSeconds in Timeout must be at least 60 seconds.";

/// How Batch's frontend wraps an input-validation refusal, as
/// `crate::state::client_exception_message` builds it: the detail, then the id of the call
/// being refused. The id is the response's own, so only the FIXED text is asserted here.
fn assert_enveloped(case: &str, body: &Value, detail: &str) {
    let message = error_message(body);
    let expected = format!("Error executing request, Exception : {detail}, RequestId: ");
    assert!(
        message.starts_with(&expected) && message.len() > expected.len(),
        "case {case}: expected the enveloped detail {expected:?} with this call's request id, \
         got {message:?}"
    );
}

/// RULE: compute resources of a type that is launched into subnets must name at least one;
/// the refusal is the modelled `ClientException` and nothing is created.
///
/// Rows differ along what the rule generalises over: the compute `type` (every member of the
/// model's own `CRType`, including the one the rule does NOT cover), and whether `subnets` is
/// absent, empty, or names one.
#[tokio::test]
async fn compute_resources_launched_into_subnets_must_name_one() {
    struct Row {
        case: &'static str,
        compute_type: &'static str,
        /// The `subnets` member of the request, or `None` to leave it out entirely.
        subnets: Option<Value>,
        /// Whether the create must be refused.
        refused: bool,
    }

    let mut rows: Vec<Row> = Vec::new();
    for compute_type in COMPUTE_TYPES {
        let launched_into_subnets = compute_type != "ECS_MANAGED_INSTANCES";
        rows.push(Row {
            case: "no subnets member at all",
            compute_type,
            subnets: None,
            refused: launched_into_subnets,
        });
        rows.push(Row {
            case: "an empty subnets list",
            compute_type,
            subnets: Some(json!([])),
            refused: launched_into_subnets,
        });
        rows.push(Row {
            case: "one subnet",
            compute_type,
            subnets: Some(json!(["subnet-1111"])),
            refused: false,
        });
        rows.push(Row {
            case: "two subnets",
            compute_type,
            subnets: Some(json!(["subnet-1111", "subnet-2222"])),
            refused: false,
        });
    }

    let batch = Batch::new();
    for (index, row) in rows.iter().enumerate() {
        let case = format!("{} with {}", row.compute_type, row.case);
        let name = format!("ce-subnets-{index}");
        let mut resources = json!({"type": row.compute_type, "maxvCpus": 16});
        if let Some(subnets) = row.subnets.clone() {
            resources["subnets"] = subnets;
        }
        let (status, body) = batch
            .call(
                "/v1/createcomputeenvironment",
                json!({
                    "computeEnvironmentName": &name,
                    "type": "MANAGED",
                    "state": "ENABLED",
                    "computeResources": resources,
                }),
            )
            .await;
        if row.refused {
            assert_eq!(
                status, 400,
                "case {case}: expected the modelled 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                CLIENT_EXCEPTION,
                "case {case}: expected {CLIENT_EXCEPTION}, got {body}"
            );
            assert_enveloped(&case, &body, SUBNETS_REQUIRED_SENTENCE);
            // Nothing is left behind: the refusal happens before any state is written.
            let listed = batch
                .ok(
                    "/v1/describecomputeenvironments",
                    json!({"computeEnvironments": [&name]}),
                )
                .await;
            assert_eq!(
                listed["computeEnvironments"].as_array().map(Vec::len),
                Some(0),
                "case {case}: a refused create leaves no compute environment, got {listed}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200, got {status} with {body}"
            );
            assert_eq!(
                body["computeEnvironmentArn"].as_str(),
                Some(arn(&format!("compute-environment/{name}")).as_str()),
                "case {case}: expected the minted ARN, got {body}"
            );
        }
    }
}

/// RULE: `timeout.attemptDurationSeconds` below the documented minimum is refused and creates
/// no job; the minimum itself and anything above it are accepted.
///
/// Rows span the published bound and one past it on either side, plus the absent member.
#[tokio::test]
async fn a_job_timeout_below_the_documented_minimum_is_refused() {
    let batch = Batch::new();
    let environment = compute_environment(&batch, "ce-timeout").await;
    let queue_arn = job_queue(&batch, "q-timeout", &environment).await;
    let (definition, _) = job_definition(&batch, "jd-timeout", container_properties()).await;

    // (case, the attemptDurationSeconds the submit names, whether it must be refused)
    let minimum = state::MIN_ATTEMPT_DURATION_SECONDS;
    let rows: &[(&str, Option<i32>, bool)] = &[
        ("no timeout at all", None, false),
        ("the published minimum", Some(minimum), false),
        ("one second above the minimum", Some(minimum + 1), false),
        ("an hour", Some(3_600), false),
        ("one second below the minimum", Some(minimum - 1), true),
        ("a single second", Some(1), true),
        ("zero", Some(0), true),
        ("a negative duration", Some(-1), true),
    ];

    for (case, seconds, refused) in rows {
        let mut request = json!({
            "jobName": "timeout-probe",
            "jobQueue": &queue_arn,
            "jobDefinition": &definition,
        });
        if let Some(seconds) = seconds {
            request["timeout"] = json!({"attemptDurationSeconds": seconds});
        }
        let (status, body) = batch.call("/v1/submitjob", request).await;
        if *refused {
            assert_eq!(
                status, 400,
                "case {case}: expected the modelled 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                CLIENT_EXCEPTION,
                "case {case}: expected {CLIENT_EXCEPTION}, got {body}"
            );
            assert_enveloped(case, &body, ATTEMPT_DURATION_SENTENCE);
            assert!(
                body["jobId"].is_null(),
                "case {case}: a refused submit creates no job, got {body}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200, got {status} with {body}"
            );
            assert!(
                body["jobId"].as_str().is_some_and(|id| !id.is_empty()),
                "case {case}: expected a jobId, got {body}"
            );
        }
    }
}

/// RULE: every `consumableResourceList` entry of the job a submit would create must name a
/// consumable resource this service holds, whichever of the two identifier forms it uses and
/// whether the list came from the submit or from the job definition.
#[tokio::test]
async fn a_job_claiming_an_unregistered_consumable_resource_is_refused() {
    let batch = Batch::new();
    let environment = compute_environment(&batch, "ce-consumable").await;
    let queue_arn = job_queue(&batch, "q-consumable", &environment).await;
    batch
        .ok(
            "/v1/createconsumableresource",
            json!({"consumableResourceName": "registered", "totalQuantity": 8}),
        )
        .await;
    let registered_arn = arn("consumable-resource/registered");
    let unregistered_arn = arn("consumable-resource/never-registered");

    let (plain, _) = job_definition(&batch, "jd-consumable", container_properties()).await;
    // A definition that CARRIES the claim itself, so the rule is read off the job the submit
    // would create and not only off the override member.
    let claiming = batch
        .ok(
            "/v1/registerjobdefinition",
            json!({
                "jobDefinitionName": "jd-consumable-claiming",
                "type": "container",
                "containerProperties": container_properties(),
                "consumableResourceProperties": {
                    "consumableResourceList": [
                        {"consumableResource": "never-registered", "quantity": 1},
                    ],
                },
            }),
        )
        .await;
    let claiming = claiming["jobDefinitionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case register claiming definition: got {claiming}"))
        .to_string();

    struct Row {
        case: &'static str,
        definition: &'static str,
        /// The `consumableResourcePropertiesOverride` the submit names, if any.
        list: Option<Value>,
        /// The ARN the refusal must name, or `None` when the submit must be accepted.
        refused_arn: Option<String>,
    }

    let rows: Vec<Row> = vec![
        Row {
            case: "no consumable resources at all",
            definition: "plain",
            list: None,
            refused_arn: None,
        },
        Row {
            case: "an override naming a registered resource by name",
            definition: "plain",
            list: Some(json!([{"consumableResource": "registered", "quantity": 1}])),
            refused_arn: None,
        },
        Row {
            case: "an override naming a registered resource by ARN",
            definition: "plain",
            list: Some(json!([{"consumableResource": &registered_arn, "quantity": 2}])),
            refused_arn: None,
        },
        Row {
            case: "an override naming an unregistered resource by name",
            definition: "plain",
            list: Some(json!([{"consumableResource": "never-registered", "quantity": 1}])),
            refused_arn: Some(unregistered_arn.clone()),
        },
        Row {
            case: "an override naming an unregistered resource by ARN",
            definition: "plain",
            list: Some(json!([{"consumableResource": &unregistered_arn, "quantity": 1}])),
            refused_arn: Some(unregistered_arn.clone()),
        },
        Row {
            case: "an override whose second entry is unregistered",
            definition: "plain",
            list: Some(json!([
                {"consumableResource": "registered", "quantity": 1},
                {"consumableResource": "never-registered", "quantity": 1},
            ])),
            refused_arn: Some(unregistered_arn.clone()),
        },
        Row {
            case: "an empty override list",
            definition: "plain",
            list: Some(json!([])),
            refused_arn: None,
        },
        Row {
            case: "the job definition's own unregistered claim",
            definition: "claiming",
            list: None,
            refused_arn: Some(unregistered_arn.clone()),
        },
        Row {
            case: "an override that replaces the definition's unregistered claim",
            definition: "claiming",
            list: Some(json!([{"consumableResource": "registered", "quantity": 1}])),
            refused_arn: None,
        },
    ];

    for row in &rows {
        let case = row.case;
        let definition = match row.definition {
            "claiming" => &claiming,
            _ => &plain,
        };
        let mut request = json!({
            "jobName": "consumable-probe",
            "jobQueue": &queue_arn,
            "jobDefinition": definition,
        });
        if let Some(list) = row.list.clone() {
            request["consumableResourcePropertiesOverride"] =
                json!({"consumableResourceList": list});
        }
        let (status, body) = batch.call("/v1/submitjob", request).await;
        match row.refused_arn.as_deref() {
            Some(expected_arn) => {
                assert_eq!(
                    status, 400,
                    "case {case}: expected the modelled 400, got {status} with {body}"
                );
                assert_eq!(
                    error_code(&body),
                    CLIENT_EXCEPTION,
                    "case {case}: expected {CLIENT_EXCEPTION}, got {body}"
                );
                assert_eq!(
                    error_message(&body),
                    format!("Consumable resource {expected_arn} does not exist."),
                    "case {case}: expected the not-found sentence for {expected_arn}, got {body}"
                );
                assert!(
                    body["jobId"].is_null(),
                    "case {case}: a refused submit creates no job, got {body}"
                );
            }
            None => assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200, got {status} with {body}"
            ),
        }
    }
}

/// RULE: an `UpdateComputeEnvironment` naming an Amazon EC2-only compute-resource member for
/// an environment whose compute resources run on Fargate is refused with the one sentence
/// that names the whole set; the members that DO apply to Fargate are accepted on the same
/// environment, and the same EC2-only members are accepted on an EC2 environment.
#[tokio::test]
async fn ec2_only_compute_resource_members_are_refused_on_a_fargate_environment() {
    let batch = Batch::new();
    let fargate = managed_compute_environment(&batch, "ce-fargate-gate", state::STATE_ENABLED)
        .await;
    batch
        .ok(
            "/v1/createcomputeenvironment",
            json!({
                "computeEnvironmentName": "ce-ec2-gate",
                "type": "MANAGED",
                "state": "ENABLED",
                "computeResources": {
                    "type": "EC2",
                    "maxvCpus": 16,
                    "subnets": ["subnet-1111"],
                    "instanceRole": "arn:aws:iam::123456789012:instance-profile/ecsInstanceRole",
                },
            }),
        )
        .await;

    // (case, the members of the update request, whether a FARGATE environment refuses it)
    let rows: &[(&str, Value, bool)] = &[
        (
            "instanceRole",
            json!({"computeResources": {"instanceRole": "arn:aws:iam::123456789012:instance-profile/bogus"}}),
            true,
        ),
        (
            "allocationStrategy",
            json!({"computeResources": {"allocationStrategy": "BEST_FIT_PROGRESSIVE"}}),
            true,
        ),
        (
            "instanceTypes",
            json!({"computeResources": {"instanceTypes": ["optimal"]}}),
            true,
        ),
        ("imageId", json!({"computeResources": {"imageId": "ami-1234"}}), true),
        ("bidPercentage", json!({"computeResources": {"bidPercentage": 50}}), true),
        ("ec2KeyPair", json!({"computeResources": {"ec2KeyPair": "kp"}}), true),
        (
            "placementGroup",
            json!({"computeResources": {"placementGroup": "pg"}}),
            true,
        ),
        (
            "instance tags",
            json!({"computeResources": {"tags": {"Name": "batch"}}}),
            true,
        ),
        (
            "the compute type itself",
            json!({"computeResources": {"type": "EC2"}}),
            true,
        ),
        (
            "updateToLatestImageVersion",
            json!({"computeResources": {"updateToLatestImageVersion": true}}),
            true,
        ),
        (
            "updatePolicy, which is the request's own member",
            json!({"updatePolicy": {"terminateJobsOnUpdate": true, "jobExecutionTimeoutMinutes": 30}}),
            true,
        ),
        (
            "an EC2-only member beside one that applies",
            json!({"computeResources": {"maxvCpus": 32, "imageId": "ami-5678"}}),
            true,
        ),
        // The members that DO apply to Fargate compute resources.
        ("maxvCpus", json!({"computeResources": {"maxvCpus": 32}}), false),
        (
            "subnets",
            json!({"computeResources": {"subnets": ["subnet-3333"]}}),
            false,
        ),
        (
            "securityGroupIds",
            json!({"computeResources": {"securityGroupIds": ["sg-3333"]}}),
            false,
        ),
        ("state", json!({"state": "DISABLED"}), false),
        ("nothing but the identity", json!({}), false),
    ];

    for (case, members, refused_on_fargate) in rows {
        for (environment, kind) in [(&fargate, "FARGATE"), (&"ce-ec2-gate".to_string(), "EC2")] {
            let case = format!("{case} on the {kind} environment");
            let mut request = json!({"computeEnvironment": environment});
            for (member, value) in members.as_object().cloned().unwrap_or_default() {
                request[member] = value;
            }
            let (status, body) = batch.call("/v1/updatecomputeenvironment", request).await;
            if *refused_on_fargate && kind == "FARGATE" {
                assert_eq!(
                    status, 400,
                    "case {case}: expected the modelled 400, got {status} with {body}"
                );
                assert_eq!(
                    error_code(&body),
                    CLIENT_EXCEPTION,
                    "case {case}: expected {CLIENT_EXCEPTION}, got {body}"
                );
                assert_eq!(
                    error_message(&body),
                    FARGATE_PARAMETERS_SENTENCE,
                    "case {case}: expected the Fargate parameter sentence, got {body}"
                );
            } else {
                assert_eq!(
                    status, 200,
                    "case {case}: expected HTTP 200, got {status} with {body}"
                );
            }
        }
    }
}

/// The container properties of a definition whose program's exit status is a PARAMETER.
///
/// The command is the one a Fargate container job declares — a shell and a line for it — and
/// the line carries the placeholder, so the status the program exits with is whatever the
/// job's effective parameters put there.
fn parameterised_container() -> Value {
    json!({
        "image": "busybox",
        "command": ["/bin/sh", "-c", "exit Ref::code"],
        "resourceRequirements": [
            {"type": "VCPU", "value": "1"},
            {"type": "MEMORY", "value": "512"},
        ],
    })
}

/// RULE: the program a job's container is given is its declared command with the job's
/// effective parameters substituted into every `Ref::<name>` placeholder — the submit's
/// `parameters` over the job definition's defaults — so the status that program exits with
/// decides the job's terminal status and its `container.exitCode`, and the command the job
/// reports is the resolved one.
///
/// Rows differ along where the value comes from (the definition's default, the submit's
/// override, both) and the status it makes the program exit with.
#[tokio::test]
async fn a_container_command_substitutes_the_jobs_effective_parameters() {
    let batch = Batch::new();
    let environment =
        managed_compute_environment(&batch, "ce-parameters", state::STATE_ENABLED).await;
    let queue_arn = job_queue(&batch, "q-parameters", &environment).await;
    let definition = batch
        .ok(
            "/v1/registerjobdefinition",
            json!({
                "jobDefinitionName": "jd-parameters",
                "type": "container",
                "containerProperties": parameterised_container(),
                "parameters": {"code": "0"},
            }),
        )
        .await;
    let definition = definition["jobDefinitionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case register: expected an ARN, got {definition}"))
        .to_string();

    // (case, the submit's own parameters, the exit status the program must reach, the
    // terminal status that status decides, the resolved program the job must report)
    let rows: &[(&str, Option<Value>, i64, &str, &str)] = &[
        ("the definition's default", None, 0, SUCCEEDED, "exit 0"),
        (
            "an override that keeps the default value",
            Some(json!({"code": "0"})),
            0,
            SUCCEEDED,
            "exit 0",
        ),
        (
            "an override the definition has a default for",
            Some(json!({"code": "7"})),
            7,
            FAILED,
            "exit 7",
        ),
        (
            "an override naming another status",
            Some(json!({"code": "1"})),
            1,
            FAILED,
            "exit 1",
        ),
        (
            "an override beside a parameter nothing substitutes",
            Some(json!({"code": "0", "unused": "9"})),
            0,
            SUCCEEDED,
            "exit 0",
        ),
    ];

    for (index, (case, parameters, exit_code, terminal, resolved)) in rows.iter().enumerate() {
        let mut request = json!({
            "jobName": format!("param-{index}"),
            "jobQueue": &queue_arn,
            "jobDefinition": &definition,
        });
        if let Some(parameters) = parameters.clone() {
            request["parameters"] = parameters;
        }
        let body = batch.ok("/v1/submitjob", request).await;
        let job_id = body["jobId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected a jobId, got {body}"))
            .to_string();
        let detail = until_status(&batch, case, &job_id, terminal).await;
        assert_eq!(
            detail["container"]["exitCode"].as_i64(),
            Some(*exit_code),
            "case {case}: expected the substituted program to exit {exit_code}, got {detail}"
        );
        // The RECORD is the container the job RUNS: the values stand where the placeholders
        // were, beside the parameters they came from, so a client reads what ran.
        assert_eq!(
            detail["container"]["command"],
            json!(["/bin/sh", "-c", resolved]),
            "case {case}: expected the resolved command to be reported, got {detail}"
        );
    }
}

/// RULE: a child of an array job whose `N_TO_N` dependency has not finished is HELD in
/// `PENDING` — "each index child of this job must wait for the corresponding index child of
/// each dependency" (`SubmitJob$dependsOn`) — and is released only once that child has
/// finished.
#[tokio::test]
async fn an_array_child_waits_in_pending_for_the_matching_index_of_its_dependency() {
    // A queue with no capacity of its own, so the depended-on array rests in RUNNABLE and the
    // dependents stay held for the whole test.
    let batch = Batch::new();
    let environment = compute_environment(&batch, "ce-n-to-n").await;
    let queue_arn = job_queue(&batch, "q-n-to-n", &environment).await;
    let (definition, _) = job_definition(&batch, "jd-n-to-n", container_properties()).await;

    let first = batch
        .ok(
            "/v1/submitjob",
            json!({
                "jobName": "upstream",
                "jobQueue": &queue_arn,
                "jobDefinition": &definition,
                "arrayProperties": {"size": state::MIN_ARRAY_SIZE},
            }),
        )
        .await;
    let first = first["jobId"]
        .as_str()
        .unwrap_or_else(|| panic!("case submit upstream: got {first}"))
        .to_string();
    let second = batch
        .ok(
            "/v1/submitjob",
            json!({
                "jobName": "downstream",
                "jobQueue": &queue_arn,
                "jobDefinition": &definition,
                "arrayProperties": {"size": state::MIN_ARRAY_SIZE},
                "dependsOn": [{"jobId": &first, "type": "N_TO_N"}],
            }),
        )
        .await;
    let second = second["jobId"]
        .as_str()
        .unwrap_or_else(|| panic!("case submit downstream: got {second}"))
        .to_string();

    for index in 0..state::MIN_ARRAY_SIZE {
        let child = state::array_child_job_id(&second, index);
        let case = format!("child {index} of the dependent array");
        let detail = until_status(&batch, &case, &child, PENDING).await;
        // It waits for the matching index of the depended-on array, and for nothing else.
        assert_eq!(
            detail["dependsOn"],
            json!([{"jobId": state::array_child_job_id(&first, index), "type": "N_TO_N"}]),
            "case {case}: expected the matching index as the one dependency, got {detail}"
        );
        assert!(
            detail["startedAt"].is_null(),
            "case {case}: a held job has not started, got {detail}"
        );
    }
    // And the parent of the held array is PENDING too, as the model's own note says: "array
    // job parents are updated to PENDING when any child job is updated to RUNNABLE and
    // remain in PENDING status while child jobs are running".
    let parent = until_status(&batch, "the dependent array's parent", &second, PENDING).await;
    assert!(
        parent["arrayProperties"]["statusSummary"][PENDING]
            .as_i64()
            .unwrap_or_default()
            >= i64::from(state::MIN_ARRAY_SIZE),
        "case the dependent array's parent: every child is held, got {parent}"
    );
}

/// RULE: `GetJobQueueSnapshot`'s `queueUtilization.totalCapacityUsage` is "capacity
/// utilization for already dispatched jobs", so it reports what the queue's dispatched jobs
/// are HOLDING — for a container job, the resources its container reserves — and reports
/// nothing once they have given it back.
#[tokio::test]
async fn a_queue_snapshot_reports_the_capacity_its_dispatched_jobs_hold() {
    let batch = Batch::new();
    let environment =
        managed_compute_environment(&batch, "ce-snapshot-usage", state::STATE_ENABLED).await;
    let queue_arn = job_queue(&batch, "q-snapshot-usage", &environment).await;
    // (case, the vCPUs and MiB the container reserves)
    let rows: &[(&str, &str, &str)] = &[
        ("one vCPU", "1", "2048"),
        ("two vCPUs", "2", "4096"),
        ("a quarter of a vCPU", "0.25", "512"),
    ];

    for (index, (case, vcpus, memory)) in rows.iter().enumerate() {
        let (definition, _) = job_definition(
            &batch,
            &format!("jd-snapshot-usage-{index}"),
            json!({
                "image": "busybox",
                "command": ["/bin/sh", "-c", "sleep 300"],
                "resourceRequirements": [
                    {"type": "VCPU", "value": vcpus},
                    {"type": "MEMORY", "value": memory},
                ],
            }),
        )
        .await;
        let body = batch
            .ok(
                "/v1/submitjob",
                json!({
                    "jobName": format!("holder-{index}"),
                    "jobQueue": &queue_arn,
                    "jobDefinition": &definition,
                }),
            )
            .await;
        let job_id = body["jobId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected a jobId, got {body}"))
            .to_string();
        // Wait for the job to reach a status in which it HOLDS capacity, then read the
        // snapshot: the quantities are the reservation its container states.
        until_status(&batch, case, &job_id, "RUNNING").await;
        let snapshot = batch
            .ok("/v1/getjobqueuesnapshot", json!({"jobQueue": &queue_arn}))
            .await;
        let usage = snapshot["queueUtilization"]["totalCapacityUsage"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let vcpu_entry = usage
            .iter()
            .find(|entry| entry["capacityUnit"].as_str() == Some(state::RESOURCE_TYPE_VCPU))
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case}: expected a VCPU capacity usage entry, got {snapshot}")
            });
        let expected: f64 = vcpus.parse().unwrap_or_default();
        assert!(
            vcpu_entry["quantity"]
                .as_f64()
                .is_some_and(|quantity| (quantity - expected).abs() < f64::EPSILON),
            "case {case}: expected {expected} vCPU held, got {vcpu_entry}"
        );
        let memory_entry = usage
            .iter()
            .find(|entry| entry["capacityUnit"].as_str() == Some(state::RESOURCE_TYPE_MEMORY))
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case}: expected a MEMORY capacity usage entry, got {snapshot}")
            });
        let expected_memory: f64 = memory.parse().unwrap_or_default();
        assert!(
            memory_entry["quantity"]
                .as_f64()
                .is_some_and(|quantity| (quantity - expected_memory).abs() < f64::EPSILON),
            "case {case}: expected {expected_memory} MiB held, got {memory_entry}"
        );

        // Terminating the job gives the capacity back, so the snapshot stops reporting it.
        batch
            .ok("/v1/terminatejob", json!({"jobId": &job_id, "reason": "done"}))
            .await;
        until_status(&batch, case, &job_id, FAILED).await;
        let snapshot = batch
            .ok("/v1/getjobqueuesnapshot", json!({"jobQueue": &queue_arn}))
            .await;
        assert_eq!(
            snapshot["queueUtilization"]["totalCapacityUsage"]
                .as_array()
                .map(Vec::len),
            Some(0),
            "case {case}: a terminal job holds nothing, got {snapshot}"
        );
    }
}
