// SPDX-License-Identifier: CC0-1.0

//! WHICH layer refuses a value outside a modelled enum, and what that refusal SAYS.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the
//! router's `dispatch`, using the harness `tests/all/job_flow_lifecycle.rs` owns, so
//! these are the same bytes an SDK client sends.
//!
//! THE RULE every table below asserts: Amazon EMR checks the ENUM-valued members of
//! an operation's own input shape itself, ahead of the generic request validator, and
//! refuses a value outside such an enum with its own modelled client-input error
//! `InvalidRequestException` (HTTP 400) — where the operation's model declares that
//! error at all — naming, where the rejected values are elements of ONE
//! collection-valued filter member, those values in the request's own order. An enum
//! carried DEEPER, inside a nested request structure, is reached only by the generic
//! request validator, so it keeps that validator's `ValidationException`, worded in
//! EMR's own prose where EMR validates the structure the value sits in and in the
//! validator's member-path envelope otherwise. A refusal identity is therefore a
//! property of WHERE the offending value sits and of WHICH operation was served, not of
//! the service as a whole.
//!
//! The rows span what the rule generalises over: how deep the offending value sits
//! (a member of the input shape, an element of one of its collections, and two, three
//! and five structure steps down), whether the served operation's model declares EMR's
//! own client error, which enum and which operation, how many values the member carries
//! and which of them are outside the set, the order they were submitted in, and a
//! request that violates both classes at once. The admitted rows are the last part of
//! the rule: a value the model declares is never refused at the boundary, at any depth.

use serde_json::{json, Value};

use crate::input_refusals::error_message;
use crate::job_flow_lifecycle::{at, kept_alive_request, Emr};

/// The client-input error EMR's own model declares, which its own enum check answers on
/// an operation that declares it (`smithy_query(query="operation",
/// operation="ListClusters")` lists `InvalidRequestException`, HTTP 400).
const SELF_CHECKED: &str = "InvalidRequestException";

/// The generic request validator's code — what a caller sees for anything EMR does not
/// check itself, and for an operation whose model declares no client error at all.
/// Recorded for `PutAutoScalingPolicy`'s out-of-enum `ComparisonOperator` and for
/// `RunJobFlow`, which `smithy_query(query="digest")` shows declaring only
/// `InternalServerError`.
const GENERIC: &str = "ValidationException";

/// Every value `com.amazonaws.emr#ClusterState` declares
/// (`smithy_query(query="shape", shape="ClusterState")`).
const CLUSTER_STATES: [&str; 7] = [
    "STARTING",
    "BOOTSTRAPPING",
    "RUNNING",
    "WAITING",
    "TERMINATING",
    "TERMINATED",
    "TERMINATED_WITH_ERRORS",
];

/// Every value `com.amazonaws.emr#InstanceState` declares
/// (`smithy_query(query="shape", shape="InstanceState")`).
const INSTANCE_STATES: [&str; 5] = [
    "AWAITING_FULFILLMENT",
    "PROVISIONING",
    "BOOTSTRAPPING",
    "RUNNING",
    "TERMINATED",
];

/// Every value `com.amazonaws.emr#ComparisonOperator` declares
/// (`smithy_query(query="shape", shape="ComparisonOperator")`), reached five
/// structure steps down `AutoScalingPolicy`.
const COMPARISON_OPERATORS: [&str; 4] = [
    "GREATER_THAN_OR_EQUAL",
    "GREATER_THAN",
    "LESS_THAN",
    "LESS_THAN_OR_EQUAL",
];

