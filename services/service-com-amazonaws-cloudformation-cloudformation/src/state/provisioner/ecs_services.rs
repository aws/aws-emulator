// SPDX-License-Identifier: CC0-1.0

//! The per-type provisioning arms of the Amazon ECS service an `AWS::ECS::Service` declares.
//! One file of [`super`]'s per-type arms, dispatched to by its `Provisioner`, beside the
//! cluster arms the same service owns ([`super::application_services`]).
//!
//! ## Why the service is really created
//!
//! "Ref returns the Amazon Resource Name (ARN)" of the service, and a service is the handle
//! every Amazon ECS call about running work takes: `DescribeServices`, `ListTasks`,
//! `UpdateService`, an Application Auto Scaling scalable target's `ResourceId` and a
//! deployment's own circuit breaker each address one by its name or that ARN. A stack that
//! reported the resource created while nothing created the service publishes a handle that
//! resolves to nothing — `ListTasks` answers `ServiceNotFoundException` — so the workload a
//! template deploys is not running and nothing it was to serve is served.
//!
//! ## The properties
//!
//! The type's properties are `CreateService`'s own members in LOWER camel case
//! ([`ECS_SERVICE_MEMBERS`] names each pair, and [`lower_camel_members`] re-cases every
//! member name inside a value the same way), with three exceptions:
//!
//! * `ServiceName` is the name CloudFormation decided for the resource rather than a member
//!   travelling beside the others — it is the last segment of this resource's own ARN
//!   ([`declared_service_name`]);
//! * one property's documented name is not its member's at all: a service's
//!   `PlacementStrategies` sets `placementStrategy` ("The placement strategy objects to use
//!   for tasks in your service");
//! * `Tags` is rebuilt rather than carried, so the service carries the tags CloudFormation
//!   gives the resource beside the declared ones, as the `{key, value}` list Amazon ECS's own
//!   `Tag` shape spells.
//!
//! A declared property none of those members carries is REFUSED rather than dropped
//! ([`refuse_unapplied_properties`]): a service reported created with less than its
//! declaration asked for is the shallow success this arm exists to avoid.
//!
//! `UpdateService` "modifies the parameters of a service", and every property it takes a
//! member for is applied to the service the stack already has ([`update_ecs_service`]); a
//! property fixed when a service is made (its launch type, its scheduling strategy, the IAM
//! role a classic load balancer registers through) is not a member of that call at all, which
//! is why a declaration changing one replaces the resource instead.

use super::*;

/// The members `CreateService` takes this type's properties as, as (property, member) pairs.
///
/// Source: AWS's CloudFormation resource reference for the type, member by member against the
/// `CreateServiceRequest` shape
/// (`smithy_query(query="operation",operation="CreateService")`). A property these pairs do
/// not name is one that call has no member for; the service's `ServiceName` is set from the
/// resource's own identity instead, and its `tags` by [`tags_of`], which carries the stack's
/// propagated tags beside the declared ones.
const ECS_SERVICE_MEMBERS: &[(&str, &str)] = &[
    ("AvailabilityZoneRebalancing", "availabilityZoneRebalancing"),
    ("CapacityProviderStrategy", "capacityProviderStrategy"),
    ("Cluster", "cluster"),
    ("DeploymentConfiguration", "deploymentConfiguration"),
    ("DeploymentController", "deploymentController"),
    (DESIRED_COUNT_PROPERTY, DESIRED_COUNT_MEMBER),
    ("EnableECSManagedTags", "enableECSManagedTags"),
    ("EnableExecuteCommand", "enableExecuteCommand"),
    ("HealthCheckGracePeriodSeconds", "healthCheckGracePeriodSeconds"),
    ("LaunchType", "launchType"),
    ("LoadBalancers", "loadBalancers"),
    ("NetworkConfiguration", "networkConfiguration"),
    ("PlacementConstraints", "placementConstraints"),
    ("PlacementStrategies", "placementStrategy"),
    ("PlatformVersion", "platformVersion"),
    ("PropagateTags", "propagateTags"),
    ("Role", "role"),
    ("SchedulingStrategy", "schedulingStrategy"),
    ("ServiceConnectConfiguration", "serviceConnectConfiguration"),
    ("ServiceRegistries", "serviceRegistries"),
    ("TaskDefinition", "taskDefinition"),
    ("VolumeConfigurations", "volumeConfigurations"),
    ("VpcLatticeConfigurations", "vpcLatticeConfigurations"),
];

