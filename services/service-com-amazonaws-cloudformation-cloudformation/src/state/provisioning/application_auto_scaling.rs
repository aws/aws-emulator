// SPDX-License-Identifier: CC0-1.0

//! `AWS::ApplicationAutoScaling::ScalableTarget` and `AWS::ApplicationAutoScaling::ScalingPolicy`:
//! the scalable target a stack declares and the policy that steers it, registered in
//! Application Auto Scaling itself.
//!
//! This is the per-type half of provisioning for two resource types, the half
//! [`super::super::provisioner`] holds for every other type this emulator creates; it lives
//! in its own module beside [`super::lambda_function`] for the reason that module records —
//! that file is far past the length a change to this repository may touch — and is reached
//! through the same seam ([`super::Owner`]).
//!
//! ## Why they are really registered
//!
//! A stack that declares autoscaling for a table has to leave autoscaling BEHIND it: the
//! whole point of the declaration is that `DescribeScalableTargets` and
//! `DescribeScalingPolicies` report the target and the policy afterwards, and that the policy
//! moves the resource's capacity. So each type's create is the owning service's own call —
//! `RegisterScalableTarget` for the target, `PutScalingPolicy` for the policy — and a
//! declaration that service REFUSES (a capacity window whose minimum is above its maximum, a
//! resource that does not exist) fails the resource, and with it the stack, rather than being
//! reported as deployed.
//!
//! ## The properties
//!
//! `AWS::ApplicationAutoScaling::ScalableTarget`'s properties are `RegisterScalableTarget`'s
//! own members under the same names, with two exceptions:
//!
//! * `ScheduledActions` is a LIST of the scheduled actions on the target, which that service
//!   takes one `PutScheduledAction` call per — each entry's members being that call's own,
//!   under the same names;
//! * `SuspendedState` is that call's own member and travels as written.
//!
//! The type declares no `Tags` property, so there is no tag for CloudFormation to propagate
//! to one of these resources.
//!
//! `AWS::ApplicationAutoScaling::ScalingPolicy`'s properties are `PutScalingPolicy`'s own
//! members under the same names, plus `ScalingTargetId`: "the CloudFormation-generated ID of
//! an Application Auto Scaling scalable target", which is the physical id of the
//! `ScalableTarget` resource the policy is attached to and carries the target's whole identity
//! ([`split_scaling_target_id`]) — so a declaration states either that id or the three
//! identity members, and both reach the same call.
//!
//! ## The identity of each
//!
//! A scalable target's physical id is the `<ResourceId>|<ScalableDimension>|<ServiceNamespace>`
//! composite the target is identified by, which no naming rule could compute from the
//! declaration's name (the type declares none), and a scaling policy's is the `PolicyARN` its
//! own put answers. Both are adopted from the create
//! ([`super::super::template::identity_is_service_assigned`] names the two types), so a
//! policy whose `ScalingTargetId` is a `Ref` of the target resource is re-pointed at that
//! composite before its own create is made.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::{call_json_rpc, Created, Refusal, AWS_JSON_1_1};
use crate::state::StoredResource;

/// The resource types this module provisions. Visible to the rest of this service's state
/// because more than one module answers about them: the identity of each is the one its
/// create answers ([`crate::state::template::identity_is_service_assigned`]).
pub(in crate::state) const APPLICATION_AUTO_SCALING_SCALABLE_TARGET: &str =
    "AWS::ApplicationAutoScaling::ScalableTarget";
/// See [`APPLICATION_AUTO_SCALING_SCALABLE_TARGET`].
pub(in crate::state) const APPLICATION_AUTO_SCALING_SCALING_POLICY: &str =
    "AWS::ApplicationAutoScaling::ScalingPolicy";

/// Application Auto Scaling's signing name, which is what routes the in-process call to its
/// own handlers, and the service shape an awsJson `X-Amz-Target` names an operation under
/// (that model's `sigv4Name: application-autoscaling` and service shape
/// `AnyScaleFrontendService`, `smithy_query(query="service_info")`). It speaks `awsJson1_1`.
const APPLICATION_AUTO_SCALING_SIGNING_NAME: &str = "application-autoscaling";
/// See [`APPLICATION_AUTO_SCALING_SIGNING_NAME`].
const APPLICATION_AUTO_SCALING_TARGET_PREFIX: &str = "AnyScaleFrontendService";