/// What a refusal must say.
enum Says {
    /// EMR's own sentence, in full: the filter member's words and the rejected values.
    Exactly(String),
    /// The generic validator's envelope, which must name this member path.
    Names(&'static str),
}

/// One row: a request, the error code it must carry, and what its message must say.
struct Case {
    case: String,
    operation: &'static str,
    body: Value,
    code: &'static str,
    says: Says,
}

/// A refusal EMR's own enum check owns, whose message enumerates the rejected values
/// of one collection-valued filter member.
fn own_sentence(case: &str, operation: &'static str, body: Value, sentence: &str) -> Case {
    Case {
        case: case.to_string(),
        operation,
        body,
        code: SELF_CHECKED,
        says: Says::Exactly(sentence.to_string()),
    }
}

/// A refusal EMR's own enum check owns on a SINGLE-valued member: there is no set of
/// submitted values to enumerate, so the message stays the validator's envelope, which
/// names the member and the value it held.
fn own_envelope(case: &str, operation: &'static str, body: Value, path: &'static str) -> Case {
    Case {
        case: case.to_string(),
        operation,
        body,
        code: SELF_CHECKED,
        says: Says::Names(path),
    }
}

/// A refusal the GENERIC request validator owns: the offending enum sits inside a
/// nested request structure, which EMR's own check does not reach.
fn generic(case: &str, operation: &'static str, body: Value, path: &'static str) -> Case {
    Case {
        case: case.to_string(),
        operation,
        body,
        code: GENERIC,
        says: Says::Names(path),
    }
}

/// A refusal under the generic validator's code that EMR nevertheless words itself:
/// an undeclared value of one of a CloudWatch metric alarm's enum members, stated in
/// EMR's own prose for that member.
fn generic_sentence(case: &str, operation: &'static str, body: Value, sentence: &str) -> Case {
    Case {
        case: case.to_string(),
        operation,
        body,
        code: GENERIC,
        says: Says::Exactly(sentence.to_string()),
    }
}

/// One scaling rule of an `AutoScalingPolicy`, with each of its three nested enums
/// settable so a row can put the offending value at a different depth.
fn scaling_rule(name: &str, comparison: &str, statistic: &str, adjustment: &str) -> Value {
    json!({
        "Name": name,
        "Action": {
            "SimpleScalingPolicyConfiguration": {
                "AdjustmentType": adjustment,
                "ScalingAdjustment": 1,
                "CoolDown": 300,
            },
        },
        "Trigger": {
            "CloudWatchAlarmDefinition": {
                "ComparisonOperator": comparison,
                "EvaluationPeriods": 1,
                "MetricName": "YARNMemoryAvailablePercentage",
                "Namespace": "AWS/ElasticMapReduce",
                "Period": 300,
                "Threshold": 15.0,
                "Statistic": statistic,
                "Unit": "PERCENT",
            },
        },
    })
}

/// A `PutAutoScalingPolicy` body carrying `rules`. The cluster and group ids never
/// matter to these rows: a boundary refusal happens before the handler resolves them.
fn put_policy(cluster_id: &str, group_id: &str, rules: Value) -> Value {
    json!({
        "ClusterId": cluster_id,
        "InstanceGroupId": group_id,
        "AutoScalingPolicy": {
            "Constraints": { "MinCapacity": 1, "MaxCapacity": 2 },
            "Rules": rules,
        },
    })
}

/// A well-formed rule, used as the valid neighbour of an offending one.
fn good_rule(name: &str) -> Value {
    scaling_rule(name, COMPARISON_OPERATORS[1], "AVERAGE", "CHANGE_IN_CAPACITY")
}

/// Every row names a cluster that EXISTS, because a cluster-scoped request's cluster is
/// resolved BEFORE its other members are checked: an unresolvable id would be the
/// refusal, and these rows are about the enum value.
fn refused_cases(cluster: &str) -> Vec<Case> {
    let group = "ig-ABCDEFGHIJKLM";
    vec![
        // ---- EMR's own check, on a collection-valued filter member --------------
        own_sentence(
            "one ClusterStates entry outside ClusterState",
            "ListClusters",
            json!({ "ClusterStates": ["NOPE"] }),
            "Cluster states [NOPE] are not valid.",
        ),
        own_sentence(
            "a declared ClusterState followed by an undeclared one",
            "ListClusters",
            json!({ "ClusterStates": ["RUNNING", "NOPE"] }),
            "Cluster states [NOPE] are not valid.",
        ),
        own_sentence(
            "an undeclared ClusterState between two declared ones",
            "ListClusters",
            json!({ "ClusterStates": ["TERMINATED_WITH_ERRORS", "bogus", "STARTING"] }),
            "Cluster states [bogus] are not valid.",
        ),
        // Enum values are case-sensitive, so a declared value in lower case is
        // outside the set — and both rejected values are named, in the order the
        // request spelled them.
        own_sentence(
            "two undeclared ClusterStates, the second a declared value in lower case",
            "ListClusters",
            json!({ "ClusterStates": ["NOPE", "running"] }),
            "Cluster states [NOPE, running] are not valid.",
        ),
        own_sentence(
            "the same two undeclared ClusterStates in the other order",
            "ListClusters",
            json!({ "ClusterStates": ["running", "NOPE"] }),
            "Cluster states [running, NOPE] are not valid.",
        ),
        own_sentence(
            "every ClusterStates entry undeclared",
            "ListClusters",
            json!({ "ClusterStates": ["one", "two", "three"] }),
            "Cluster states [one, two, three] are not valid.",
        ),
        // The same rule on another operation's filter, so the sentence is built from
        // the member rather than written for ListClusters.
        own_sentence(
            "a StepStates entry outside StepState",
            "ListSteps",
            json!({ "ClusterId": cluster, "StepStates": ["NOT_A_STATE"] }),
            "Step states [NOT_A_STATE] are not valid.",
        ),
        own_sentence(
            "a SessionStates entry outside SessionState",
            "ListSessions",
            json!({ "ClusterId": cluster, "SessionStates": ["NOPE"] }),
            "Session states [NOPE] are not valid.",
        ),
        own_sentence(
            "an InstanceStates entry outside InstanceState",
            "ListInstances",
            json!({ "ClusterId": cluster, "InstanceStates": ["NOPE"] }),
            "Instance states [NOPE] are not valid.",
        ),
        // A filter member whose name is three words, so the sentence's wording is
        // derived from the member and not from a per-member table.
        own_sentence(
            "an InstanceGroupTypes entry outside InstanceGroupType",
            "ListInstances",
            json!({ "ClusterId": cluster, "InstanceGroupTypes": ["PRIMARY"] }),
            "Instance group types [PRIMARY] are not valid.",
        ),
        // A request violating the enum class AND another class at once is refused as
        // the enum class: that is the check EMR runs first.
        own_sentence(
            "an undeclared SessionStates entry beside a MaxResults out of range",
            "ListSessions",
            json!({
                "ClusterId": cluster,
                "SessionStates": ["NOPE"],
                "MaxResults": 101,
            }),
            "Session states [NOPE] are not valid.",
        ),
        // ---- EMR's own check, on a single-valued member of the input shape ------
        own_envelope(
            "a Status outside NotebookExecutionStatus",
            "ListNotebookExecutions",
            json!({ "Status": "NOT_A_REAL_STATUS" }),
            "status",
        ),
        own_envelope(
            "an IdentityType outside IdentityType",
            "ListStudioSessionMappings",
            json!({ "IdentityType": "PERSON" }),
            "identityType",
        ),
        own_envelope(
            "an InstanceFleetType outside InstanceFleetType",
            "ListInstances",
            json!({ "ClusterId": cluster, "InstanceFleetType": "WORKER" }),
            "instanceFleetType",
        ),
        own_envelope(
            "a ProfilerType outside ProfilerType",
            "CreatePersistentAppUI",
            json!({
                "TargetResourceArn":
                    "arn:aws:elasticmapreduce:us-east-1:000000000000:cluster/j-ABCDEFGHIJKLM",
                "ProfilerType": "SPARKUI",
            }),
            "profilerType",
        ),
        // An undeclared value on a member of the input shape of an operation whose
        // model declares NO client error: `RunJobFlow` declares only
        // `InternalServerError` (HTTP 500), so EMR's own client-input error is an
        // identity it cannot answer and the generic one stands — and it stands on the
        // SHALLOWER violation, reported beside the one nested five steps down.
        generic(
            "an undeclared ScaleDownBehavior beside an undeclared nested Market",
            "RunJobFlow",
            json!({
                "Name": "both classes at once",
                "ReleaseLabel": "emr-6.15.0",
                "ScaleDownBehavior": "NOPE",
                "Instances": {
                    "KeepJobFlowAliveWhenNoSteps": true,
                    "InstanceGroups": [{
                        "Name": "primary",
                        "InstanceRole": "MASTER",
                        "InstanceType": "m5.xlarge",
                        "InstanceCount": 1,
                        "Market": "CHEAP",
                    }],
                },
            }),
            "scaleDownBehavior",
        ),
        // A request whose input-member enum violations span TWO collection members has
        // no single member to word the sentence around, so the validator's envelope
        // stands — under EMR's own code, which the operation declares.
        own_envelope(
            "undeclared entries in two InstanceStates and InstanceGroupTypes at once",
            "ListInstances",
            json!({
                "ClusterId": cluster,
                "InstanceStates": ["NOPE"],
                "InstanceGroupTypes": ["PRIMARY"],
            }),
            "instanceStates",
        ),
        // ---- the generic validator's code, EMR's own prose ---------------------
        // The recorded case: five structure steps down PutAutoScalingPolicy's policy,
        // on the CloudWatch metric alarm EMR validates itself.
        generic_sentence(
            "an undeclared ComparisonOperator in the only scaling rule",
            "PutAutoScalingPolicy",
            put_policy(
                cluster,
                group,
                json!([scaling_rule(
                    "scale-out-bad-comparison-operator",
                    "NOT_A_REAL_COMPARISON_OPERATOR",
                    "AVERAGE",
                    "CHANGE_IN_CAPACITY",
                )]),
            ),
            "The supplied cloud watch comparison operator is invalid.",
        ),
        // The same depth reached through the SECOND rule, so the refusal is not keyed
        // to one position.
        generic_sentence(
            "an undeclared ComparisonOperator in the second scaling rule",
            "PutAutoScalingPolicy",
            put_policy(
                cluster,
                group,
                json!([
                    good_rule("scale-out"),
                    scaling_rule("scale-in", "NEARLY", "AVERAGE", "CHANGE_IN_CAPACITY"),
                ]),
            ),
            "The supplied cloud watch comparison operator is invalid.",
        ),
        // The alarm's other two enum members: the sentence names the member EMR
        // rejected, so it is built from the member rather than written for one of
        // them.
        generic_sentence(
            "an undeclared Statistic in a scaling rule",
            "PutAutoScalingPolicy",
            put_policy(
                cluster,
                group,
                json!([scaling_rule(
                    "scale-out",
                    COMPARISON_OPERATORS[0],
                    "MEDIAN",
                    "CHANGE_IN_CAPACITY",
                )]),
            ),
            "The supplied cloud watch statistic is invalid.",
        ),
        generic_sentence(
            "an undeclared Unit in a scaling rule",
            "PutAutoScalingPolicy",
            put_policy(
                cluster,
                group,
                json!([{
                    "Name": "scale-out",
                    "Action": {
                        "SimpleScalingPolicyConfiguration": {
                            "AdjustmentType": "CHANGE_IN_CAPACITY",
                            "ScalingAdjustment": 1,
                        },
                    },
                    "Trigger": {
                        "CloudWatchAlarmDefinition": {
                            "ComparisonOperator": COMPARISON_OPERATORS[1],
                            "MetricName": "YARNMemoryAvailablePercentage",
                            "Period": 300,
                            "Threshold": 15.0,
                            "Unit": "FURLONGS",
                        },
                    },
                }]),
            ),
            "The supplied cloud watch unit is invalid.",
        ),
        // ---- the generic validator, on an enum inside any other nested structure
        // The same depth on the OTHER branch of a scaling rule, which is no CloudWatch
        // alarm: nothing words it but the validator's envelope.
        generic(
            "an undeclared AdjustmentType in a scaling rule",
            "PutAutoScalingPolicy",
            put_policy(
                cluster,
                group,
                json!([scaling_rule(
                    "scale-out",
                    COMPARISON_OPERATORS[3],
                    "AVERAGE",
                    "CHANGE_IN_HEADCOUNT",
                )]),
            ),
            "autoScalingPolicy.rules.1.member.action.simpleScalingPolicyConfiguration.\
             adjustmentType",
        ),
        // Two structure steps down, through a list.
        generic(
            "an undeclared ActionOnFailure on a submitted step",
            "AddJobFlowSteps",
            json!({
                "JobFlowId": cluster,
                "Steps": [{
                    "Name": "a step",
                    "ActionOnFailure": "GIVE_UP",
                    "HadoopJarStep": { "Jar": "command-runner.jar" },
                }],
            }),
            "steps.1.member.actionOnFailure",
        ),
        // Two structure steps down, through a structure.
        generic(
            "an undeclared InstanceFleetType on a submitted fleet",
            "AddInstanceFleet",
            json!({
                "ClusterId": cluster,
                "InstanceFleet": {
                    "InstanceFleetType": "WORKER",
                    "TargetOnDemandCapacity": 1,
                    "InstanceTypeConfigs": [{ "InstanceType": "m5.xlarge" }],
                },
            }),
            "instanceFleet.instanceFleetType",
        ),
        // Three structure steps down.
        generic(
            "an undeclared Market on a launched instance group",
            "RunJobFlow",
            json!({
                "Name": "an undeclared market",
                "ReleaseLabel": "emr-6.15.0",
                "Instances": {
                    "KeepJobFlowAliveWhenNoSteps": true,
                    "InstanceGroups": [{
                        "Name": "primary",
                        "InstanceRole": "MASTER",
                        "InstanceType": "m5.xlarge",
                        "InstanceCount": 1,
                        "Market": "CHEAP",
                    }],
                },
            }),
            "instances.instanceGroups.1.member.market",
        ),
    ]
}

/// THE RULE, on everything a caller gets wrong: the code and the message of an
/// out-of-enum refusal are the ones belonging to the layer that reaches the offending
/// value.
#[tokio::test]
async fn an_out_of_enum_value_is_refused_by_the_layer_that_reaches_it() {
    let emr = Emr::new();
    let cluster = emr.run_job_flow(kept_alive_request("filtered")).await;
    for case in refused_cases(&cluster) {
        let label = &case.case;
        let (status, payload) = emr.call(case.operation, case.body.clone()).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400, got {status} with {payload}"
        );
        let code = at(&payload, &["__type"]).as_str().unwrap_or_default().to_string();
        assert!(
            code.contains(case.code),
            "case {label:?}: expected error code {}, got {code:?} in {payload}",
            case.code
        );
        let message = error_message(&payload);
        match &case.says {
            Says::Exactly(sentence) => assert_eq!(
                &message, sentence,
                "case {label:?}: expected {sentence:?}, got {message:?}"
            ),
            Says::Names(path) => assert!(
                message.contains(path),
                "case {label:?}: the refusal must name the member {path:?}, got {message:?}"
            ),
        }
    }
}