/// The members of that set `UpdateService` also takes, so a changed declaration of one is
/// applied to the service the stack already has rather than replacing it.
///
/// Source: `UpdateService`'s own member list
/// (`smithy_query(query="operation",operation="UpdateService")`) intersected with the pairs
/// above. What is left out is what that call has no member for — `launchType`, `role`,
/// `schedulingStrategy` and the `cluster` the service lives in, which addresses the call —
/// each of them a property AWS documents as requiring replacement.
const ECS_SERVICE_UPDATE_MEMBERS: &[&str] = &[
    "availabilityZoneRebalancing",
    "capacityProviderStrategy",
    "deploymentConfiguration",
    "deploymentController",
    "desiredCount",
    "enableECSManagedTags",
    "enableExecuteCommand",
    "healthCheckGracePeriodSeconds",
    "loadBalancers",
    "networkConfiguration",
    "placementConstraints",
    "placementStrategy",
    "platformVersion",
    "propagateTags",
    "serviceConnectConfiguration",
    "serviceRegistries",
    "taskDefinition",
    "volumeConfigurations",
    "vpcLatticeConfigurations",
];

/// The SCALAR properties a drift read reports back, each against the member
/// `Service` carries it in.
///
/// A property that is a LIST or a structure on the wire (the load balancers, the network
/// configuration, the placement rules) is left out, which is the position every arm whose
/// property is not a scalar is in and what [`drift`] documents. The role a service registers
/// through is reported as the ARN that service resolved it to (`Service$roleArn`).
const ECS_SERVICE_REPORTED_MEMBERS: &[(&str, &str)] = &[
    ("AvailabilityZoneRebalancing", "availabilityZoneRebalancing"),
    ("DesiredCount", "desiredCount"),
    ("EnableECSManagedTags", "enableECSManagedTags"),
    ("EnableExecuteCommand", "enableExecuteCommand"),
    ("HealthCheckGracePeriodSeconds", "healthCheckGracePeriodSeconds"),
    ("LaunchType", "launchType"),
    ("PlatformVersion", "platformVersion"),
    ("PropagateTags", "propagateTags"),
    ("Role", "roleArn"),
    ("SchedulingStrategy", "schedulingStrategy"),
    ("TaskDefinition", "taskDefinition"),
];

/// The property a template states the number of tasks the scheduler keeps in, and the member
/// `CreateService` takes it as.
const DESIRED_COUNT_PROPERTY: &str = "DesiredCount";
const DESIRED_COUNT_MEMBER: &str = "desiredCount";

/// The count a NEW service is created with where its declaration states none: "For new
/// services, if a desired count isn't specified, a default of 1 is used"
/// (`AWS::ECS::Service`'s own `DesiredCount`). The member is required of the API for a replica
/// service ("This is required if schedulingStrategy is REPLICA or isn't specified") while the
/// property is not required of the template, so this is the value CloudFormation supplies
/// rather than one the caller left out.
const DEFAULT_DESIRED_COUNT: u64 = 1;

/// The property naming the scheduling strategy, and the one value of it that keeps no desired
/// count at all: "when using this strategy, you don't need to specify a desired number of
/// tasks" (`SchedulingStrategy$DAEMON`), so a daemon service is created without the member.
const SCHEDULING_STRATEGY_PROPERTY: &str = "SchedulingStrategy";
const SCHEDULING_STRATEGY_DAEMON: &str = "DAEMON";

/// The property carrying the service's own tag set, merged with the ones CloudFormation
/// propagates ([`tags_of`]) and rebuilt rather than carried.
const TAGS_PROPERTY: &str = "Tags";

/// The property naming the cluster the service runs on, which ADDRESSES every call about the
/// service rather than travelling beside the others on an update or a delete.
const CLUSTER_PROPERTY: &str = "Cluster";

/// The property a template may name the service itself with, which is the name
/// CloudFormation decided for the resource.
const SERVICE_NAME_PROPERTY: &str = "ServiceName";

/// The member the create answers the service's own record under, and the one that record
/// carries the ARN Amazon ECS built for it in.
const SERVICE_MEMBER: &str = "service";
const SERVICE_ARN_MEMBER: &str = "serviceArn";

/// The segment an ECS service ARN carries the cluster and the service name after
/// (`arn:<partition>:ecs:<region>:<account>:service/<cluster>/<name>`).
const SERVICE_ARN_SEGMENT: &str = "service/";

/// The error codes Amazon ECS answers about a service, or the cluster that would hold one,
/// that it does not hold ("The specified service wasn't found", "The specified cluster wasn't
/// found") — which is what says a delete has nothing left to remove.
const SERVICE_NOT_FOUND: &str = "ServiceNotFoundException";

