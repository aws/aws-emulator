// SPDX-License-Identifier: CC0-1.0

//! The required-property question asked of a structure NESTED inside a declaration's
//! property, rather than of the property document itself.
//!
//! A resource type's schema states its property types as structures of their own, and a
//! member one of those structures marks required is required of every structure of that kind
//! the declaration states — wherever it sits: inside a property, inside an element of a
//! list-valued property, inside a structure inside one of those. So a declaration that STATES
//! such a structure without one of its required members is one no handler of the type could
//! ever be given, exactly as a document missing a required property of the resource is
//! ([`super::missing_required_properties`] asks that question of the document): the stage that
//! runs before provisioning reports it and the operation deploys nothing.
//!
//! Every such omission is ONE error, as a schema validation reports one error per violation it
//! found: a structure missing two required members fails the operation twice over, and the
//! summary the stack carries counts them ([`super::validation_failure_summary`]).
//!
//! What it reports is the SAME sentence that question reports
//! ([`super::missing_property_reason`]) — `Required property [<member>] not found`, whose one
//! variable part is the member the contract names — and it reports it at the JSON pointer of
//! the structure that LACKS the member, which is the element of the submitted template the
//! validation was applied to ([`super::member_path`] spells that pointer, with the index of an
//! element where the structure sits in a list). A failure of the document's own required
//! property names `/Resources/<LogicalId>/Properties` for the same reason: the path is the
//! structure judged, not the member that is missing from it.
//!
//! The catalogue is the same kind as the rest of that stage's and is read the same way: one
//! row per (resource type, nested structure), carrying the members that structure's contract
//! marks required, so a structure no row covers is left to the service that owns the resource
//! rather than guessed at. A structure the declaration does not state at all is not judged —
//! `required` is a condition ON a structure, and a property a declaration omits states none —
//! which is why a property that is itself required is judged by the question above this one,
//! and why a value that is no object here (an expression this service left unevaluated, a
//! value the author wrote where the schema states a structure) is passed over: what it fails
//! is a constraint another question of this stage judges.

use super::{PropertyFailure, StoredResource};

/// One nested property structure's REQUIRED members: the ones a structure of that kind cannot
/// be deployed without.
#[derive(Debug, Clone, Copy)]
pub struct NestedRequiredMembers {
    /// The resource type the structure belongs to, as a template declares it.
    pub resource_type: &'static str,
    /// The chain of members leading from the resource's property document down to the
    /// structure judged, each spelled as a template spells it: one name for a property of the
    /// resource itself, and the members stepped through for a structure nested deeper. A step
    /// that passes THROUGH a list is taken over every element of it, so one row judges every
    /// structure of its kind the declaration states, each at its own pointer.
    pub structure: &'static [&'static str],
    /// Whether the structure judged is one ELEMENT of a list the last member of that chain
    /// states, rather than that member's own value.
    ///
    /// What a LIST-valued property publishes is its ELEMENTS' contract, so a declaration
    /// stating no list there states no structure such a row is about at all — an expression
    /// this service left unevaluated, or a value the author wrote where the schema states a
    /// list — and the row judges nothing rather than holding that value to the elements'
    /// members; the structure it is not is a constraint another question of this stage judges
    /// ([`super::structured_properties`]).
    pub list: bool,
    /// The members that structure's contract marks required, in the order the contract
    /// publishes them, which is the order a structure missing SEVERAL of them is reported in.
    pub required: &'static [&'static str],
}

