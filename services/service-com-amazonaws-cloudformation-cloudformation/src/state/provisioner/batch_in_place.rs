// SPDX-License-Identifier: CC0-1.0

//! Applying a CHANGED declaration to an AWS Batch resource the stack already has, through
//! the one call that service publishes for reconfiguring that resource where it stands.
//!
//! Each of these types is created by an arm of [`super::network_and_delivery`] (the job
//! queue and the service environment) or of [`super::batch_share_policies`] (the scheduling
//! policy), and each is identified by the ARN AWS Batch minted for it — which is the
//! resource's physical id and what its `Ref` publishes. So a property that service's own
//! update call takes a member for is applied to the resource that exists, addressed by that
//! ARN, and the resource keeps it: no dependent reference of the stack is invalidated by a
//! change the owning API can make in place.
//!
//! A property the update call has NO member for is one AWS Batch fixes when it makes the
//! resource, which is why a declaration restating it replaces the resource instead of
//! reaching here ([`crate::state::template::replacement_properties`] names them, the
//! resource's own name being in that position for every Batch type through
//! [`crate::state::template::name_is_create_only`]).
//!
//! A member the declaration LEAVES OUT is not sent: each of these updates is documented to
//! patch the resource — "every member the request carries is written and every member it
//! omits is left exactly as it was" — so there is no value that means "no change", and
//! sending one invented here would be a change the template never asked for.

use super::*;

/// The `AWS::Batch::JobQueue` properties `UpdateJobQueue` takes whose value goes through
/// unchanged under its lower-camel member name.
///
/// Source: the type's entry in the "AWS resource and property types reference" paired with
/// `UpdateJobQueueRequest`'s own members (`smithy_query(query="operation",
/// operation="UpdateJobQueue")` of the `batch` model: `jobQueue`, `state`,
/// `schedulingPolicyArn`, `priority`, `computeEnvironmentOrder`, `serviceEnvironmentOrder`
/// and `jobStateTimeLimitActions`). These are the scalar ones, each published with "Update
/// requires: No interruption"; the LIST-valued three are carried by the mapping the CREATE of
/// the same type uses ([`carry_batch_entry_lists`], over [`BATCH_JOB_QUEUE_ENTRY_LISTS`]), so
/// the two paths cannot drift. `JobQueueName` and
/// `JobQueueType` are members of no part of that call, so a declaration restating either
/// replaces the queue rather than reaching this arm.
const BATCH_JOB_QUEUE_IN_PLACE_MEMBERS: &[(&str, &str)] = &[
    ("Priority", "priority"),
    ("State", "state"),
    ("SchedulingPolicyArn", "schedulingPolicyArn"),
];

/// The member `UpdateJobQueue` addresses the queue by: "the name or the Amazon Resource Name
/// (ARN) of the job queue", which this arm gives the ARN that is the resource's physical id.
const BATCH_JOB_QUEUE_IDENTIFIER_MEMBER: &str = "jobQueue";

/// The member `UpdateServiceEnvironment` and `DeleteServiceEnvironment` address a service
/// environment by, likewise the ARN that is the resource's physical id.
const BATCH_SERVICE_ENVIRONMENT_IDENTIFIER_MEMBER: &str = "serviceEnvironment";

/// The member `UpdateSchedulingPolicy` addresses a scheduling policy by, which is the only
/// handle that call takes (`arn`).
const BATCH_SCHEDULING_POLICY_IDENTIFIER_MEMBER: &str = "arn";

/// The `AWS::Batch::ServiceEnvironment` property `UpdateServiceEnvironment` takes whose value
/// goes through unchanged under its lower-camel member name.
///
/// Source: that type's reference paired with `UpdateServiceEnvironmentRequest`, whose members
/// are `serviceEnvironment`, `state` and `capacityLimits`. `CapacityLimits` is a LIST of
/// modelled structures, carried by the same mapping the create uses
/// ([`BATCH_SERVICE_ENVIRONMENT_ENTRY_LISTS`]);
/// `ServiceEnvironmentName` and `ServiceEnvironmentType` are members of no part of that call,
/// so a declaration restating either replaces the environment.
const BATCH_SERVICE_ENVIRONMENT_IN_PLACE_MEMBERS: &[(&str, &str)] = &[("State", "state")];

/// The two `AWS::Batch::SchedulingPolicy` properties whose value is a modelled STRUCTURE
/// `UpdateSchedulingPolicy` takes, under the member it is
/// (`UpdateSchedulingPolicyRequest`: `arn`, `quotaSharePolicy`, `fairsharePolicy`).
///
/// They are the policy's whole configuration — its `Name` is a member of the create alone —
/// so this is every property of the type an update can change. Each is carried under its
/// member with every member name inside it re-cased the way the CREATE of the same type
/// carries it ([`carry_structures`]).
const BATCH_SCHEDULING_POLICY_IN_PLACE_STRUCTURES: &[(&str, &str)] = &[
    ("FairsharePolicy", "fairsharePolicy"),
    ("QuotaSharePolicy", "quotaSharePolicy"),
];

