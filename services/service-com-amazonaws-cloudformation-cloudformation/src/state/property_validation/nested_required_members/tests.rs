// SPDX-License-Identifier: CC0-1.0

//! The suites for the required-member question of a nested property structure
//! ([`super`]): what the stage reports for a structure missing one, and the bounds its walk
//! and its reporting are held to.

use super::*;
use ::serde_json::{json, Value};
use ::std::collections::BTreeMap;

/// One declared resource of `resource_type` carrying `properties`.
fn declared(logical_id: &str, resource_type: &str, properties: Value) -> StoredResource {
    StoredResource {
        logical_id: logical_id.to_string(),
        resource_type: resource_type.to_string(),
        physical_resource_id: format!("{logical_id}-physical"),
        attributes: BTreeMap::new(),
        metadata: None,
        properties,
        materialised: false,
        deletion_policy: None,
        update_replace_policy: None,
    }
}

/// The types the rows below are about.
const EVENTS_RULE: &str = crate::state::template::EVENTS_RULE;
const SCHEDULE: &str = crate::state::template::SCHEDULER_SCHEDULE;

/// One target carrying every member `Target` requires, which the rows build their nested
/// structures inside.
fn target(extra: Value) -> Value {
    let mut target = json!({"Id": "t1", "Arn": "arn:aws:sqs:us-east-1:000000000000:q"});
    if let (Some(target), Some(extra)) = (target.as_object_mut(), extra.as_object()) {
        for (member, value) in extra {
            target.insert(member.clone(), value.clone());
        }
    }
    target
}

/// One row of the table: the case, the type declared, its property document, and the
/// `(member, pointer under the property document)` pairs the stage must report, in order —
/// empty where every structure the declaration states carries what it requires.
type NestedRow<'a> = (&'a str, &'a str, Value, &'a [(&'a str, &'a str)]);

