// SPDX-License-Identifier: CC0-1.0

//! The per-type provisioning arms of `AWS::AutoScaling::ScalingPolicy`: the scaling policy a
//! stack attaches to an Auto Scaling group. One file of [`super`]'s per-type arms, dispatched
//! to by its `Provisioner`.
//!
//! ## The rule this file carries
//!
//! A stack reports a resource created only where the service that OWNS the type really holds
//! it. A scaling policy is Amazon EC2 Auto Scaling's own resource — the one
//! `autoscaling:DescribePolicies` answers for, `autoscaling:ExecutePolicy` performs and
//! `autoscaling:DeletePolicy` removes — so a stack that reached `CREATE_COMPLETE` with one
//! declared is a stack whose client can read that policy back, execute it and see the group it
//! steers move.
//!
//! The type's properties ARE `PutScalingPolicy`'s own members ("creates or updates a scaling
//! policy for an Auto Scaling group"), each under the same name, and that call is `awsQuery`
//! like the group's own create — so the evaluated document is walked by
//! [`query_members`]. Two members are the arm's own:
//!
//! * `PolicyName`, which the type does not publish: CloudFormation decides the policy's name,
//!   and the name it decided is the name the policy is created under — carried on the record as
//!   its `Name`, because it is NOT this resource's physical id;
//! * the policy's ARN, which Amazon EC2 Auto Scaling mints and the arm reads back from that
//!   service rather than computing — the read that also makes the create's success a resource
//!   that service really holds. That ARN is this resource's physical id: "Ref returns the
//!   Amazon Resource Name (ARN) of the scaling policy"
//!   (`AWS::AutoScaling::ScalingPolicy`, Return values), so what the stack publishes for the
//!   resource is the value `ExecutePolicy` and `DeletePolicy` resolve to a policy by on their
//!   own, with no group beside it.

use super::*;

/// The template resource type this file provisions.
pub(in crate::state) const AUTOSCALING_SCALING_POLICY: &str = "AWS::AutoScaling::ScalingPolicy";

/// The members `PutScalingPolicy` takes, from Amazon EC2 Auto Scaling's own model
/// (`smithy_query(query="operation",operation="PutScalingPolicy",
/// model_path="services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/\
/// smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json")`), with each list's
/// element name from its list shape's member `@xmlName` — `StepAdjustments` fixes none, so it
/// carries the Smithy default, `member`.
///
/// This is the catalogue the declaration of an `AWS::AutoScaling::ScalingPolicy` is walked
/// against: the type's properties ARE these members, so a property whose name is one of them
/// is passed through under it, and one the catalogue does not carry is refused rather than
/// dropped ([`query_members`]).
const PUT_SCALING_POLICY_MEMBERS: &[(&str, QueryMember)] = &[
    ("AdjustmentType", QueryMember::Scalar),
    ("AutoScalingGroupName", QueryMember::Scalar),
    ("Cooldown", QueryMember::Scalar),
    ("Enabled", QueryMember::Scalar),
    ("EstimatedInstanceWarmup", QueryMember::Scalar),
    ("MetricAggregationType", QueryMember::Scalar),
    ("MinAdjustmentMagnitude", QueryMember::Scalar),
    ("PolicyType", QueryMember::Scalar),
    ("PredictiveScalingConfiguration", QueryMember::Structure),
    ("ScalingAdjustment", QueryMember::Scalar),
    ("StepAdjustments", QueryMember::List("member")),
    ("TargetTrackingConfiguration", QueryMember::Structure),
];

/// The `AWS::AutoScaling::ScalingPolicy` property the walk does NOT turn into a
/// `PutScalingPolicy` member: the policy's own name, which CloudFormation decides and which
/// the arm sets from the name on this resource's record ([`declared_name`]) rather than from its
/// physical id, that being the policy's ARN.
const SCALING_POLICY_PROPERTIES_NOT_SENT: &[&str] = &["PolicyName"];

/// The attribute this type publishes the policy's ARN under through `Fn::GetAtt`.
const POLICY_ARN_ATTRIBUTE: &str = "Arn";

/// The attribute it publishes the policy's NAME under, which is also where the record carries
/// the name the policy was created under ([`declared_name`]): the physical id is the ARN, so
/// every later call that addresses the policy by name reads it from here.
const POLICY_NAME_ATTRIBUTE: &str = "PolicyName";

/// The attribute the uniform return-value convention carries a resource's name under, which is
/// what [`declared_name`] reads for a type whose physical id is not that name.
const NAME_ATTRIBUTE: &str = "Name";

/// The member `PutScalingPolicy` and `DescribePolicies` report a policy's ARN under.
const POLICY_ARN_MEMBER: &str = "PolicyARN";