/// Apply a changed declaration to the job queue the stack created, through `UpdateJobQueue` —
/// the one call AWS Batch publishes for reconfiguring a queue that exists.
///
/// Every property that call takes a member for travels in it, addressed by the queue's ARN,
/// so the queue keeps that ARN: a stack output that publishes it, and every declaration that
/// reads it, stay valid across the change. The accepted update leaves the queue in the
/// transient `UPDATING` status, so the resource is not reported changed until AWS Batch
/// reports it `VALID` again — the same reading the create makes of `CREATING`
/// ([`batch_await_valid`]).
pub(super) async fn update_batch_job_queue(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    let properties = declared.as_object().cloned().unwrap_or_else(Map::new);
    let mut request = Map::new();
    request.insert(
        BATCH_JOB_QUEUE_IDENTIFIER_MEMBER.to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    copy_members(&properties, BATCH_JOB_QUEUE_IN_PLACE_MEMBERS, &mut request);
    carry_batch_entry_lists(&properties, BATCH_JOB_QUEUE_ENTRY_LISTS, &mut request);
    let reported = call_rest_json(
        ctx,
        BATCH_SIGNING_NAME,
        region,
        "POST",
        BATCH_UPDATE_JOB_QUEUE_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    batch_await_valid(
        ctx,
        region,
        BATCH_DESCRIBE_JOB_QUEUES_URI,
        "jobQueues",
        &resource.physical_resource_id,
    )
    .await?;
    Ok(Some(Created::attributes(answered_arn_attribute(
        &reported,
        "jobQueueArn",
        "JobQueueArn",
        resource,
    ))))
}

/// Apply a changed declaration to the service environment the stack created, through
/// `UpdateServiceEnvironment` — the one call AWS Batch publishes for reconfiguring one that
/// exists, which takes its state and the capacity it is limited to.
///
/// The environment keeps the ARN the create minted, which is what the queues of the same
/// stack order it by: a `ServiceEnvironmentOrder` entry naming it is not invalidated by a
/// change to the capacity behind it. That call leaves the environment's status where it is,
/// so there is nothing to wait for.
pub(super) async fn update_batch_service_environment(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    let properties = declared.as_object().cloned().unwrap_or_else(Map::new);
    let mut request = Map::new();
    request.insert(
        BATCH_SERVICE_ENVIRONMENT_IDENTIFIER_MEMBER.to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    copy_members(
        &properties,
        BATCH_SERVICE_ENVIRONMENT_IN_PLACE_MEMBERS,
        &mut request,
    );
    carry_batch_entry_lists(&properties, BATCH_SERVICE_ENVIRONMENT_ENTRY_LISTS, &mut request);
    let reported = call_rest_json(
        ctx,
        BATCH_SIGNING_NAME,
        region,
        "POST",
        BATCH_UPDATE_SERVICE_ENVIRONMENT_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    Ok(Some(Created::attributes(answered_arn_attribute(
        &reported,
        "serviceEnvironmentArn",
        "ServiceEnvironmentArn",
        resource,
    ))))
}

/// Apply a changed declaration to the scheduling policy the stack created, through
/// `UpdateSchedulingPolicy` — the one call AWS Batch publishes for reconfiguring a policy
/// that exists, which takes the policy's ARN with the share policy it now allocates by.
///
/// The policy keeps that ARN, which is what the `SchedulingPolicyArn` of a job queue in the
/// same stack was created against, so a change to how the policy shares capacity does not
/// disturb the queue scheduled by it. That call answers nothing, and the type publishes only
/// that ARN, so what the resource publishes is unchanged.
pub(super) async fn update_batch_scheduling_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    let properties = declared.as_object().cloned().unwrap_or_else(Map::new);
    let mut request = Map::new();
    request.insert(
        BATCH_SCHEDULING_POLICY_IDENTIFIER_MEMBER.to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    carry_structures(
        &properties,
        BATCH_SCHEDULING_POLICY_IN_PLACE_STRUCTURES,
        &mut request,
    )?;
    call_rest_json(
        ctx,
        BATCH_SIGNING_NAME,
        region,
        "POST",
        BATCH_UPDATE_SCHEDULING_POLICY_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    let mut attributes = BTreeMap::new();
    attributes.insert(
        SCHEDULING_POLICY_ARN_ATTRIBUTE.to_string(),
        resource.physical_resource_id.clone(),
    );
    Ok(Some(Created::attributes(attributes)))
}

/// `UpdateSchedulingPolicy`'s URI, from the AWS Batch model's own `@http` trait
/// (`smithy_query(query="operation", operation="UpdateSchedulingPolicy")`: a `POST`).
const BATCH_UPDATE_SCHEDULING_POLICY_URI: &str = "/v1/updateschedulingpolicy";

/// The ARN the resource publishes after an update: the one that service answered the update
/// with, and the resource's own physical id where the answer carried none.
///
/// The two are the same ARN — an in-place update does not move a Batch resource's identity —
/// so the fall-back publishes what the resource already is rather than dropping the attribute
/// a `Fn::GetAtt` of the stack reads.
fn answered_arn_attribute(
    reported: &Value,
    member: &str,
    attribute: &str,
    resource: &StoredResource,
) -> BTreeMap<String, String> {
    let arn = reported
        .get(member)
        .and_then(Value::as_str)
        .filter(|arn| !arn.is_empty())
        .unwrap_or(&resource.physical_resource_id)
        .to_string();
    let mut attributes = BTreeMap::new();
    attributes.insert(attribute.to_string(), arn);
    attributes
}