/// The attribute `AWS::ECS::Service` publishes through `Fn::GetAtt` ("Name: the name of the
/// Amazon ECS service"), and the member Amazon ECS reports it under.
const SERVICE_NAME_ATTRIBUTE: &str = "Name";
const SERVICE_NAME_MEMBER: &str = "serviceName";

/// Create the service an `AWS::ECS::Service` declares, in Amazon ECS itself — so a client
/// that read the stack's output finds the service in `DescribeServices`, its tasks in
/// `ListTasks`, and the task definition, the desired count and the load balancer the template
/// declared really in force on it.
///
/// `Err` is Amazon ECS's own refusal of the create, which is what the resource fails with: a
/// cluster it does not hold, a task definition it cannot resolve, a target group whose type
/// does not match the task definition's network mode, a desired count outside what the
/// scheduling strategy admits.
pub(super) async fn create_ecs_service(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unapplied_properties(&resource.properties, &resource.resource_type)?;
    let mut request = Map::new();
    if let Value::Object(properties) = &resource.properties {
        for (property, member) in ECS_SERVICE_MEMBERS {
            if let Some(value) = properties.get(*property).filter(|value| !value.is_null()) {
                request.insert((*member).to_string(), lower_camel_members(value, 0)?);
            }
        }
    }
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("tags".to_string(), tags_as_list(&tags, LOWER_CASED_TAG));
    }
    let name = declared_service_name(resource);
    request.insert(
        SERVICE_NAME_MEMBER.to_string(),
        Value::String(name.clone()),
    );
    // The count the type documents for a new service whose declaration states none
    // ([`DEFAULT_DESIRED_COUNT`]), which the API requires of a replica service.
    if !request.contains_key(DESIRED_COUNT_MEMBER) && !keeps_no_desired_count(resource) {
        request.insert(
            DESIRED_COUNT_MEMBER.to_string(),
            Value::from(DEFAULT_DESIRED_COUNT),
        );
    }
    let reported = call_json_rpc(
        ctx,
        ECS_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ECS_TARGET_PREFIX}.CreateService"),
        &Value::Object(request),
    )
    .await?;
    let created = reported.get(SERVICE_MEMBER);
    let mut attributes = BTreeMap::new();
    attributes.insert(
        SERVICE_NAME_ATTRIBUTE.to_string(),
        created
            .and_then(|service| reported_text(service, SERVICE_NAME_MEMBER))
            .unwrap_or(name),
    );
    // The ARN Amazon ECS built for the service is the identity a `Ref` resolves to and the
    // handle every call on it takes, so it replaces the one the naming rules computed rather
    // than being reported beside it.
    let identity = created.and_then(|service| reported_text(service, SERVICE_ARN_MEMBER));
    Ok(Created {
        physical_resource_id: identity,
        attributes,
    })
}

/// Apply a CHANGED declaration to the service the stack already has, through Amazon ECS's own
/// `UpdateService` — "modifies the parameters of a service".
///
/// Every member that call takes is sent as the declaration now states it
/// ([`ECS_SERVICE_UPDATE_MEMBERS`]), addressed by the service's own name inside the cluster it
/// runs on: so a changed task definition, desired count or network configuration is really in
/// force on the service the stack reports, and no change is reported as applied that this
/// call could not carry.
pub(super) async fn update_ecs_service(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Created, Refusal> {
    refuse_unapplied_properties(declared, &resource.resource_type)?;
    let mut request = Map::new();
    if let Value::Object(properties) = declared {
        for (property, member) in ECS_SERVICE_MEMBERS {
            if !ECS_SERVICE_UPDATE_MEMBERS.contains(member) {
                continue;
            }
            if let Some(value) = properties.get(*property).filter(|value| !value.is_null()) {
                request.insert((*member).to_string(), lower_camel_members(value, 0)?);
            }
        }
    }
    let name = declared_service_name(resource);
    request.insert(SERVICE_MEMBER.to_string(), Value::String(name.clone()));
    if let Some(cluster) = declared_cluster(resource, declared) {
        request.insert("cluster".to_string(), Value::String(cluster));
    }
    let reported = call_json_rpc(
        ctx,
        ECS_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ECS_TARGET_PREFIX}.UpdateService"),
        &Value::Object(request),
    )
    .await?;
    let mut attributes = BTreeMap::new();
    attributes.insert(
        SERVICE_NAME_ATTRIBUTE.to_string(),
        reported
            .get(SERVICE_MEMBER)
            .and_then(|service| reported_text(service, SERVICE_NAME_MEMBER))
            .unwrap_or(name),
    );
    Ok(Created::attributes(attributes))
}