/// The error code Application Auto Scaling answers for a target, a policy or a scheduled
/// action it does not hold (`ObjectNotFoundException`, the modelled 404 of its deletes),
/// which is what says a delete has nothing left to remove.
const OBJECT_NOT_FOUND: &str = "ObjectNotFoundException";

/// The code a refusal of this module's own carries for a declaration it cannot act on: the
/// same one the catalogue's arms use for a property this emulator does not apply.
const NOT_IMPLEMENTED: &str = "NotImplemented";

/// The code a refusal carries for a declaration that states none of a required property.
const MISSING_PROPERTY: &str = "InvalidRequest";

/// The separator the composite identity of a scalable target is written with: "the
/// CloudFormation-generated ID of an Application Auto Scaling scalable target" is the
/// resource id, the scalable dimension and the service namespace joined by it.
const IDENTITY_SEPARATOR: char = '|';

/// Which of the two types one resource is.
///
/// Visible to the whole of this service's state module, because it travels with
/// [`super::Owner`]: the algorithm resolves a type's owner once and passes it back here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::state) enum Kind {
    /// `AWS::ApplicationAutoScaling::ScalableTarget`.
    ScalableTarget,
    /// `AWS::ApplicationAutoScaling::ScalingPolicy`.
    ScalingPolicy,
}

/// The kind `resource_type` names, or `None` for a type this module does not own.
pub(super) fn kind_of(resource_type: &str) -> Option<Kind> {
    match resource_type {
        APPLICATION_AUTO_SCALING_SCALABLE_TARGET => Some(Kind::ScalableTarget),
        APPLICATION_AUTO_SCALING_SCALING_POLICY => Some(Kind::ScalingPolicy),
        _ => None,
    }
}

/// The three members that identify a scalable target, which both types declare and every
/// call of the owning service takes.
const IDENTITY_MEMBERS: [&str; 3] = ["ResourceId", "ScalableDimension", "ServiceNamespace"];

/// The members of `RegisterScalableTarget` an `AWS::ApplicationAutoScaling::ScalableTarget`
/// declares under the same names.
///
/// Source: `RegisterScalableTarget`'s member list
/// (`smithy_query(query="operation",operation="RegisterScalableTarget")`), intersected with
/// the properties the type publishes. `Tags` is a member of the call that the TYPE does not
/// publish, so no declaration can carry one.
const TARGET_MEMBERS: &[&str] = &[
    "MaxCapacity",
    "MinCapacity",
    "ResourceId",
    "RoleARN",
    "ScalableDimension",
    "ServiceNamespace",
    "SuspendedState",
];

/// Every property `AWS::ApplicationAutoScaling::ScalableTarget` publishes. A declared
/// property this list does not carry belongs to no such resource and is refused rather than
/// dropped.
const TARGET_PROPERTIES: &[&str] = &[
    "MaxCapacity",
    "MinCapacity",
    "ResourceId",
    "RoleARN",
    "ScalableDimension",
    "ScheduledActions",
    "ServiceNamespace",
    "SuspendedState",
];

/// The property carrying the scheduled actions on a target, one `PutScheduledAction` each.
const SCHEDULED_ACTIONS: &str = "ScheduledActions";

/// The members of `PutScheduledAction` one `ScheduledActions` entry states under the same
/// names, beside the target identity the call is addressed by.
///
/// Source: `PutScheduledAction`'s member list, intersected with the properties the type's
/// `ScheduledAction` holds.
const SCHEDULED_ACTION_MEMBERS: &[&str] = &[
    "EndTime",
    "ScalableTargetAction",
    "Schedule",
    "ScheduledActionName",
    "StartTime",
    "Timezone",
];

/// The members of `PutScalingPolicy` an `AWS::ApplicationAutoScaling::ScalingPolicy`
/// declares under the same names.
const POLICY_MEMBERS: &[&str] = &[
    "PolicyName",
    "PolicyType",
    "PredictiveScalingPolicyConfiguration",
    "ResourceId",
    "ScalableDimension",
    "ServiceNamespace",
    "StepScalingPolicyConfiguration",
    "TargetTrackingScalingPolicyConfiguration",
];

/// Every property `AWS::ApplicationAutoScaling::ScalingPolicy` publishes: the call's own
/// members above, plus the id of the scalable target the policy is attached to.
const POLICY_PROPERTIES: &[&str] = &[
    "PolicyName",
    "PolicyType",
    "PredictiveScalingPolicyConfiguration",
    "ResourceId",
    "ScalableDimension",
    "ScalingTargetId",
    "ServiceNamespace",
    "StepScalingPolicyConfiguration",
    "TargetTrackingScalingPolicyConfiguration",
];