/// The nested structures this stage knows the REQUIRED members of.
///
/// Source for each row: the property type's entry in the "AWS resource and property types
/// reference" of the CloudFormation User Guide — every member it publishes with "Required:
/// Yes", in the order that reference lists them (alphabetically) — which are the `@required`
/// members of the structure the owning API carries that property type onto, named here as the
/// TEMPLATE spells them (the two names differ where the property type renames a member, and
/// the provider that maps one is where that correspondence lives,
/// [`crate::state::provisioner::network_and_delivery::ecs_parameters`]).
///
/// What a declaration is held to is the TYPE'S schema, which is not always the owning API's
/// own `required`: a member that reference publishes "Required: Yes" is required of a template
/// however the API models it, which is the same reading the value sets of this stage are in
/// ([`super::enum_properties`] says so for a published `enum` the API has since widened).
pub const NESTED_REQUIRED_MEMBERS: &[NestedRequiredMembers] = &[
    // An `AWS::Events::Rule`'s `Targets` are the resources the rule invokes, each a `Target`
    // (`smithy_query(shape="Target")` of the `eventbridge` model): a target IS the ARN it
    // invokes, under the `Id` the rule addresses it by, so both members are required of every
    // element of that list — and a rule's targets are installed by the `PutTargets` the
    // provider makes beside `PutRule`, which could carry neither.
    NestedRequiredMembers {
        resource_type: crate::state::template::EVENTS_RULE,
        structure: &["Targets"],
        list: true,
        required: &["Arn", "Id"],
    },
    // `Target$AppSyncParameters` carry "the GraphQL operation to be parsed and executed, if
    // the event target is an AppSync API", and the type publishes `GraphQLOperation` with
    // "Required: Yes" — the operation IS the parameters, so a declaration stating them without
    // it names nothing for AppSync to execute. (EventBridge's own
    // `AppSyncParameters$GraphQLOperation` is optional on the wire; what a TEMPLATE is held to
    // is the type's schema.)
    NestedRequiredMembers {
        resource_type: crate::state::template::EVENTS_RULE,
        structure: &["Targets", "AppSyncParameters"],
        list: false,
        required: &["GraphQLOperation"],
    },
    // `Target$BatchParameters` are "the custom parameters to be used when the target is an
    // Batch job", and `BatchParameters` declares the job definition to submit and the name to
    // submit it under required.
    NestedRequiredMembers {
        resource_type: crate::state::template::EVENTS_RULE,
        structure: &["Targets", "BatchParameters"],
        list: false,
        required: &["JobDefinition", "JobName"],
    },
    // `Target$EcsParameters` are "the custom parameters to be used when the target is an
    // Amazon ECS task", and `EcsParameters` declares `TaskDefinitionArn` required: the task
    // definition is WHAT the target runs, so a declaration stating ECS parameters without it
    // names no task EventBridge could ever run.
    NestedRequiredMembers {
        resource_type: crate::state::template::EVENTS_RULE,
        structure: &["Targets", "EcsParameters"],
        list: false,
        required: &["TaskDefinitionArn"],
    },
    // ...the capacity providers those parameters may run the task on, each a
    // `CapacityProviderStrategyItem` whose `CapacityProvider` ("the short name of the capacity
    // provider") is required: a strategy entry naming no provider selects nothing.
    NestedRequiredMembers {
        resource_type: crate::state::template::EVENTS_RULE,
        structure: &["Targets", "EcsParameters", "CapacityProviderStrategy"],
        list: true,
        required: &["CapacityProvider"],
    },
    // ...the tags they apply to the task, which the property type publishes as `TagList`: a
    // `Tag` IS a key and the value assigned under it, and both are required.
    NestedRequiredMembers {
        resource_type: crate::state::template::EVENTS_RULE,
        structure: &["Targets", "EcsParameters", "TagList"],
        list: true,
        required: &["Key", "Value"],
    },
    // ...and the awsvpc configuration inside those parameters, which the property type
    // publishes as `NetworkConfiguration.AwsVpcConfiguration`: `AwsVpcConfiguration` declares
    // `Subnets` required ("specifies the subnets associated with the task"), a task having no
    // network without them.
    NestedRequiredMembers {
        resource_type: crate::state::template::EVENTS_RULE,
        structure: &[
            "Targets",
            "EcsParameters",
            "NetworkConfiguration",
            "AwsVpcConfiguration",
        ],
        list: false,
        required: &["Subnets"],
    },
    // `Target$InputTransformer` customises what the target is sent, and `InputTransformer`
    // declares `InputTemplate` required — the template IS the input it produces, the paths map
    // beside it only naming what may be substituted into it.
    NestedRequiredMembers {
        resource_type: crate::state::template::EVENTS_RULE,
        structure: &["Targets", "InputTransformer"],
        list: false,
        required: &["InputTemplate"],
    },
    // `Target$KinesisParameters` control the shard an event goes to, and `KinesisParameters`
    // declares `PartitionKeyPath` required: the path IS the partition key it extracts.
    NestedRequiredMembers {
        resource_type: crate::state::template::EVENTS_RULE,
        structure: &["Targets", "KinesisParameters"],
        list: false,
        required: &["PartitionKeyPath"],
    },
    // `Target$RedshiftDataParameters` run a statement on a cluster, and
    // `RedshiftDataParameters` declares `Database` required — "the name of the database" the
    // statement is run against.
    NestedRequiredMembers {
        resource_type: crate::state::template::EVENTS_RULE,
        structure: &["Targets", "RedshiftDataParameters"],
        list: false,
        required: &["Database"],
    },
    // `Target$RunCommandParameters` state which instances the command is sent to, and
    // `RunCommandParameters` declares `RunCommandTargets` required: the criteria ARE the
    // parameters.
    NestedRequiredMembers {
        resource_type: crate::state::template::EVENTS_RULE,
        structure: &["Targets", "RunCommandParameters"],
        list: false,
        required: &["RunCommandTargets"],
    },
    // ...and each of those criteria is a `RunCommandTarget`, which declares both its `Key`
    // ("can be either tag:<tag-key> or InstanceIds") and the `Values` that key selects by
    // required.
    NestedRequiredMembers {
        resource_type: crate::state::template::EVENTS_RULE,
        structure: &["Targets", "RunCommandParameters", "RunCommandTargets"],
        list: true,
        required: &["Key", "Values"],
    },
    // `Target$SageMakerPipelineParameters` start a pipeline execution, and each entry of its
    // `PipelineParameterList` is a `SageMakerPipelineParameter`: the name of the parameter and
    // the value to start the execution with, both required.
    NestedRequiredMembers {
        resource_type: crate::state::template::EVENTS_RULE,
        structure: &[
            "Targets",
            "SageMakerPipelineParameters",
            "PipelineParameterList",
        ],
        list: true,
        required: &["Name", "Value"],
    },
    // An `AWS::Scheduler::Schedule`'s `FlexibleTimeWindow` "allows you to configure a time
    // window during which EventBridge Scheduler invokes the schedule", and
    // `FlexibleTimeWindow` declares `Mode` required: the mode is what says whether there is a
    // window at all.
    NestedRequiredMembers {
        resource_type: crate::state::template::SCHEDULER_SCHEDULE,
        structure: &["FlexibleTimeWindow"],
        list: false,
        required: &["Mode"],
    },
    // ...and its `Target` is the schedule's target (`smithy_query(shape="Target")` of the
    // `scheduler` model), which declares the ARN it invokes AND the `RoleArn` EventBridge
    // Scheduler invokes it as required — a schedule has nothing to invoke without the one and
    // no identity to invoke it with without the other. The property is a single structure
    // rather than a list, so the failure is reported at the property itself.
    NestedRequiredMembers {
        resource_type: crate::state::template::SCHEDULER_SCHEDULE,
        structure: &["Target"],
        list: false,
        required: &["Arn", "RoleArn"],
    },
    // ...and the templated ECS target inside it, whose `EcsParameters` declares
    // `TaskDefinitionArn` required for the same reason EventBridge's does.
    NestedRequiredMembers {
        resource_type: crate::state::template::SCHEDULER_SCHEDULE,
        structure: &["Target", "EcsParameters"],
        list: false,
        required: &["TaskDefinitionArn"],
    },
    // ...the templated EventBridge target, whose `EventBridgeParameters` declares both the
    // `DetailType` that says "what fields to expect in the event detail" and the `Source` of
    // the event required: a `PutEvents` entry carries both.
    NestedRequiredMembers {
        resource_type: crate::state::template::SCHEDULER_SCHEDULE,
        structure: &["Target", "EventBridgeParameters"],
        list: false,
        required: &["DetailType", "Source"],
    },
    // ...the templated Kinesis target, whose `KinesisParameters` declares `PartitionKey`
    // required: the key is what "specifies the shard to which EventBridge Scheduler sends the
    // event".
    NestedRequiredMembers {
        resource_type: crate::state::template::SCHEDULER_SCHEDULE,
        structure: &["Target", "KinesisParameters"],
        list: false,
        required: &["PartitionKey"],
    },
    // ...and the templated SageMaker target, each entry of whose `PipelineParameterList` is
    // "the name and value pair of a parameter to use to start execution of a SageMaker Model
    // Building Pipeline", both required.
    NestedRequiredMembers {
        resource_type: crate::state::template::SCHEDULER_SCHEDULE,
        structure: &[
            "Target",
            "SageMakerPipelineParameters",
            "PipelineParameterList",
        ],
        list: true,
        required: &["Name", "Value"],
    },
];