/// RULE: a structure a declaration states inside one of its properties is held to the
/// members that structure's contract marks required, and one that does not carry such a
/// member fails the validation with `Required property [<member>] not found` at the
/// pointer of the structure that lacks it — one error per missing member.
///
/// The rows span what the rule generalises over: how deep the structure sits (a property's
/// own structure, a structure inside an element of a list, two and three levels down),
/// which element of the list it sits in, how many elements the list carries, how many
/// members of a structure requiring several are missing, the resource type (two of them,
/// including rows the report that led here did not cite), and the forms a value can take
/// where no structure is stated at all.
#[test]
fn a_nested_structure_missing_a_required_member_fails_the_validation() {
    let rows: &[NestedRow] = &[
        (
            "a target declaring ECS parameters without the task definition",
            EVENTS_RULE,
            json!({"Targets": [target(json!({"EcsParameters": {"TaskCount": 1}}))]}),
            &[("TaskDefinitionArn", "Targets/0/EcsParameters")],
        ),
        (
            "the SECOND of three targets is the incomplete one",
            EVENTS_RULE,
            json!({"Targets": [
                target(json!({"EcsParameters": {"TaskDefinitionArn": "arn:task"}})),
                target(json!({"EcsParameters": {}})),
                target(json!({})),
            ]}),
            &[("TaskDefinitionArn", "Targets/1/EcsParameters")],
        ),
        (
            "a target carrying the task definition, and nothing else missing",
            EVENTS_RULE,
            json!({"Targets": [target(json!({"EcsParameters":
                {"TaskDefinitionArn": "arn:aws:ecs:us-east-1:000000000000:task-definition/t:1",
                 "LaunchType": "FARGATE"}}))]}),
            &[],
        ),
        (
            "an element of the target list missing the id the rule addresses it by",
            EVENTS_RULE,
            json!({"Targets": [{"Arn": "arn:aws:sqs:us-east-1:000000000000:q"}]}),
            &[("Id", "Targets/0")],
        ),
        (
            // Both of the element's own required members are missing, so the element is
            // two errors, in the order the contract publishes the members.
            "an element of the target list stating neither member",
            EVENTS_RULE,
            json!({"Targets": [{"Input": "{}"}]}),
            &[("Arn", "Targets/0"), ("Id", "Targets/0")],
        ),
        (
            // The structure's own members are judged before the structures inside it,
            // and each omission is its own error at its own pointer.
            "an element missing its ARN and its nested task definition",
            EVENTS_RULE,
            json!({"Targets": [{"Id": "t1", "EcsParameters": {}}]}),
            &[
                ("Arn", "Targets/0"),
                ("TaskDefinitionArn", "Targets/0/EcsParameters"),
            ],
        ),
        (
            "an awsvpc configuration three levels down without its subnets",
            EVENTS_RULE,
            json!({"Targets": [target(json!({"EcsParameters": {
                "TaskDefinitionArn": "arn:task",
                "NetworkConfiguration": {"AwsVpcConfiguration": {
                    "SecurityGroups": ["sg-1"]}}}}))]}),
            &[(
                "Subnets",
                "Targets/0/EcsParameters/NetworkConfiguration/AwsVpcConfiguration",
            )],
        ),
        (
            "an awsvpc configuration carrying its subnets",
            EVENTS_RULE,
            json!({"Targets": [target(json!({"EcsParameters": {
                "TaskDefinitionArn": "arn:task",
                "NetworkConfiguration": {"AwsVpcConfiguration": {
                    "Subnets": ["subnet-1"]}}}}))]}),
            &[],
        ),
        (
            "an input transformer without the template it produces",
            EVENTS_RULE,
            json!({"Targets": [target(json!({"InputTransformer":
                {"InputPathsMap": {"instance": "$.detail.instance"}}}))]}),
            &[("InputTemplate", "Targets/0/InputTransformer")],
        ),
        (
            "a batch target missing the name it submits the job under",
            EVENTS_RULE,
            json!({"Targets": [target(json!({"BatchParameters":
                {"JobDefinition": "arn:job"}}))]}),
            &[("JobName", "Targets/0/BatchParameters")],
        ),
        (
            // Both members of one nested structure missing: two errors at the same
            // pointer, in the order the contract publishes them.
            "a batch target stating neither the job definition nor the job name",
            EVENTS_RULE,
            json!({"Targets": [target(json!({"BatchParameters": {}}))]}),
            &[
                ("JobDefinition", "Targets/0/BatchParameters"),
                ("JobName", "Targets/0/BatchParameters"),
            ],
        ),
        (
            "a Redshift target without the database it runs against",
            EVENTS_RULE,
            json!({"Targets": [target(json!({"RedshiftDataParameters":
                {"Sql": "select 1"}}))]}),
            &[("Database", "Targets/0/RedshiftDataParameters")],
        ),
        (
            "a Kinesis target without the partition key path it extracts",
            EVENTS_RULE,
            json!({"Targets": [target(json!({"KinesisParameters": {}}))]}),
            &[("PartitionKeyPath", "Targets/0/KinesisParameters")],
        ),
        (
            "run-command parameters stating no criteria at all",
            EVENTS_RULE,
            json!({"Targets": [target(json!({"RunCommandParameters": {}}))]}),
            &[("RunCommandTargets", "Targets/0/RunCommandParameters")],
        ),
        (
            "one criterion of a run-command target without its values",
            EVENTS_RULE,
            json!({"Targets": [target(json!({"RunCommandParameters": {"RunCommandTargets": [
                {"Key": "InstanceIds", "Values": ["i-1"]},
                {"Key": "tag:Stage"},
            ]}}))]}),
            &[(
                "Values",
                "Targets/0/RunCommandParameters/RunCommandTargets/1",
            )],
        ),
        (
            "a parameter structure the declaration states no member of this stage judges",
            EVENTS_RULE,
            json!({"Targets": [target(json!({"SqsParameters": {"MessageGroupId": "g"}}))]}),
            &[],
        ),
        (
            "an AppSync target without the GraphQL operation to execute",
            EVENTS_RULE,
            json!({"Targets": [target(json!({"AppSyncParameters": {}}))]}),
            &[("GraphQLOperation", "Targets/0/AppSyncParameters")],
        ),
        (
            "the second capacity-provider strategy entry naming no provider",
            EVENTS_RULE,
            json!({"Targets": [target(json!({"EcsParameters": {
                "TaskDefinitionArn": "arn:task",
                "CapacityProviderStrategy": [
                    {"CapacityProvider": "FARGATE", "Weight": 1},
                    {"Weight": 2}]}}))]}),
            &[(
                "CapacityProvider",
                "Targets/0/EcsParameters/CapacityProviderStrategy/1",
            )],
        ),
        (
            "a task tag stating neither its key nor its value",
            EVENTS_RULE,
            json!({"Targets": [target(json!({"EcsParameters": {
                "TaskDefinitionArn": "arn:task",
                "TagList": [{}]}}))]}),
            &[
                ("Key", "Targets/0/EcsParameters/TagList/0"),
                ("Value", "Targets/0/EcsParameters/TagList/0"),
            ],
        ),
        (
            "a pipeline parameter without the value to start the execution with",
            EVENTS_RULE,
            json!({"Targets": [target(json!({"SageMakerPipelineParameters":
                {"PipelineParameterList": [{"Name": "Stage"}]}}))]}),
            &[(
                "Value",
                "Targets/0/SageMakerPipelineParameters/PipelineParameterList/0",
            )],
        ),
        (
            "a rule stating no targets at all",
            EVENTS_RULE,
            json!({"EventPattern": {"source": ["aws.ec2"]}}),
            &[],
        ),
        (
            "a rule whose target list is empty",
            EVENTS_RULE,
            json!({"Targets": []}),
            &[],
        ),
        (
            // A list-valued property stating no list states no element of it: what the
            // elements' contract requires is not asked of the value in their place.
            "a target list the declaration states as an unevaluated expression",
            EVENTS_RULE,
            json!({"Targets": {"Fn::ImportValue": "Targets"}}),
            &[],
        ),
        (
            "a target list the declaration states as a single structure",
            EVENTS_RULE,
            json!({"Targets": {"Id": "t1"}}),
            &[],
        ),
        (
            // ...and the other way round: a property stating ONE structure, declared as a
            // list, states no structure the row is about either.
            "a schedule whose single target property is declared as a list",
            SCHEDULE,
            json!({"ScheduleExpression": "rate(1 hour)",
                   "FlexibleTimeWindow": {"Mode": "OFF"},
                   "Target": [{"Arn": "arn:aws:sqs:us-east-1:000000000000:q"}]}),
            &[],
        ),
        (
            "a target list whose value has not landed yet",
            EVENTS_RULE,
            json!({"Targets": Value::Null}),
            &[],
        ),
        (
            "a target whose nested parameters are an unevaluated expression",
            EVENTS_RULE,
            json!({"Targets": [target(json!({"EcsParameters": {"Ref": "Params"}}))]}),
            &[],
        ),
        (
            // A member whose value has not landed yet is a member the declaration
            // STATES, so it is not reported as missing.
            "a target whose ARN reads a value that has not landed yet",
            EVENTS_RULE,
            json!({"Targets": [{"Id": "t1", "Arn": Value::Null}]}),
            &[],
        ),
        (
            // A type of another service, whose single-structure property reports no
            // index at all.
            "a schedule whose target states no role to invoke it as",
            SCHEDULE,
            json!({"ScheduleExpression": "rate(1 hour)",
                   "FlexibleTimeWindow": {"Mode": "OFF"},
                   "Target": {"Arn": "arn:aws:sqs:us-east-1:000000000000:q"}}),
            &[("RoleArn", "Target")],
        ),
        (
            "a schedule whose flexible time window states no mode",
            SCHEDULE,
            json!({"ScheduleExpression": "rate(1 hour)",
                   "FlexibleTimeWindow": {"MaximumWindowInMinutes": 15},
                   "Target": {"Arn": "arn:aws:sqs:us-east-1:000000000000:q",
                              "RoleArn": "arn:aws:iam::000000000000:role/r"}}),
            &[("Mode", "FlexibleTimeWindow")],
        ),
        (
            "a schedule whose ECS target states no task definition",
            SCHEDULE,
            json!({"ScheduleExpression": "rate(1 hour)",
                   "FlexibleTimeWindow": {"Mode": "OFF"},
                   "Target": {"Arn": "arn:aws:ecs:us-east-1:000000000000:cluster/c",
                              "RoleArn": "arn:aws:iam::000000000000:role/r",
                              "EcsParameters": {"TaskCount": 2}}}),
            &[("TaskDefinitionArn", "Target/EcsParameters")],
        ),
        (
            "a schedule whose EventBridge target states neither member it requires",
            SCHEDULE,
            json!({"ScheduleExpression": "rate(1 hour)",
                   "FlexibleTimeWindow": {"Mode": "OFF"},
                   "Target": {"Arn": "arn:aws:events:us-east-1:000000000000:event-bus/b",
                              "RoleArn": "arn:aws:iam::000000000000:role/r",
                              "EventBridgeParameters": {}}}),
            &[
                ("DetailType", "Target/EventBridgeParameters"),
                ("Source", "Target/EventBridgeParameters"),
            ],
        ),
        (
            "a schedule whose Kinesis target states no partition key",
            SCHEDULE,
            json!({"ScheduleExpression": "rate(1 hour)",
                   "FlexibleTimeWindow": {"Mode": "OFF"},
                   "Target": {"Arn": "arn:aws:kinesis:us-east-1:000000000000:stream/s",
                              "RoleArn": "arn:aws:iam::000000000000:role/r",
                              "KinesisParameters": {}}}),
            &[("PartitionKey", "Target/KinesisParameters")],
        ),
        (
            "a schedule carrying every member its structures require",
            SCHEDULE,
            json!({"ScheduleExpression": "rate(1 hour)",
                   "FlexibleTimeWindow": {"Mode": "FLEXIBLE",
                                          "MaximumWindowInMinutes": 15},
                   "Target": {"Arn": "arn:aws:ecs:us-east-1:000000000000:cluster/c",
                              "RoleArn": "arn:aws:iam::000000000000:role/r",
                              "EcsParameters": {"TaskDefinitionArn": "arn:task"}}}),
            &[],
        ),
        (
            "a type this stage knows no nested structure of",
            "AWS::SQS::Queue",
            json!({"RedrivePolicy": {"maxReceiveCount": 5}}),
            &[],
        ),
    ];
    for (case, resource_type, properties, reported) in rows {
        let resources = vec![declared("R", resource_type, properties.clone())];
        let failed = super::super::failures(&resources);
        let expected: Vec<(String, String)> = reported
            .iter()
            .map(|(member, at)| {
                (
                    super::super::missing_property_reason(member),
                    format!("/Resources/R/Properties/{at}"),
                )
            })
            .collect();
        let found: Vec<(String, String)> = failed
            .iter()
            .map(|failure| (failure.reason.clone(), failure.path.clone()))
            .collect();
        assert_eq!(
            found, expected,
            "case {case:?}: expected the failures {expected:?}, got {found:?}"
        );
    }
}