/// The property naming the scalable target a policy is attached to.
const SCALING_TARGET_ID: &str = "ScalingTargetId";

/// The member `PutScalingPolicy` answers the policy's ARN under, which is this resource's
/// physical id and the `Arn` attribute the type publishes.
const POLICY_ARN_MEMBER: &str = "PolicyARN";

/// The attribute `AWS::ApplicationAutoScaling::ScalingPolicy` publishes through `Fn::GetAtt`
/// ("Arn: returns the ARN of a scaling policy").
const ARN_ATTRIBUTE: &str = "Arn";

/// The resource id, scalable dimension and service namespace a `ScalingTargetId` carries.
///
/// A scalable target "is uniquely identified by the combination of resource ID, scalable
/// dimension, and namespace", and the composite the id is written as states them in that
/// order. `None` for a value that is no such composite, which names no target.
fn split_scaling_target_id(scaling_target_id: &str) -> Option<[&str; 3]> {
    let mut fields = scaling_target_id.split(IDENTITY_SEPARATOR);
    let (Some(resource_id), Some(scalable_dimension), Some(service_namespace), None) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    (!resource_id.is_empty() && !scalable_dimension.is_empty() && !service_namespace.is_empty())
        .then_some([resource_id, scalable_dimension, service_namespace])
}

/// The composite id a scalable target is identified by, built from the identity its
/// declaration states.
fn scaling_target_id(identity: &Map<String, Value>) -> String {
    IDENTITY_MEMBERS
        .iter()
        .map(|member| {
            identity
                .get(*member)
                .and_then(Value::as_str)
                .unwrap_or_default()
        })
        .collect::<Vec<&str>>()
        .join(&IDENTITY_SEPARATOR.to_string())
}

/// Create the resource one of these types declares, in Application Auto Scaling itself — so
/// a client that deployed the stack reads the target back with `DescribeScalableTargets` and
/// the policy with `DescribeScalingPolicies`, and the policy really steers the resource.
///
/// Neither type publishes a `Tags` property, so CloudFormation propagates no tag to one of
/// these resources: there is no call that could carry one.
pub(super) async fn create(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    match kind {
        Kind::ScalableTarget => create_target(ctx, region, resource).await,
        Kind::ScalingPolicy => put_policy(ctx, region, &resource.properties, resource).await,
    }
}

/// Register the target an `AWS::ApplicationAutoScaling::ScalableTarget` declares, and put
/// every scheduled action its declaration carries.
async fn create_target(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    register_target(ctx, region, &resource.properties, resource).await?;
    for action in declared_scheduled_actions(&resource.properties) {
        put_scheduled_action(ctx, region, &resource.properties, resource, &action).await?;
    }
    Ok(Created {
        // The identity in force is the composite the target is identified by, which no
        // naming rule computes from the declaration.
        physical_resource_id: Some(scaling_target_id(&target_identity(
            &resource.properties,
            resource,
        )?)),
        attributes: BTreeMap::new(),
    })
}

/// Register the target one declaration states.
async fn register_target(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    declared: &Value,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    refuse_unapplied_properties(declared, &resource.resource_type, TARGET_PROPERTIES)?;
    let mut request = declared_members(declared, TARGET_MEMBERS);
    for (member, value) in target_identity(declared, resource)? {
        request.insert(member, value);
    }
    call_application_auto_scaling(ctx, region, "RegisterScalableTarget", &Value::Object(request))
        .await
        .map(|_| ())
}

/// Put one scheduled action on the target `declared` states.
async fn put_scheduled_action(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    declared: &Value,
    resource: &StoredResource,
    action: &Value,
) -> Result<(), Refusal> {
    let mut request = declared_members(action, SCHEDULED_ACTION_MEMBERS);
    for (member, value) in target_identity(declared, resource)? {
        request.insert(member, value);
    }
    call_application_auto_scaling(ctx, region, "PutScheduledAction", &Value::Object(request))
        .await
        .map(|_| ())
}