/// Every member a resource's declaration does not carry in a nested structure it STATES that
/// that structure's contract requires ([`NESTED_REQUIRED_MEMBERS`]) — one error per missing
/// member: the rows of the catalogue in the order it holds them, within a row the structures in
/// the order the declaration states them, and within a structure the members in the order its
/// contract publishes them; empty for a type with no row, for a structure the declaration does
/// not state, and for one carrying every member its row names.
///
/// The structures a row is about are found by the ONE walk of this stage that steps a property
/// document down a chain of members ([`super::judged_at`]), so a structure nested in a
/// list-valued property is judged once per element and reported at that element's own pointer,
/// and how many structures one declaration is judged on is bounded there rather than by the
/// document — per row, so what one declaration of a type with `n` rows may have visited is `n`
/// times that bound, which is a product of two constants and of nothing a template states.
///
/// A member the structure carries is a member it STATES, whatever its value — the same reading
/// the document's own required properties get, so a member whose value has merely not landed
/// yet ([`crate::state::StoredStack::declarations_to_validate`]) is not reported as missing.
///
/// A value that is no structure at all judges nothing, which is the fail-open reading every
/// question of this stage gives a value it cannot decide about: a value the author wrote where
/// the schema states a structure fails a constraint another question judges, and an INTRINSIC
/// FUNCTION there ([`crate::state::template::is_intrinsic`] reads what one looks like) is an
/// expression this service left unevaluated rather than a structure missing every member.
pub(super) fn missing_nested_required_members(
    resource: &StoredResource,
) -> Vec<PropertyFailure> {
    NESTED_REQUIRED_MEMBERS
        .iter()
        .filter(|row| row.resource_type == resource.resource_type)
        .flat_map(|row| {
            super::judged_at(&resource.properties, row.structure)
                .into_iter()
                .filter(|(at, _)| row.list == is_element_of_a_list(at, row.structure))
                .filter_map(|(at, stated)| {
                    stated
                        .as_object()
                        .filter(|members| !crate::state::template::is_intrinsic(members))
                        .map(|members| (at, members))
                })
                .flat_map(|(at, members)| {
                    let at: Vec<&str> = at.iter().map(String::as_str).collect();
                    let path = super::member_path(&resource.logical_id, &at);
                    row.required
                        .iter()
                        .filter(|member| members.get(**member).is_none())
                        .map(|missing| PropertyFailure {
                            logical_id: resource.logical_id.clone(),
                            resource_type: resource.resource_type.clone(),
                            reason: super::missing_property_reason(missing),
                            path: path.clone(),
                        })
                        .collect::<Vec<PropertyFailure>>()
                })
                .collect::<Vec<PropertyFailure>>()
        })
        .collect()
}

/// Whether the structure found at `at` is one ELEMENT of a list the chain's last member
/// states, rather than that member's own value.
///
/// The walk names an element by its INDEX under the member it sits in, so a value reached as an
/// element carries one more step than the chain that named it ([`super::judged_at`] pushes it)
/// — which is what a row's [`NestedRequiredMembers::list`] is read against.
fn is_element_of_a_list(at: &[String], structure: &[&str]) -> bool {
    at.last().map(String::as_str) != structure.last().copied()
}

#[cfg(test)]
mod tests;