/// Delete the service a stack created, by its own name inside the cluster it runs on.
///
/// `force` is sent because the service a stack deletes is the one it deployed, tasks and all:
/// "You can't delete a service that is actively maintaining tasks unless you specify the
/// force option" (`DeleteService`) — the alternative being to scale the service to no tasks
/// first, which is the same outcome for a resource whose record is going away. A service
/// Amazon ECS no longer holds (`ServiceNotFoundException`, judged on the CODE and never on a
/// message that carries the request's own values), and a cluster it no longer holds, are each
/// already in the state the delete asks for.
pub(super) async fn delete_ecs_service(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let mut request = Map::new();
    request.insert(
        SERVICE_MEMBER.to_string(),
        Value::String(declared_service_name(resource)),
    );
    if let Some(cluster) = declared_cluster(resource, &resource.properties) {
        request.insert("cluster".to_string(), Value::String(cluster));
    }
    request.insert("force".to_string(), Value::Bool(true));
    match call_json_rpc(
        ctx,
        ECS_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ECS_TARGET_PREFIX}.DeleteService"),
        &Value::Object(request),
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal)
            if refusal.code == SERVICE_NOT_FOUND || refusal.code == CLUSTER_NOT_FOUND =>
        {
            Ok(())
        }
        Err(refusal) => Err(refusal),
    }
}

/// The service's configuration as Amazon ECS reports it, for drift detection: its own
/// `DescribeServices` entry, projected onto the property names the type declares
/// ([`ECS_SERVICE_REPORTED_MEMBERS`]).
///
/// A service that read reports none of, one it reports `INACTIVE` — the status
/// `DeleteService` documents a deleted service keeps — and a cluster it no longer holds each
/// say the resource was deleted, which is what `None` reports.
pub(super) async fn describe_ecs_service(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    /// The status Amazon ECS reports for a service whose deletion has gone through.
    const INACTIVE: &str = "INACTIVE";

    let mut request = Map::new();
    request.insert(
        "services".to_string(),
        Value::Array(vec![Value::String(declared_service_name(resource))]),
    );
    if let Some(cluster) = declared_cluster(resource, &resource.properties) {
        request.insert("cluster".to_string(), Value::String(cluster));
    }
    let answered = match call_json_rpc(
        ctx,
        ECS_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ECS_TARGET_PREFIX}.DescribeServices"),
        &Value::Object(request),
    )
    .await
    {
        Ok(answered) => answered,
        Err(refusal) if refusal.code == CLUSTER_NOT_FOUND => return Ok(None),
        Err(refusal) => return Err(refusal),
    };
    let Some(reported) = answered
        .get("services")
        .and_then(Value::as_array)
        .and_then(|services| services.first())
    else {
        return Ok(None);
    };
    if reported_text(reported, "status").as_deref() == Some(INACTIVE) {
        return Ok(None);
    }
    Ok(Some(reported_as_properties(
        reported,
        ECS_SERVICE_REPORTED_MEMBERS,
    )))
}

/// The refusal for a declared property this module does not apply, naming which property it
/// was: refusing is what keeps a service from being reported created with less than its
/// declaration asked for.
///
/// What the arm applies is the pair table ([`ECS_SERVICE_MEMBERS`]) plus the two properties it
/// handles itself — the `ServiceName` the resource is created under and the `Tags` it rebuilds
/// — so a property outside that set is one no call of this arm carries.
fn refuse_unapplied_properties(properties: &Value, resource_type: &str) -> Result<(), Refusal> {
    let Value::Object(members) = properties else {
        return Ok(());
    };
    for property in members.keys() {
        let applied = ECS_SERVICE_MEMBERS
            .iter()
            .any(|(declared, _)| declared == property)
            || [SERVICE_NAME_PROPERTY, TAGS_PROPERTY].contains(&property.as_str());
        if applied {
            continue;
        }
        return Err(Refusal {
            code: "NotImplemented".to_string(),
            message: format!(
                "this emulator does not apply the {property} property of {resource_type}"
            ),
        });
    }
    Ok(())
}

/// Whether the declaration asks for a service that keeps no desired count: the DAEMON
/// scheduling strategy, which "deploys exactly one task on each active container instance"
/// and needs no desired number of tasks at all.
fn keeps_no_desired_count(resource: &StoredResource) -> bool {
    property(resource, SCHEDULING_STRATEGY_PROPERTY)
        .is_some_and(|strategy| strategy == SCHEDULING_STRATEGY_DAEMON)
}