/// THE RULE's other half: a value the model declares is never refused at the
/// boundary, whatever depth it sits at — it reaches the handler, which answers for
/// itself. Naming the status per row is what keeps a re-scaffolded operation from
/// passing here silently: `ListInstances` and `ListSessions` are implemented, so the
/// declared values reach their handlers, which then refuse the CLUSTER this suite names
/// and holds none of.
#[tokio::test]
async fn a_declared_enum_value_reaches_the_handler_at_every_depth() {
    let emr = Emr::new();
    let cluster = "j-ABCDEFGHIJKLM";
    let rows: Vec<(&str, &'static str, Value, u16)> = vec![
        (
            "one declared ClusterState",
            "ListClusters",
            json!({ "ClusterStates": ["RUNNING"] }),
            200,
        ),
        (
            "every declared ClusterState at once",
            "ListClusters",
            json!({ "ClusterStates": CLUSTER_STATES }),
            200,
        ),
        ("no filter at all", "ListClusters", json!({}), 200),
        (
            "a declared NotebookExecutionStatus",
            "ListNotebookExecutions",
            json!({ "Status": "FINISHED" }),
            200,
        ),
        (
            "a declared IdentityType",
            "ListStudioSessionMappings",
            json!({ "IdentityType": "USER" }),
            200,
        ),
        (
            "every declared InstanceState at once",
            "ListInstances",
            json!({ "ClusterId": cluster, "InstanceStates": INSTANCE_STATES }),
            400,
        ),
        (
            // `ListSessions` is implemented: the declared values reach the handler, which
            // then refuses the CLUSTER this suite names and holds none of.
            "declared SessionStates",
            "ListSessions",
            json!({ "ClusterId": cluster, "SessionStates": ["IDLE", "BUSY"] }),
            400,
        ),
    ];
    for (label, operation, body, expected) in rows {
        let (status, payload) = emr.call(operation, body).await;
        assert_eq!(
            status, expected,
            "case {label:?}: a value the model declares must reach the handler and be \
             answered {expected}, got {status} with {payload}"
        );
    }
}