/// Put the scaling policy an `AWS::AutoScaling::ScalingPolicy` declares, in Amazon EC2 Auto
/// Scaling, and report the ARN that service minted for it as this resource's identity.
///
/// The policy is created under the name CloudFormation decided for it, and read back through
/// that service's own `DescribePolicies`: a create this arm reports as succeeded is a policy a
/// client resolves, so nothing is reported created that the owning service does not hold. The
/// identity the stack publishes for it is the ARN that read answered — "Ref returns the Amazon
/// Resource Name (ARN) of the scaling policy" — so a policy the owning service minted no ARN
/// for is a resource with no identity to publish, which is a failed create rather than a
/// success.
pub(super) async fn put_scaling_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let name = declared_name(resource);
    let mut parameters = query_members(
        &resource.properties,
        PUT_SCALING_POLICY_MEMBERS,
        &[],
        SCALING_POLICY_PROPERTIES_NOT_SENT,
        AUTOSCALING_SCALING_POLICY,
    )?;
    parameters.push(("PolicyName".to_string(), name.clone()));
    call_query(
        ctx,
        AUTOSCALING_SIGNING_NAME,
        region,
        AUTOSCALING_API_VERSION,
        "PutScalingPolicy",
        &parameters,
    )
    .await?;
    let reported = describe_scaling_policy(ctx, region, &resource.properties, &name)
        .await?
        .ok_or_else(|| Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: format!(
                "PutScalingPolicy was accepted but Amazon EC2 Auto Scaling reports no policy \
                 named {name}"
            ),
        })?;
    let policy_arn = reported
        .get(POLICY_ARN_MEMBER)
        .and_then(Value::as_str)
        .filter(|arn| !arn.is_empty())
        .ok_or_else(|| Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: format!(
                "Amazon EC2 Auto Scaling reported no {POLICY_ARN_MEMBER} for the scaling policy \
                 {name}, which is what a reference to this resource resolves to"
            ),
        })?
        .to_string();
    Ok(Created {
        physical_resource_id: Some(policy_arn.clone()),
        attributes: BTreeMap::from([
            (POLICY_NAME_ATTRIBUTE.to_string(), name.clone()),
            (NAME_ATTRIBUTE.to_string(), name),
            (POLICY_ARN_ATTRIBUTE.to_string(), policy_arn),
        ]),
    })
}

/// Apply a CHANGED `AWS::AutoScaling::ScalingPolicy` declaration to the policy that exists,
/// through `PutScalingPolicy` again — the one call that states a policy's configuration,
/// documented "creates or updates a scaling policy for an Auto Scaling group", so the policy
/// keeps the name and the ARN it was created with rather than being replaced.
///
/// `Ok(None)` is a change that call cannot make: a policy declared on a DIFFERENT Auto Scaling
/// group is a different policy, and the group it was on would keep the policy this resource
/// put there, so the deployment reports the change as not applied rather than as a success
/// that left the old group steered.
pub(super) async fn update_scaling_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    if declared_group(declared) != declared_group(&resource.properties) {
        return Ok(None);
    }
    let name = declared_name(resource);
    let mut parameters = query_members(
        declared,
        PUT_SCALING_POLICY_MEMBERS,
        &[],
        SCALING_POLICY_PROPERTIES_NOT_SENT,
        AUTOSCALING_SCALING_POLICY,
    )?;
    parameters.push(("PolicyName".to_string(), name));
    call_query(
        ctx,
        AUTOSCALING_SIGNING_NAME,
        region,
        AUTOSCALING_API_VERSION,
        "PutScalingPolicy",
        &parameters,
    )
    .await?;
    Ok(Some(Created::default()))
}

/// Delete the scaling policy a stack created, addressed by the group it belongs to and the
/// name it was created under — the name this resource's record carries ([`declared_name`]),
/// since its physical id is the policy's ARN.
///
/// A policy Amazon EC2 Auto Scaling no longer holds is already in the state the delete asks
/// for, which is read from that service's own report of the policy rather than from the
/// refusal: every absent-resource refusal of this API is the one `ValidationError` code, so
/// the code alone cannot tell an absent policy from a request refused for another reason.
pub(super) async fn delete_scaling_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let name = declared_name(resource);
    if describe_scaling_policy(ctx, region, &resource.properties, &name)
        .await?
        .is_none()
    {
        return Ok(());
    }
    let mut parameters = vec![("PolicyName".to_string(), name)];
    if let Some(group) = declared_group(&resource.properties) {
        parameters.push(("AutoScalingGroupName".to_string(), group));
    }
    call_query(
        ctx,
        AUTOSCALING_SIGNING_NAME,
        region,
        AUTOSCALING_API_VERSION,
        "DeletePolicy",
        &parameters,
    )
    .await
    .map(|_| ())
}

/// The scaling policy as Amazon EC2 Auto Scaling reports it NOW, or `None` where it reports no
/// such policy: the scalar members of the one entry `DescribePolicies` answers for the name,
/// which are named exactly as this type's properties are (`AdjustmentType`, `PolicyType`,
/// `ScalingAdjustment`, `Cooldown`) beside the policy's own ARN.
///
/// This is both the read the create confirms itself with and the ACTUAL configuration drift
/// detection compares the declared one against. An awsQuery answer carries each policy as a
/// `<member>` of its `ScalingPolicies` list, and the request names ONE policy, so the first
/// such element is that policy ([`reported_child`]). The group is named beside it where the declaration states one,
/// because a policy name is unique within a group rather than within the account.
pub(super) async fn describe_scaling_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    properties: &Value,
    name: &str,
) -> Result<Option<Value>, Refusal> {
    let mut parameters = vec![("PolicyNames.member.1".to_string(), name.to_string())];
    if let Some(group) = declared_group(properties) {
        parameters.push(("AutoScalingGroupName".to_string(), group));
    }
    let answered = call_query(
        ctx,
        AUTOSCALING_SIGNING_NAME,
        region,
        AUTOSCALING_API_VERSION,
        "DescribePolicies",
        &parameters,
    )
    .await?;
    Ok(reported_child(&answered, "PolicyName", name))
}

/// The Auto Scaling group a declaration states the policy on: "the name of the Auto Scaling
/// group" (`PutScalingPolicyType$AutoScalingGroupName`), which every call that addresses the
/// policy takes beside its name.
///
/// THE ONE reader of that property for this service's types: every type a stack declares ON a
/// group names the group in that same property, so the group of a lifecycle hook, a scheduled
/// action or a warm pool is read here too ([`super::auto_scaling_group_children`]).
pub(super) fn declared_group(properties: &Value) -> Option<String> {
    properties
        .get("AutoScalingGroupName")
        .map(attribute_text)
        .filter(|group| !group.is_empty())
}