/// The name the service is created under: the one the declaration states, or the last segment
/// of the identifier CloudFormation computed for the resource — which is that identifier's
/// own `service/<cluster>/<name>` path ([`SERVICE_ARN_SEGMENT`]) — since "Ref returns the
/// Amazon Resource Name (ARN)" for this type rather than the name.
fn declared_service_name(resource: &StoredResource) -> String {
    if let Some(declared) = property(resource, SERVICE_NAME_PROPERTY) {
        return declared;
    }
    let identifier = &resource.physical_resource_id;
    let Some((_, path)) = identifier.split_once(SERVICE_ARN_SEGMENT) else {
        return identifier.clone();
    };
    match path.rsplit_once('/') {
        Some((_, name)) => name.to_string(),
        None => path.to_string(),
    }
}

/// The cluster a call about the service is addressed in: the one `declared` names, as a short
/// name or a full ARN — both of which `CreateService$cluster` admits — or `None` for a
/// declaration that names none, which is the `default` cluster that member documents.
fn declared_cluster(resource: &StoredResource, declared: &Value) -> Option<String> {
    if let Some(cluster) = declared
        .get(CLUSTER_PROPERTY)
        .and_then(scalar_text)
        .filter(|cluster| !cluster.is_empty())
    {
        return Some(cluster);
    }
    property(resource, CLUSTER_PROPERTY)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every member the update sends is one the create sends too, so one declaration cannot
    /// mean two different things on the two calls.
    #[test]
    fn every_updated_member_is_a_created_member() {
        for member in ECS_SERVICE_UPDATE_MEMBERS {
            assert!(
                ECS_SERVICE_MEMBERS
                    .iter()
                    .any(|(_, created)| created == member),
                "case {member:?}: expected the updated member to be one the create sends, \
                 got none"
            );
        }
    }

    /// A property outside the set this arm applies is refused rather than dropped, and every
    /// property of that set is accepted.
    #[test]
    fn a_property_the_calls_do_not_carry_is_refused() {
        let accepted = serde_json::json!({
            "Cluster": "a-cluster",
            "TaskDefinition": "a-family:1",
            "DesiredCount": 2,
            "ServiceName": "a-service",
            "Tags": [{ "Key": "Tier", "Value": "gold" }],
        });
        assert!(
            refuse_unapplied_properties(&accepted, template::ECS_SERVICE).is_ok(),
            "case \"the properties this arm applies\": expected them to be admitted, got a \
             refusal"
        );
        let refused = serde_json::json!({ "Cluster": "a-cluster", "NotAProperty": true });
        let answer = refuse_unapplied_properties(&refused, template::ECS_SERVICE);
        assert!(
            answer.is_err(),
            "case \"a property the type does not publish\": expected a refusal, got {answer:?}"
        );
    }

    /// The name a call is addressed by is the declared one where the template states it, and
    /// otherwise the name segment of the identifier CloudFormation computed.
    #[test]
    fn the_service_name_is_the_declared_one_or_the_identifiers_own_segment() {
        // (case, the declared properties, the computed identifier, the name)
        let rows: &[(&str, Value, &str, &str)] = &[
            (
                "a declared name",
                serde_json::json!({ "ServiceName": "orders" }),
                "arn:aws:ecs:us-east-1:000000000000:service/a-cluster/ignored",
                "orders",
            ),
            (
                "no declared name: the identifier's own segment",
                serde_json::json!({}),
                "arn:aws:ecs:us-east-1:000000000000:service/a-cluster/stack-Svc-ABC123",
                "stack-Svc-ABC123",
            ),
            (
                "an identifier that is no service ARN at all",
                serde_json::json!({}),
                "stack-Svc-ABC123",
                "stack-Svc-ABC123",
            ),
        ];
        for (case, properties, identifier, expected) in rows {
            let resource = StoredResource {
                logical_id: "Svc".to_string(),
                resource_type: template::ECS_SERVICE.to_string(),
                physical_resource_id: (*identifier).to_string(),
                attributes: BTreeMap::new(),
                metadata: None,
                properties: properties.clone(),
                materialised: false,
                deletion_policy: None,
                update_replace_policy: None,
            };
            let name = declared_service_name(&resource);
            assert_eq!(
                name, *expected,
                "case {case:?}: expected the service to be addressed as {expected:?}, got \
                 {name:?}"
            );
        }
    }
}