/// Put the policy an `AWS::ApplicationAutoScaling::ScalingPolicy` declares, and report the
/// ARN the owning service answered as the resource's identity and its `Arn` attribute.
async fn put_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    declared: &Value,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    refuse_unapplied_properties(declared, &resource.resource_type, POLICY_PROPERTIES)?;
    let mut request = declared_members(declared, POLICY_MEMBERS);
    for (member, value) in policy_identity(declared, resource)? {
        request.insert(member, value);
    }
    let answered =
        call_application_auto_scaling(ctx, region, "PutScalingPolicy", &Value::Object(request))
            .await?;
    let policy_arn = answered
        .get(POLICY_ARN_MEMBER)
        .and_then(Value::as_str)
        .filter(|arn| !arn.is_empty())
        .ok_or_else(|| Refusal {
            code: MISSING_PROPERTY.to_string(),
            message: format!(
                "Application Auto Scaling reported no {POLICY_ARN_MEMBER} for the scaling policy \
                 it created, which is what a reference to this resource resolves to"
            ),
        })?
        .to_string();
    Ok(Created {
        physical_resource_id: Some(policy_arn.clone()),
        attributes: BTreeMap::from([(ARN_ATTRIBUTE.to_string(), policy_arn)]),
    })
}

/// Whether the owning service reports the resource ready.
///
/// Nothing is asked, and nothing waits: a scalable target is registered the moment
/// `RegisterScalableTarget` answers — `DescribeScalableTargets` reports it and a policy may
/// be attached to it — and a policy is in force the moment `PutScalingPolicy` answers.
/// Neither call reports a non-terminal state for the resource to settle out of.
pub(super) fn ready() -> super::Readiness {
    super::Readiness::Ready
}

/// Apply a CHANGED declaration to the resource the stack already has, through the owning
/// service's own call.
///
/// For a target that is `RegisterScalableTarget` again, which "registers OR UPDATES a
/// scalable target" under the same identity — so the target keeps its identity and nothing is
/// replaced — plus the scheduled actions the new declaration states, and the delete of every
/// action it no longer names. For a policy it is `PutScalingPolicy` again, the one call that
/// states a policy's configuration, which replaces the one in force.
///
/// `Ok(None)` is a change neither call can make: a resource moved to a DIFFERENT scalable
/// target is a different resource, and the target it was on would keep what this resource put
/// there, so the deployment reports the change as not applied rather than as a success that
/// left the old target configured.
pub(super) async fn update(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    // The DECLARED document, not the record's: by the time this is reached the resource
    // carries the properties it was CREATED with, so a property the new declaration
    // introduces is only in `declared`.
    if moved_to_another_target(kind, resource, declared) {
        return Ok(None);
    }
    match kind {
        Kind::ScalableTarget => {
            register_target(ctx, region, declared, resource).await?;
            let declared_actions = declared_scheduled_actions(declared);
            for action in &declared_actions {
                put_scheduled_action(ctx, region, declared, resource, action).await?;
            }
            // An action the declaration DROPPED is one this resource put there and no longer
            // asks for, so it is deleted rather than left behind.
            let wanted: Vec<String> = declared_actions
                .iter()
                .filter_map(scheduled_action_name)
                .collect();
            for name in declared_scheduled_actions(&resource.properties)
                .iter()
                .filter_map(scheduled_action_name)
            {
                if !wanted.contains(&name) {
                    delete_scheduled_action(ctx, region, resource, &name).await?;
                }
            }
            Ok(Some(Created::attributes(BTreeMap::new())))
        }
        Kind::ScalingPolicy => put_policy(ctx, region, declared, resource).await.map(Some),
    }
}

/// Whether a changed declaration moves the resource onto ANOTHER scalable target, which is a
/// different resource rather than a change to this one.
fn moved_to_another_target(kind: Kind, resource: &StoredResource, declared: &Value) -> bool {
    let members: &[&str] = match kind {
        Kind::ScalableTarget => &IDENTITY_MEMBERS,
        Kind::ScalingPolicy => &["ResourceId", "ScalableDimension", "ServiceNamespace", SCALING_TARGET_ID],
    };
    members.iter().any(|member| {
        let (Some(now), Some(before)) = (
            declared.get(*member).and_then(Value::as_str),
            resource.properties.get(*member).and_then(Value::as_str),
        ) else {
            return false;
        };
        now != before
    })
}