/// RULE: how many errors one operation is refused with is bounded by the stage's own bound
/// ([`super::super::MAX_REPORTED_FAILURES`]) and not by how many structures the declaration
/// states, so a template cannot decide how many validation results one request records.
///
/// The bound is a LIMIT of the stage and is pinned so it is visible rather than only
/// commented: the declaration is still refused, and the errors kept are the first ones.
#[test]
fn the_errors_one_declaration_is_refused_with_are_bounded() {
    let bound = super::super::MAX_REPORTED_FAILURES;
    let targets: Vec<Value> = (0..bound * 2).map(|_| json!({"Input": "{}"})).collect();
    let resources = vec![declared("R", EVENTS_RULE, json!({"Targets": targets}))];
    let failed = super::super::failures(&resources);
    assert_eq!(
        failed.len(),
        bound,
        "case \"every target missing both of its required members\": expected {bound} \
             errors, got {}",
        failed.len()
    );
}

/// RULE: what one walk of a member chain may VISIT is bounded by the stage's own budget
/// ([`super::super::MAX_WALKED_VALUES`]) and not by what the declaration states, so a
/// document whose lists nest at several steps of a chain — the product of their lengths,
/// with no value at the end of most of them to count — cannot decide how much work one
/// request buys.
///
/// The bound is a LIMIT of the stage and is pinned so it is visible rather than only
/// commented: a structure the walk reaches inside the budget fails the validation, the same
/// structure behind a list that spends it does not, and the service that owns the resource
/// is what judges it then. The `RunCommandParameters` of the first target is a LIST, which
/// the row that judges that structure passes over, so what the elements cost is visits and
/// nothing else.
#[test]
fn the_values_one_walk_visits_are_bounded() {
    let rows: &[(&str, usize, bool)] = &[
        ("the incomplete criterion is inside the budget", 2, true),
        (
            "the incomplete criterion is behind a list that spends it",
            super::super::MAX_WALKED_VALUES,
            false,
        ),
    ];
    for (case, passed_over, refused) in rows {
        let filler: Vec<Value> = (0..*passed_over).map(|_| json!({})).collect();
        let properties = json!({"Targets": [
            {"Id": "spender", "Arn": "arn:aws:sqs:us-east-1:000000000000:q",
             "RunCommandParameters": filler},
            {"Id": "judged", "Arn": "arn:aws:ssm:us-east-1:000000000000:document/d",
             "RunCommandParameters": {"RunCommandTargets": [{}]}},
        ]});
        let resources = vec![declared("R", EVENTS_RULE, properties)];
        let failed = super::super::failures(&resources);
        assert_eq!(
            !failed.is_empty(),
            *refused,
            "case {case:?}: expected the stage to report a failure: {refused}, got {failed:?}"
        );
    }
}

/// RULE: every row is about a structure NESTED inside a property — a chain of at least one
/// member, carrying at least one required member of it — and names those members in the
/// order its contract publishes them, which is the order a structure missing several of
/// them is reported in. One structure is named by at most one row, so no two rows can hold
/// the same structure to two different sets.
#[test]
fn every_row_names_one_nested_structure_in_its_published_order() {
    let mut named: Vec<(&str, &[&str])> = Vec::new();
    for row in NESTED_REQUIRED_MEMBERS {
        let case = (row.resource_type, row.structure);
        assert!(
            !row.structure.is_empty() && !row.required.is_empty(),
            "case {case:?}: expected a nested structure with required members, got {row:?}"
        );
        let mut published = row.required.to_vec();
        published.sort_unstable();
        published.dedup();
        assert_eq!(
            published.as_slice(),
            row.required,
            "case {case:?}: expected the required members in published order with no \
                 repetition, got {:?}",
            row.required
        );
        assert!(
            !named.contains(&case),
            "case {case:?}: expected one row per nested structure, it is named twice"
        );
        named.push(case);
    }
}