/// THE RULE at the far end of the nesting, proved end to end: each declared
/// `ComparisonOperator` passes the boundary, is PERSISTED on the instance group the
/// request names, and reads back through `ListInstanceGroups` — so the narrowed
/// refusal path did not start refusing what the model admits.
#[tokio::test]
async fn every_declared_comparison_operator_is_admitted_and_persisted() {
    let emr = Emr::new();
    let cluster = emr
        .run_job_flow(json!({
            "Name": "nested enum round trip",
            "ReleaseLabel": "emr-6.15.0",
            "JobFlowRole": crate::job_flow_lifecycle::INSTANCE_PROFILE, "ServiceRole": crate::job_flow_lifecycle::SERVICE_ROLE,
            "Instances": {
                "KeepJobFlowAliveWhenNoSteps": true,
                "InstanceGroups": [
                    { "Name": "primary", "InstanceRole": "MASTER",
                      "InstanceType": "m5.xlarge", "InstanceCount": 1 },
                    { "Name": "core", "InstanceRole": "CORE",
                      "InstanceType": "m5.xlarge", "InstanceCount": 2 },
                ],
            },
        }))
        .await;
    let groups = emr
        .ok("ListInstanceGroups", json!({ "ClusterId": &cluster }))
        .await;
    let core = groups["InstanceGroups"]
        .as_array()
        .expect("a group listing")
        .iter()
        .find(|group| group["InstanceGroupType"] == json!("CORE"))
        .map(|group| group["Id"].as_str().expect("a group id").to_string())
        .expect("the CORE instance group");

    for comparison in COMPARISON_OPERATORS {
        let label = format!("ComparisonOperator {comparison}");
        let (status, payload) = emr
            .call(
                "PutAutoScalingPolicy",
                put_policy(
                    &cluster,
                    &core,
                    json!([scaling_rule(
                        "scale-out",
                        comparison,
                        "AVERAGE",
                        "CHANGE_IN_CAPACITY"
                    )]),
                ),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: a declared ComparisonOperator must be accepted, got {status} \
             with {payload}"
        );

        let groups = emr
            .ok("ListInstanceGroups", json!({ "ClusterId": &cluster }))
            .await;
        let stored = groups["InstanceGroups"]
            .as_array()
            .expect("a group listing")
            .iter()
            .find(|group| group["Id"] == json!(&core))
            .cloned()
            .expect("the CORE instance group");
        let read_back = at(
            &stored,
            &[
                "AutoScalingPolicy",
                "Rules",
            ],
        )
        .as_array()
        .and_then(|rules| rules.first().cloned())
        .unwrap_or(Value::Null);
        assert_eq!(
            at(
                &read_back,
                &["Trigger", "CloudWatchAlarmDefinition", "ComparisonOperator"]
            ),
            &json!(comparison),
            "case {label:?}: the persisted policy must carry the submitted operator, got \
             {stored}"
        );
    }
}