/// Delete the resource a stack created.
///
/// A target is deleted by `DeregisterScalableTarget`, which "deletes the specified scalable
/// target" and with it "any scaling policies and any scheduled actions" of that target, so
/// its scheduled actions need no delete of their own. A policy is deleted by
/// `DeleteScalingPolicy`, addressed by the name and the target its declaration states. Either
/// way a resource the owning service no longer holds is already in the state the delete asks
/// for, judged on the CODE and never on a message.
pub(super) async fn delete(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let (operation, identity) = match kind {
        Kind::ScalableTarget => (
            "DeregisterScalableTarget",
            target_identity(&resource.properties, resource)?,
        ),
        Kind::ScalingPolicy => {
            let mut request = policy_identity(&resource.properties, resource)?;
            let policy_name = resource
                .properties
                .get("PolicyName")
                .and_then(Value::as_str)
                .ok_or_else(|| missing(&resource.resource_type, "PolicyName"))?;
            request.insert(
                "PolicyName".to_string(),
                Value::String(policy_name.to_string()),
            );
            ("DeleteScalingPolicy", request)
        }
    };
    match call_application_auto_scaling(ctx, region, operation, &Value::Object(identity)).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == OBJECT_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// Delete one scheduled action a changed declaration no longer names.
async fn delete_scheduled_action(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    name: &str,
) -> Result<(), Refusal> {
    let mut request = target_identity(&resource.properties, resource)?;
    request.insert(
        "ScheduledActionName".to_string(),
        Value::String(name.to_string()),
    );
    match call_application_auto_scaling(
        ctx,
        region,
        "DeleteScheduledAction",
        &Value::Object(request),
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == OBJECT_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The scheduled actions a declaration carries, in the order it states them.
fn declared_scheduled_actions(properties: &Value) -> Vec<Value> {
    properties
        .get(SCHEDULED_ACTIONS)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `ScheduledActionName` one entry states, which is what a later deployment compares the
/// declared actions by.
fn scheduled_action_name(action: &Value) -> Option<String> {
    action
        .get("ScheduledActionName")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .map(ToString::to_string)
}

/// The three identity members every call on a scalable target takes, as one declaration
/// states them.
///
/// Each is a REQUIRED property of the type, so a declaration carrying none of one describes
/// no target that could be registered: it is refused here, naming that property, rather than
/// sent as a request the owning service would reject.
fn target_identity(
    declared: &Value,
    resource: &StoredResource,
) -> Result<Map<String, Value>, Refusal> {
    let mut identity = Map::new();
    for member in IDENTITY_MEMBERS {
        let value = declared
            .get(member)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| missing(&resource.resource_type, member))?;
        identity.insert(member.to_string(), Value::String(value.to_string()));
    }
    Ok(identity)
}

/// The identity of the target a POLICY is attached to: the three members its declaration
/// states, or the ones the `ScalingTargetId` it names carries.
///
/// A policy declares one or the other — "ScalingTargetId: the CloudFormation-generated ID of
/// an Application Auto Scaling scalable target" stands in for the triple — and both reach the
/// same call, so the composite is read apart here rather than at the call site.
fn policy_identity(
    declared: &Value,
    resource: &StoredResource,
) -> Result<Map<String, Value>, Refusal> {
    if let Some(scaling_target_id) = declared.get(SCALING_TARGET_ID).and_then(Value::as_str) {
        let Some(fields) = split_scaling_target_id(scaling_target_id) else {
            return Err(Refusal {
                code: MISSING_PROPERTY.to_string(),
                message: format!(
                    "{} declares a {SCALING_TARGET_ID} that names no scalable target",
                    resource.resource_type
                ),
            });
        };
        return Ok(IDENTITY_MEMBERS
            .iter()
            .zip(fields)
            .map(|(member, value)| ((*member).to_string(), Value::String(value.to_string())))
            .collect());
    }
    target_identity(declared, resource)
}

/// The members of `members` a declaration states, as the request carries them: the type and
/// the API spell each one identically, so every value goes through as the template wrote it —
/// read as the TYPE the owning API models it as ([`modelled`]).
fn declared_members(properties: &Value, members: &[&str]) -> Map<String, Value> {
    let mut request = Map::new();
    for member in members {
        if let Some(value) = properties.get(*member).filter(|value| !value.is_null()) {
            request.insert((*member).to_string(), modelled(member, value));
        }
    }
    request
}

/// The members these types declare that the owning API models as NUMBERS.
///
/// Source: the member lists of `RegisterScalableTarget`, `PutScalingPolicy` and
/// `PutScheduledAction` and of the structures they carry
/// (`smithy_query(query="shape",shape="TargetTrackingScalingPolicyConfiguration")` and its
/// siblings): the two capacities, a target utilisation, the cooldowns, a step adjustment's
/// bounds and its adjustment, and the predictive buffers.
const NUMERIC_MEMBERS: &[&str] = &[
    "Cooldown",
    "MaxCapacity",
    "MaxCapacityBuffer",
    "MetricIntervalLowerBound",
    "MetricIntervalUpperBound",
    "MinAdjustmentMagnitude",
    "MinCapacity",
    "ScaleInCooldown",
    "ScaleOutCooldown",
    "ScalingAdjustment",
    "SchedulingBufferTime",
    "TargetValue",
];

/// The members these types declare that the owning API models as BOOLEANS: whether scale in
/// is disabled, and the three suspended-scaling flags.
const BOOLEAN_MEMBERS: &[&str] = &[
    "DisableScaleIn",
    "DynamicScalingInSuspended",
    "DynamicScalingOutSuspended",
    "ScheduledScalingSuspended",
];

/// The number `text` names, read as the WHOLE number it is where it names one — a capacity is
/// an integer member, and `1` is not the same JSON value as `1.0` — and as a fractional one
/// otherwise. `None` for text that names no number at all.
fn as_number(text: &str) -> Option<Value> {
    let text = text.trim();
    if let Ok(whole) = text.parse::<i64>() {
        return Some(Value::Number(whole.into()));
    }
    ::serde_json::Number::from_f64(text.parse::<f64>().ok()?).map(Value::Number)
}

/// One declared value as the TYPE the owning API models that member as.
///
/// A template's scalar is text wherever the author quoted it or fed it through a `Ref` to a
/// `String` parameter — "you can write CloudFormation templates in JSON or YAML", and neither
/// spelling makes a quoted number a number — while the API's member is a number or a boolean.
/// So a member the model types as one is read from the text the declaration carries, here,
/// for every member of every structure these types declare: a value that is already of that
/// type travels as written, and one that is NO such value is left exactly as the template
/// wrote it, because the owning service's own validation is what judges it.
fn modelled(member: &str, value: &Value) -> Value {
    match value {
        Value::String(text) if NUMERIC_MEMBERS.contains(&member) => {
            as_number(text).unwrap_or_else(|| value.clone())
        }
        Value::String(text) if BOOLEAN_MEMBERS.contains(&member) => match text.trim() {
            flag if flag.eq_ignore_ascii_case("true") => Value::Bool(true),
            flag if flag.eq_ignore_ascii_case("false") => Value::Bool(false),
            _ => value.clone(),
        },
        Value::Object(members) => Value::Object(
            members
                .iter()
                .map(|(name, held)| (name.clone(), modelled(name, held)))
                .collect(),
        ),
        // A list's entries are values of the member the list belongs to, so each is read
        // under that same member name — which is what makes a `StepAdjustments` entry's own
        // members reachable.
        Value::Array(entries) => Value::Array(
            entries
                .iter()
                .map(|entry| modelled(member, entry))
                .collect(),
        ),
        _ => value.clone(),
    }
}

/// The refusal for a declaration that states none of a required property.
fn missing(resource_type: &str, property: &str) -> Refusal {
    Refusal {
        code: MISSING_PROPERTY.to_string(),
        message: format!("{resource_type} declares no {property}"),
    }
}

/// The refusal for a declared property this module does not apply, naming which property it
/// was: refusing is what keeps a resource from being reported as created — or as updated —
/// with less than its declaration asked for. `properties` is the document the deployment
/// DECLARES, which on an update is not the one the record carries.
fn refuse_unapplied_properties(
    properties: &Value,
    resource_type: &str,
    published: &[&str],
) -> Result<(), Refusal> {
    let Value::Object(properties) = properties else {
        return Ok(());
    };
    for property in properties.keys() {
        if published.contains(&property.as_str()) {
            continue;
        }
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!(
                "this emulator does not apply the {property} property of {resource_type}"
            ),
        });
    }
    Ok(())
}

/// Call one Application Auto Scaling operation on the awsJson1_1 wire its model declares.
async fn call_application_auto_scaling(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operation: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    call_json_rpc(
        ctx,
        APPLICATION_AUTO_SCALING_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{APPLICATION_AUTO_SCALING_TARGET_PREFIX}.{operation}"),
        request,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A resource record of one of these types, as the naming rules leave one.
    fn declared(resource_type: &str, properties: Value) -> StoredResource {
        StoredResource {
            logical_id: "Target".to_string(),
            resource_type: resource_type.to_string(),
            physical_resource_id: "Target".to_string(),
            attributes: BTreeMap::new(),
            metadata: None,
            properties,
            materialised: false,
            deletion_policy: None,
            update_replace_policy: None,
        }
    }

    /// RULE: every member these arms send is a published property of the type they send it
    /// for, so a declaration is carried out in the owning service's own vocabulary and
    /// nothing is invented for it.
    #[test]
    fn every_sent_member_is_a_published_property() {
        for member in TARGET_MEMBERS {
            assert!(
                TARGET_PROPERTIES.contains(member),
                "case {member:?}: expected the sent member to be a published property of \
                 {APPLICATION_AUTO_SCALING_SCALABLE_TARGET}, got none"
            );
        }
        for member in POLICY_MEMBERS {
            assert!(
                POLICY_PROPERTIES.contains(member),
                "case {member:?}: expected the sent member to be a published property of \
                 {APPLICATION_AUTO_SCALING_SCALING_POLICY}, got none"
            );
        }
        for property in TARGET_PROPERTIES {
            let placed = TARGET_MEMBERS.contains(property) || *property == SCHEDULED_ACTIONS;
            assert!(
                placed,
                "case {property:?}: expected the property to be a register member or the \
                 target's scheduled actions, got neither"
            );
        }
        for property in POLICY_PROPERTIES {
            let placed = POLICY_MEMBERS.contains(property) || *property == SCALING_TARGET_ID;
            assert!(
                placed,
                "case {property:?}: expected the property to be a put member or the target the \
                 policy is attached to, got neither"
            );
        }
    }

    /// RULE: a scalable target's identity is the composite of its resource id, its scalable
    /// dimension and its namespace, and a policy addressed by that composite reaches the same
    /// target as one addressed by the three members.
    #[test]
    fn a_policy_reaches_one_target_however_its_declaration_names_it() {
        let identity = serde_json::json!({
            "ResourceId": "table/orders",
            "ScalableDimension": "dynamodb:table:ReadCapacityUnits",
            "ServiceNamespace": "dynamodb",
        });
        let composite = "table/orders|dynamodb:table:ReadCapacityUnits|dynamodb";
        let target = declared(APPLICATION_AUTO_SCALING_SCALABLE_TARGET, identity.clone());
        let computed = scaling_target_id(
            &target_identity(&identity, &target).expect("the identity states all three members"),
        );
        assert_eq!(
            computed, composite,
            "case \"the composite a target is identified by\": expected {composite:?}, got \
             {computed:?}"
        );

        // (case, the policy's declaration, the identity the call carries)
        let rows: &[(&str, Value)] = &[
            (
                "a policy naming the three identity members",
                serde_json::json!({
                    "PolicyName": "track",
                    "ResourceId": "table/orders",
                    "ScalableDimension": "dynamodb:table:ReadCapacityUnits",
                    "ServiceNamespace": "dynamodb",
                }),
            ),
            (
                "a policy naming the target by its composite id",
                serde_json::json!({"PolicyName": "track", "ScalingTargetId": composite}),
            ),
        ];
        for (case, properties) in rows {
            let policy = declared(APPLICATION_AUTO_SCALING_SCALING_POLICY, properties.clone());
            let reached = policy_identity(properties, &policy)
                .unwrap_or_else(|refusal| panic!("case {case:?}: expected an identity, got {refusal:?}"));
            assert_eq!(
                Value::Object(reached.clone()),
                identity,
                "case {case:?}: expected the target's identity, got {reached:?}"
            );
        }

        // A composite that names no target is refused rather than sent as a call that
        // addresses nothing.
        for (case, broken) in [
            ("two fields", "table/orders|dynamodb"),
            ("an empty field", "table/orders||dynamodb"),
            ("four fields", "a|b|c|d"),
        ] {
            let properties = serde_json::json!({"PolicyName": "track", "ScalingTargetId": broken});
            let policy = declared(APPLICATION_AUTO_SCALING_SCALING_POLICY, properties.clone());
            assert!(
                policy_identity(&properties, &policy).is_err(),
                "case {case:?}: expected a composite that names no target to be refused, got an \
                 identity"
            );
        }
    }
}
