// SPDX-License-Identifier: CC0-1.0

//! The per-type provisioning arms of the network and delivery services: the Amazon EC2
//! security-group, route-table and gateway types, Amazon EventBridge, AWS Systems Manager,
//! AWS Step Functions, Amazon Cognito, Amazon S3 Vectors and Amazon EventBridge Scheduler.
//! One file of [`super`]'s per-type arms, dispatched to by its `Provisioner`.

use super::*;
// The arms of the certificate, DNS, compute and developer-tool types this group holds, split
// out so each file stays readable; re-exported, so this module is the one path to them.
mod compute_and_developer_tools;
mod certificates_and_dns;
pub(crate) use certificates_and_dns::*;
pub(crate) use compute_and_developer_tools::*;

/// Create the security group an `AWS::EC2::SecurityGroup` declares, in Amazon EC2, and
/// authorize the inline rules its declaration asks for.
///
/// "Ref returns the ID of the security group" — an id EC2 ASSIGNS (`sg-` and 17 hexadecimal
/// characters), which is what this answers as the resource's physical id, so the stack's
/// `Ref`, its outputs and its per-resource reads report an id EC2 really holds a group for
/// and every service that takes a security group id accepts it.
///
/// The rules are not members of the create: "when you create a security group, it has no
/// inbound rules", and each entry of either rule property is one permission of
/// `AuthorizeSecurityGroupIngress` / `AuthorizeSecurityGroupEgress`. A new group does carry
/// one OUTBOUND rule of EC2's own — the one that allows all outbound traffic — and a
/// declaration that states outbound rules of its own REPLACES it: CloudFormation removes
/// that default rule, so the group ends up carrying exactly the rules the template declared
/// in each direction. The revoke comes FIRST, because EC2 refuses a second rule identical to
/// one the group already holds (`InvalidPermission.Duplicate`), which a declaration
/// restating the all-traffic rule would otherwise run into.
pub(super) async fn create_ec2_security_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut parameters = query_members(
        &resource.properties,
        EC2_SECURITY_GROUP_CREATE_MEMBERS,
        &[],
        EC2_SECURITY_GROUP_PROPERTIES_NOT_SENT,
        EC2_SECURITY_GROUP,
    )?;
    // "The name of the security group" (`CreateSecurityGroupRequest$GroupName`) is required
    // by that call while the type's own `GroupName` is optional — "if you don't specify a
    // group name, CloudFormation generates a unique physical ID and uses that ID for the
    // group name" — so the group is created under the name the stack gave the resource.
    parameters.push(("GroupName".to_string(), declared_name(resource)));
    let answered = call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "CreateSecurityGroup",
        &parameters,
    )
    .await?;
    let group_id = xml_element(&answered, "groupId")
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: "CreateSecurityGroup answered no groupId".to_string(),
        })?;
    for (property, members, action, is_egress) in [
        (
            "SecurityGroupIngress",
            EC2_SECURITY_GROUP_INGRESS_MEMBERS,
            "AuthorizeSecurityGroupIngress",
            false,
        ),
        (
            "SecurityGroupEgress",
            EC2_SECURITY_GROUP_EGRESS_MEMBERS,
            "AuthorizeSecurityGroupEgress",
            true,
        ),
    ] {
        let declared = declared_security_group_rules(&resource.properties, property);
        if declared.is_empty() {
            continue;
        }
        // A declaration that states OUTBOUND rules replaces the one EC2 gives a new group:
        // the all-traffic rule is removed, so what the group carries outbound is what the
        // template declared and nothing else. A group EC2 no longer holds that rule for is
        // already in the state this asks for, which is what its own not-found answer says.
        if is_egress {
            let mut parameters = vec![("GroupId".to_string(), group_id.clone())];
            parameters.extend(allow_all_egress_parameters());
            match call_query(
                ctx,
                EC2_SIGNING_NAME,
                region,
                EC2_API_VERSION,
                "RevokeSecurityGroupEgress",
                &parameters,
            )
            .await
            {
                Ok(_) => {}
                Err(refusal) if refusal.code == INVALID_PERMISSION_NOT_FOUND => {}
                Err(refusal) => return Err(refusal),
            }
        }
        for rule in declared {
            let mut parameters = vec![("GroupId".to_string(), group_id.clone())];
            parameters.extend(security_group_rule_parameters(rule, members)?);
            call_query(
                ctx,
                EC2_SIGNING_NAME,
                region,
                EC2_API_VERSION,
                action,
                &parameters,
            )
            .await?;
        }
    }
    tag_ec2_resource(
        ctx,
        region,
        &group_id,
        &tags_of(&resource.properties, propagated),
    )
    .await?;
    // The attributes the type publishes: the id EC2 assigned (which this type's `GroupId`
    // return value is) and the VPC the group was created in, which is the one the
    // declaration named — re-pointed at the id EC2 minted for it before this create was
    // made, so the attribute carries a VPC id EC2 knows. A declaration naming none leaves
    // the group in the Region's default VPC, whose id only EC2 holds, so none is reported
    // rather than invented. The ARN is the one EC2 built for the group, under both names the
    // uniform convention reads a resource's ARN off.
    let mut attributes = BTreeMap::new();
    attributes.insert("GroupId".to_string(), group_id.clone());
    if let Some(vpc_id) = resource
        .properties
        .get("VpcId")
        .and_then(Value::as_str)
        .filter(|vpc_id| !vpc_id.is_empty())
    {
        attributes.insert("VpcId".to_string(), vpc_id.to_string());
    }
    if let Some(arn) = xml_element(&answered, "securityGroupArn").filter(|arn| !arn.is_empty()) {
        attributes.extend(super::template::arn_attributes(
            &resource.resource_type,
            &arn,
        ));
    }
    Ok(Created {
        physical_resource_id: Some(group_id),
        attributes,
    })
}

/// Delete the security group a stack created, addressed by the id EC2 assigned it. A group
/// EC2 no longer holds (`InvalidGroup.NotFound`) is already in the state the delete asks
/// for; one another resource still references is that service's own `DependencyViolation`,
/// which the stack reports as the failure it is.
pub(super) async fn delete_ec2_security_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    group_id: &str,
) -> Result<(), Refusal> {
    let parameters = vec![("GroupId".to_string(), group_id.to_string())];
    match call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "DeleteSecurityGroup",
        &parameters,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == INVALID_GROUP_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The security group's configuration as Amazon EC2 reports it, for drift detection. A group
/// it no longer holds (`InvalidGroup.NotFound`) was deleted.
pub(super) async fn describe_ec2_security_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    group_id: &str,
) -> Result<Option<Value>, Refusal> {
    let parameters = vec![("GroupId.1".to_string(), group_id.to_string())];
    match call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "DescribeSecurityGroups",
        &parameters,
    )
    .await
    {
        Ok(answered) => Ok(Some(Value::Object(ec2_reported_entry(
            &answered,
            "securityGroupInfo",
        )))),
        Err(refusal) if refusal.code == INVALID_GROUP_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The `AWS::EC2::Volume` properties `CreateVolume` takes under the SAME member name, each a
/// scalar for the reason [`EC2_VPC_CREATE_MEMBERS`] records.
///
/// Source: AWS's CloudFormation resource reference for the type against
/// `smithy_query(query="operation",operation="CreateVolume")` — every property of the type
/// but the two below is a member of that call spelled identically. EC2 takes no name for a
/// volume and assigns the `vol-…` id itself, which is why this type's identity is the owning
/// service's ([`super::template::identity_is_service_assigned`]).
pub(super) const EC2_VOLUME_CREATE_MEMBERS: &[(&str, QueryMember)] = &[
    ("AvailabilityZone", QueryMember::Scalar),
    ("Encrypted", QueryMember::Scalar),
    ("Iops", QueryMember::Scalar),
    ("KmsKeyId", QueryMember::Scalar),
    ("MultiAttachEnabled", QueryMember::Scalar),
    ("OutpostArn", QueryMember::Scalar),
    ("Size", QueryMember::Scalar),
    ("SnapshotId", QueryMember::Scalar),
    ("Throughput", QueryMember::Scalar),
    ("VolumeInitializationRate", QueryMember::Scalar),
    ("VolumeType", QueryMember::Scalar),
];

/// The `AWS::EC2::Volume` properties another call carries: `AutoEnableIO`, which is a volume
/// ATTRIBUTE rather than a member of the create ([`MODIFY_VOLUME_ATTRIBUTE`]), and the tags,
/// which reach EC2 through the one call that tags a resource that exists
/// ([`tag_ec2_resource`]).
pub(super) const EC2_VOLUME_PROPERTIES_NOT_SENT: &[&str] = &["AutoEnableIO", "Tags"];

/// The one attribute of a volume a declaration can state, as the `ModifyVolumeAttribute`
/// member that writes it — named identically on both sides, and an `AttributeBooleanValue`.
pub(super) const EC2_VOLUME_ATTRIBUTES: &[&str] = &["AutoEnableIO"];

/// The `Modify…Attribute` call that writes a volume's one attribute.
pub(super) const MODIFY_VOLUME_ATTRIBUTE: Ec2AttributeCall = Ec2AttributeCall {
    action: "ModifyVolumeAttribute",
    id_member: "VolumeId",
};

/// Amazon EC2's error code for a volume it does not hold (`InvalidVolume.NotFound`, EC2 API
/// error codes), which is a delete asking for a state that service is already in.
pub(super) const INVALID_VOLUME_NOT_FOUND: &str = "InvalidVolume.NotFound";

/// The state Amazon EC2 reports a volume it has created and not finished creating in, and
/// the one it reports a usable volume in (`Volume$State`).
pub(super) const VOLUME_STATE_CREATING: &str = "creating";
pub(super) const VOLUME_STATE_AVAILABLE: &str = "available";
pub(super) const VOLUME_STATE_IN_USE: &str = "in-use";

/// Create the Amazon EBS volume an `AWS::EC2::Volume` declares, in Amazon EC2.
///
/// "Ref returns the volume ID" — an id EC2 ASSIGNS (`vol-` and 17 hexadecimal characters),
/// which is what this answers as the resource's physical id, so the stack's `Ref`, its
/// outputs and its per-resource reads report an id EC2 really holds a volume for and every
/// call that takes a volume id accepts it.
pub(super) async fn create_ec2_volume(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let parameters = query_members(
        &resource.properties,
        EC2_VOLUME_CREATE_MEMBERS,
        &[],
        EC2_VOLUME_PROPERTIES_NOT_SENT,
        EC2_VOLUME,
    )?;
    let answered = call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "CreateVolume",
        &parameters,
    )
    .await?;
    // `CreateVolume`'s answer IS the volume (its output shape is `Volume`), so the id it
    // assigned travels as the answer's own `volumeId` element rather than inside a wrapper
    // of its own.
    let volume_id = xml_element(&answered, "volumeId")
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: "CreateVolume answered no volumeId".to_string(),
        })?;
    for attribute in EC2_VOLUME_ATTRIBUTES {
        if let Some(value) = resource.properties.get(*attribute) {
            modify_ec2_attribute(
                ctx,
                region,
                MODIFY_VOLUME_ATTRIBUTE,
                &volume_id,
                attribute,
                value,
                true,
            )
            .await?;
        }
    }
    tag_ec2_resource(
        ctx,
        region,
        &volume_id,
        &tags_of(&resource.properties, propagated),
    )
    .await?;
    // The one attribute this type publishes is the id EC2 assigned.
    let mut attributes = BTreeMap::new();
    attributes.insert("VolumeId".to_string(), volume_id.clone());
    Ok(Created {
        physical_resource_id: Some(volume_id),
        attributes,
    })
}

/// Whether the volume a stack created is usable yet: `creating` is the state EC2 answers the
/// create in and the one a volume is still coming up in, `available` is a volume nothing has
/// attached and `in-use` one something has, and every other state
/// (`deleting`/`deleted`/`error`) is one no wait brings a new volume out of. A volume EC2 no
/// longer holds is in the same position: nothing is going to create it.
pub(super) async fn volume_readiness(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    volume_id: &str,
) -> Result<Readiness, Refusal> {
    let parameters = vec![("VolumeId.1".to_string(), volume_id.to_string())];
    let answered = match call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "DescribeVolumes",
        &parameters,
    )
    .await
    {
        Ok(answered) => answered,
        Err(refusal) if refusal.code == INVALID_VOLUME_NOT_FOUND => {
            return Ok(Readiness::Failed(format!(
                "Amazon EC2 holds no volume for {volume_id}"
            )))
        }
        Err(refusal) => return Err(refusal),
    };
    let reported = ec2_reported_entry(&answered, "volumeSet");
    // The state travels under the answer's own `status` element, which `ec2_reported` reads
    // as `Status` — `Volume$State`'s `@xmlName`.
    let state = reported
        .get("Status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match state {
        VOLUME_STATE_AVAILABLE | VOLUME_STATE_IN_USE => Ok(Readiness::Ready),
        VOLUME_STATE_CREATING => Ok(Readiness::Pending(VOLUME_STATE_CREATING.to_string())),
        other => Ok(Readiness::Failed(format!(
            "Amazon EC2 reports volume {volume_id} in state {other}"
        ))),
    }
}

/// Delete the volume a stack created, addressed by the id EC2 assigned it. A volume EC2 no
/// longer holds (`InvalidVolume.NotFound`) is already in the state the delete asks for; one
/// still attached to an instance is that service's own refusal, which the stack reports as
/// the failure it is.
pub(super) async fn delete_ec2_volume(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    volume_id: &str,
) -> Result<(), Refusal> {
    let parameters = vec![("VolumeId".to_string(), volume_id.to_string())];
    match call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "DeleteVolume",
        &parameters,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == INVALID_VOLUME_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The volume's configuration as Amazon EC2 reports it, for drift detection. A volume it no
/// longer holds (`InvalidVolume.NotFound`) was deleted.
pub(super) async fn describe_ec2_volume(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    volume_id: &str,
) -> Result<Option<Value>, Refusal> {
    let parameters = vec![("VolumeId.1".to_string(), volume_id.to_string())];
    match call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "DescribeVolumes",
        &parameters,
    )
    .await
    {
        Ok(answered) => Ok(Some(Value::Object(ec2_reported_entry(
            &answered, "volumeSet",
        )))),
        Err(refusal) if refusal.code == INVALID_VOLUME_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

// ---------------------------------------------------------------------------
// Amazon EventBridge: AWS::Events::Rule
// ---------------------------------------------------------------------------

/// Amazon EventBridge's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: events`, service shape `AWSEvents`), which speaks `awsJson1_1`.
pub(super) const EVENTS_SIGNING_NAME: &str = "events";
pub(super) const EVENTS_TARGET_PREFIX: &str = "AWSEvents";

/// The error code CloudFormation's own resource handlers answer for a create whose
/// identifier is already in use (`AlreadyExists`), for a type whose owning API takes the
/// create as an UPSERT and so refuses nothing itself.
pub(super) const ALREADY_EXISTS: &str = "AlreadyExists";

/// The `AWS::Events::Rule` properties `PutRule` takes under the SAME member name.
///
/// Source: AWS's CloudFormation resource reference for the type against
/// `smithy_query(operation="PutRule")` — `Description`, `EventBusName`, `RoleArn`,
/// `ScheduleExpression` and `State` are spelled identically on both sides. The three
/// properties NOT here are the three that are not passed straight through: `Name`, which
/// CloudFormation decides and which is this resource's physical id, `EventPattern`, which
/// the type declares as a template DOCUMENT while `PutRule` takes the JSON TEXT of one,
/// and `Targets`, which is not a `PutRule` member at all — a rule's targets are installed
/// by `PutTargets`, the same second call a client makes.
pub(super) const EVENTS_RULE_MEMBERS: &[&str] = &[
    "Description",
    "EventBusName",
    "RoleArn",
    "ScheduleExpression",
    "State",
];

/// The `Target` property type's members, named identically to `Target`'s own
/// (`smithy_query(shape="Target")`): a target's id, the ARN it invokes, the role it
/// assumes, the input it carries and the per-service parameter structures. The one member
/// absent here is `EcsParameters`, whose two renamed members [`ecs_parameters`] maps.
pub(super) const EVENTS_TARGET_MEMBERS: &[&str] = &[
    "AppSyncParameters",
    "Arn",
    "BatchParameters",
    "DeadLetterConfig",
    "HttpParameters",
    "Id",
    "Input",
    "InputPath",
    "InputTransformer",
    "KinesisParameters",
    "RedshiftDataParameters",
    "RetryPolicy",
    "RoleArn",
    "RunCommandParameters",
    "SageMakerPipelineParameters",
    "SqsParameters",
];

/// The `EcsParameters` members named identically on both sides
/// (`smithy_query(shape="EcsParameters")`). The two that are NOT are mapped by
/// [`ecs_parameters`].
pub(super) const EVENTS_ECS_PARAMETER_MEMBERS: &[&str] = &[
    "CapacityProviderStrategy",
    "EnableECSManagedTags",
    "EnableExecuteCommand",
    "Group",
    "LaunchType",
    "PlacementConstraints",
    "PlacementStrategy",
    "PlatformVersion",
    "PropagateTags",
    "ReferenceId",
    "TaskCount",
    "TaskDefinitionArn",
];

/// Create — or update — the rule an `AWS::Events::Rule` declares, so a client reading the
/// stack's outputs and then calling `events DescribeRule` with that name finds the rule
/// there rather than a `ResourceNotFoundException`.
///
/// `Ref` of this type returns the rule NAME, which is this resource's physical id, so the
/// rule is created under exactly the name the stack reports for it; the ARN `PutRule`
/// answers with is the resource's real identity and replaces the computed one.
///
/// The declared `Targets` are installed by a `PutTargets` on the created rule — the same
/// pair of calls a client makes, since `PutRule` takes no targets.
///
/// `PutRule` is an UPSERT ("creates or updates the specified rule"), so the name is probed
/// first: a rule the bus ALREADY holds is not this stack's to reconfigure, and a create
/// that put the declaration over it would report a resource the stack took over rather than
/// created. That is the refusal CloudFormation itself answers for a resource whose
/// identifier is already in use, and it is the same thing the Systems Manager arm gets for
/// free from `PutParameter` refusing a put without `Overwrite`.
///
/// A rule whose TARGETS could not be installed is not a created resource either, so the
/// rule this call just made is removed before that refusal is reported: nothing is left
/// behind under a name the stack does not record.
pub(super) async fn put_rule(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    if describe_rule(ctx, region, resource).await?.is_some() {
        return Err(Refusal {
            code: ALREADY_EXISTS.to_string(),
            message: format!(
                "Resource of type '{}' with identifier '{}' already exists.",
                resource.resource_type, resource.physical_resource_id
            ),
        });
    }
    let reported = put_rule_configuration(
        ctx,
        region,
        &resource.physical_resource_id,
        &resource.properties,
    )
    .await?;
    if let Err(refusal) = install_targets(
        ctx,
        region,
        &resource.physical_resource_id,
        &resource.properties,
    )
    .await
    {
        // Best effort: the refusal that stopped the create is what the stack reports, so a
        // failure to undo it must not displace that reason.
        let _ = delete_rule(ctx, region, resource).await;
        return Err(refusal);
    }
    Ok(reported)
}

/// Apply a CHANGED `AWS::Events::Rule` declaration to the rule the stack already has.
///
/// `PutRule` is "Creates or updates the specified rule", so the declared configuration is
/// put under the same name, and the declared target list is reconciled against the one the
/// rule carries: a target the declaration no longer names is removed, and the ones it
/// names are put (`PutTargets` "adds the specified targets to the specified rule, or
/// updates the targets if they are already associated with the rule").
///
/// A declaration that moves the rule to a DIFFERENT event bus is not an in-place change:
/// a rule belongs to one bus, so nothing here can carry it across and the deployment
/// reports the change as not applied (`Ok(None)`) rather than creating a second rule and
/// leaving the first behind.
pub(super) async fn update_rule(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    if declared_bus(declared) != declared_bus(&resource.properties) {
        return Ok(None);
    }
    let name = &resource.physical_resource_id;
    let reported = put_rule_configuration(ctx, region, name, declared).await?;
    let installed = installed_target_ids(ctx, region, name, declared).await?;
    let retained: Vec<String> = declared_targets(declared)
        .iter()
        .filter_map(|target| target.get("Id").and_then(Value::as_str))
        .map(ToString::to_string)
        .collect();
    let removed: Vec<String> = installed
        .into_iter()
        .filter(|id| !retained.contains(id))
        .collect();
    remove_targets(ctx, region, name, declared, &removed).await?;
    install_targets(ctx, region, name, declared).await?;
    Ok(Some(reported))
}

/// `PutRule` with `properties` as the rule's configuration and `name` as the one member
/// CloudFormation decides — the call behind both this type's create and its update, so the
/// two cannot send a different request for the same declaration.
///
/// No tag is sent: the type publishes no `Tags` property, so CloudFormation propagates
/// none to a rule, exactly as it propagates none to an `AWS::WAFRegional::IPSet`.
pub(super) async fn put_rule_configuration(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
    properties: &Value,
) -> Result<Created, Refusal> {
    let mut request = Map::new();
    if let Some(declared) = properties.as_object() {
        for member in EVENTS_RULE_MEMBERS {
            if let Some(value) = declared.get(*member) {
                request.insert((*member).to_string(), value.clone());
            }
        }
    }
    // The type declares `EventPattern` as a document ("Json"), while `PutRule` takes the
    // JSON text of one — the same spelling rule a policy document reaches IAM under.
    if let Some(pattern) = properties.get("EventPattern").and_then(document_text) {
        request.insert("EventPattern".to_string(), Value::String(pattern));
    }
    request.insert("Name".to_string(), Value::String(name.to_string()));
    let reported = call_events(ctx, region, "PutRule", &Value::Object(request)).await?;
    let mut attributes = BTreeMap::new();
    if let Some(arn) = reported.get("RuleArn").and_then(Value::as_str) {
        attributes.insert("Arn".to_string(), arn.to_string());
    }
    Ok(Created::attributes(attributes))
}

/// Install the targets a declaration names on the rule `name`, through EventBridge's own
/// `PutTargets`. A declaration naming none asks for no call at all, since `Targets` is a
/// `@length(min: 1)` list of that request.
///
/// `PutTargets` reports per-target failures in its answer rather than refusing the call,
/// so a reported failed entry is read as the refusal it is: the resource failed, and the
/// stack carries the code and message EventBridge gave for that target.
pub(super) async fn install_targets(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
    properties: &Value,
) -> Result<(), Refusal> {
    let declared = declared_targets(properties);
    if declared.is_empty() {
        return Ok(());
    }
    let targets: Vec<Value> = declared.iter().map(event_target).collect();
    let mut request = Map::new();
    request.insert("Rule".to_string(), Value::String(name.to_string()));
    request.insert("Targets".to_string(), Value::Array(targets));
    if let Some(bus) = declared_bus(properties) {
        request.insert("EventBusName".to_string(), Value::String(bus));
    }
    let reported = call_events(ctx, region, "PutTargets", &Value::Object(request)).await?;
    match first_listed(&reported, "FailedEntries") {
        Some(failed) => Err(Refusal {
            code: failed
                .get("ErrorCode")
                .and_then(Value::as_str)
                .unwrap_or(UNIDENTIFIED_FAILURE)
                .to_string(),
            message: failed
                .get("ErrorMessage")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        }),
        None => Ok(()),
    }
}

/// Remove the targets `ids` names from the rule `name`. An empty set asks for no call:
/// `RemoveTargets$Ids` is a `@length(min: 1)` list.
pub(super) async fn remove_targets(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
    properties: &Value,
    ids: &[String],
) -> Result<(), Refusal> {
    if ids.is_empty() {
        return Ok(());
    }
    let mut request = Map::new();
    request.insert("Rule".to_string(), Value::String(name.to_string()));
    request.insert(
        "Ids".to_string(),
        Value::Array(ids.iter().map(|id| Value::String(id.clone())).collect()),
    );
    if let Some(bus) = declared_bus(properties) {
        request.insert("EventBusName".to_string(), Value::String(bus));
    }
    let reported = call_events(ctx, region, "RemoveTargets", &Value::Object(request)).await?;
    match first_listed(&reported, "FailedEntries") {
        Some(failed) => Err(Refusal {
            code: failed
                .get("ErrorCode")
                .and_then(Value::as_str)
                .unwrap_or(UNIDENTIFIED_FAILURE)
                .to_string(),
            message: failed
                .get("ErrorMessage")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        }),
        None => Ok(()),
    }
}

/// The ids of the targets the rule `name` CARRIES, as EventBridge reports them.
///
/// One page is the whole set: a rule holds at most as many targets as `Target`'s own list
/// bound admits, which is inside `ListTargetsByRule`'s maximum page.
pub(super) async fn installed_target_ids(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
    properties: &Value,
) -> Result<Vec<String>, Refusal> {
    let mut request = Map::new();
    request.insert("Rule".to_string(), Value::String(name.to_string()));
    if let Some(bus) = declared_bus(properties) {
        request.insert("EventBusName".to_string(), Value::String(bus));
    }
    let reported = call_events(ctx, region, "ListTargetsByRule", &Value::Object(request)).await?;
    Ok(reported
        .get("Targets")
        .and_then(Value::as_array)
        .map(|targets| {
            targets
                .iter()
                .filter_map(|target| target.get("Id").and_then(Value::as_str))
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default())
}

/// Delete the rule a stack created: its targets first, because "before you can delete the
/// rule, you must remove all associated targets" (`DeleteRule`), then the rule itself.
///
/// A rule EventBridge no longer holds is already in the state the delete asks for: the
/// read that lists its targets answers `ResourceNotFoundException` for it, which is the
/// one code that says the rule is gone, and `DeleteRule` is itself idempotent ("if you
/// call delete rule multiple times for the same rule, all calls will succeed").
pub(super) async fn delete_rule(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let name = &resource.physical_resource_id;
    let installed = match installed_target_ids(ctx, region, name, &resource.properties).await {
        Ok(installed) => installed,
        // Judged on the error CODE, never on the message, which carries the rule's name.
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => return Ok(()),
        Err(refusal) => return Err(refusal),
    };
    remove_targets(ctx, region, name, &resource.properties, &installed).await?;
    let mut request = Map::new();
    request.insert("Name".to_string(), Value::String(name.to_string()));
    if let Some(bus) = declared_bus(&resource.properties) {
        request.insert("EventBusName".to_string(), Value::String(bus));
    }
    match call_events(ctx, region, "DeleteRule", &Value::Object(request)).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The rule's configuration as EventBridge reports it, for drift detection. A rule it no
/// longer holds (`ResourceNotFoundException`) was deleted.
///
/// `DescribeRule` answers the rule's members under the very names the type declares them
/// with (`Description`, `EventBusName`, `EventPattern`, `ScheduleExpression`, `State`,
/// `RoleArn`), so its answer IS the actual configuration document; the declared
/// `EventPattern` document is compared against the text EventBridge holds by
/// [`super::drift`]'s own rule for a composite a service reports as a string.
pub(super) async fn describe_rule(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let mut request = Map::new();
    request.insert(
        "Name".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    if let Some(bus) = declared_bus(&resource.properties) {
        request.insert("EventBusName".to_string(), Value::String(bus));
    }
    match call_events(ctx, region, "DescribeRule", &Value::Object(request)).await {
        Ok(answer) => Ok(Some(answer)),
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The bus a declaration binds its rule to, or `None` for one that names none — which is
/// the account's `default` bus, the member's own documented default, so nothing is sent
/// for it.
pub(super) fn declared_bus(properties: &Value) -> Option<String> {
    properties
        .get("EventBusName")
        .and_then(scalar_text)
        .filter(|bus| !bus.is_empty())
}

/// The `Targets` a declaration names, in the order it writes them.
pub(super) fn declared_targets(properties: &Value) -> Vec<Value> {
    properties
        .get("Targets")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// One declared `Target` as `PutTargets` takes it: the members named identically on both
/// sides ([`EVENTS_TARGET_MEMBERS`]) passed through as the template wrote them, plus
/// `EcsParameters` with its two renamed members mapped ([`ecs_parameters`]).
pub(super) fn event_target(declared: &Value) -> Value {
    let mut target = Map::new();
    if let Some(members) = declared.as_object() {
        for member in EVENTS_TARGET_MEMBERS {
            if let Some(value) = members.get(*member) {
                target.insert((*member).to_string(), value.clone());
            }
        }
        if let Some(ecs) = members.get("EcsParameters") {
            target.insert("EcsParameters".to_string(), ecs_parameters(ecs));
        }
    }
    Value::Object(target)
}

/// A declared `EcsParameters` as `Target$EcsParameters` takes it.
///
/// Its members are named identically on both sides ([`EVENTS_ECS_PARAMETER_MEMBERS`])
/// except two: the task's tag list, which the property type publishes as `TagList` while
/// the shape's member is `Tags`, and the awsvpc configuration, which the property type
/// publishes as `NetworkConfiguration.AwsVpcConfiguration` while the
/// `NetworkConfiguration` shape's one member is `awsvpcConfiguration`
/// (`smithy_query(shape="EcsParameters,NetworkConfiguration")`).
pub(super) fn ecs_parameters(declared: &Value) -> Value {
    let mut mapped = Map::new();
    if let Some(members) = declared.as_object() {
        for member in EVENTS_ECS_PARAMETER_MEMBERS {
            if let Some(value) = members.get(*member) {
                mapped.insert((*member).to_string(), value.clone());
            }
        }
        if let Some(tags) = members.get("TagList") {
            mapped.insert("Tags".to_string(), tags.clone());
        }
        if let Some(awsvpc) = members
            .get("NetworkConfiguration")
            .and_then(|network| network.get("AwsVpcConfiguration"))
        {
            let mut network = Map::new();
            network.insert("awsvpcConfiguration".to_string(), awsvpc.clone());
            mapped.insert("NetworkConfiguration".to_string(), Value::Object(network));
        }
    }
    Value::Object(mapped)
}

/// Call one Amazon EventBridge operation on the awsJson1_1 wire its model declares.
pub(super) async fn call_events(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operation: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    call_json_rpc(
        ctx,
        EVENTS_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{EVENTS_TARGET_PREFIX}.{operation}"),
        request,
    )
    .await
}

// ---------------------------------------------------------------------------
// AWS Systems Manager: AWS::SSM::Parameter
// ---------------------------------------------------------------------------

/// AWS Systems Manager's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: ssm`, service shape `AmazonSSM`), which speaks `awsJson1_1`.
pub(super) const SSM_SIGNING_NAME: &str = "ssm";
pub(super) const SSM_TARGET_PREFIX: &str = "AmazonSSM";

/// The error code Systems Manager answers for a parameter it does not hold
/// (`ParameterNotFound`: "The parameter couldn't be found. Verify the name and try
/// again").
pub(super) const PARAMETER_NOT_FOUND: &str = "ParameterNotFound";

/// The Systems Manager operations the `AWS::SSM::Parameter` provider asks for, by name: the
/// one call that "create[s] or update[s] a parameter in Parameter Store" and the one that
/// removes it.
///
/// Named rather than spelled at each call because the provider REPORTS which of them was
/// refused ([`handler_refusal`]), so the name in the report and the name in the
/// `X-Amz-Target` cannot drift apart.
pub(super) const PUT_PARAMETER: &str = "PutParameter";
pub(super) const DELETE_PARAMETER: &str = "DeleteParameter";

/// How a provider of this catalogue reports a call to the service that owns its resource that
/// that service REFUSED.
///
/// CloudFormation quotes the resource provider's own message in the failed resource's
/// `ResourceStatusReason` (`super::super::provisioning`), and a provider reports the API
/// OPERATION it asked for beside that service's refusal of it — so the reason says both which
/// call failed and why, which is what a client diagnosing a failed deployment reads. The two
/// parts are the operation the provider named and the message the owning service worded; the
/// error CODE is left exactly as that service answered it, since the class the failure is
/// reported under is decided from the code and never from this text.
///
/// Shared by every arm that reports a refusal this way — the `AWS::SSM::Parameter` provider
/// here and the `AWS::SSM::Association` provider beside it ([`super::ssm_associations`]) — so
/// the wording cannot drift between two copies of it.
pub(super) fn handler_refusal(operation: &str, refusal: Refusal) -> Refusal {
    let message = if refusal.message.is_empty() {
        format!("Error occurred during operation '{operation}'.")
    } else {
        format!(
            "Error occurred during operation '{operation}': {}",
            refusal.message
        )
    };
    Refusal { message, ..refusal }
}

/// The `AWS::SSM::Parameter` properties `PutParameter` takes under the SAME member name.
///
/// Source: AWS's CloudFormation resource reference for the type against
/// `smithy_query(operation="PutParameter")` — every property of the type is a member of
/// that request, spelled identically. The two not here are the two that are not passed
/// straight through: `Name`, which CloudFormation decides and which is this resource's
/// physical id, and `Tags`, which the type publishes as a MAP while the request takes the
/// `[{Key, Value}]` list its own `Tag` shape spells.
pub(super) const SSM_PARAMETER_MEMBERS: &[&str] = &[
    "AllowedPattern",
    "DataType",
    "Description",
    "Policies",
    "Tier",
    "Type",
    "Value",
];

/// Create the parameter an `AWS::SSM::Parameter` declares, so a client that deployed the
/// stack and then calls `ssm GetParameter` with that name reads the value back rather than
/// a `ParameterNotFound`.
///
/// `Ref` of this type returns the parameter NAME, which is this resource's physical id, so
/// the parameter is created under exactly the name the stack reports for it. Nothing
/// `PutParameter` answers (the version it created, the tier it resolved) is a published
/// return value of the type, so no attribute is re-reported off it: the type's `Type` and
/// `Value` are the declaration's own.
///
/// The parameter carries the tags the declaration asks for plus the ones CloudFormation
/// gives the resource ([`tags_of`]), as the list Systems Manager's own `Tag` shape spells.
pub(super) async fn put_parameter(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = parameter_request(&resource.properties, &resource.physical_resource_id);
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("Tags".to_string(), tags_as_list(&tags, UPPER_CASED_TAG));
    }
    call_ssm(ctx, region, PUT_PARAMETER, &Value::Object(request))
        .await
        .map_err(|refusal| handler_refusal(PUT_PARAMETER, refusal))?;
    Ok(Created::default())
}

/// Apply a CHANGED `AWS::SSM::Parameter` declaration to the parameter the stack already
/// has, through the `Overwrite` half of the one call that "create[s] or update[s] a
/// parameter in Parameter Store".
///
/// No tag is sent with the overwrite: "to add tags to an existing Systems Manager
/// parameter, use the AddTagsToResource operation" (`PutParameter$Tags`), so the set the
/// parameter carries is left exactly as the create left it rather than replaced.
pub(super) async fn update_parameter(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Created, Refusal> {
    let mut request = parameter_request(declared, &resource.physical_resource_id);
    request.insert("Overwrite".to_string(), Value::Bool(true));
    call_ssm(ctx, region, PUT_PARAMETER, &Value::Object(request))
        .await
        .map_err(|refusal| handler_refusal(PUT_PARAMETER, refusal))?;
    Ok(Created::default())
}

/// Delete the parameter a stack created, addressed by its name — this resource's physical
/// id, which is what `Ref` returns for the type. A parameter Systems Manager no longer
/// holds (`ParameterNotFound`) is already in the state the delete asks for.
pub(super) async fn delete_parameter(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "Name": physical_resource_id });
    match call_ssm(ctx, region, DELETE_PARAMETER, &request).await {
        Ok(_) => Ok(()),
        // Judged on the error CODE, never on the message, which carries the request's own
        // parameter name.
        Err(refusal) if refusal.code == PARAMETER_NOT_FOUND => Ok(()),
        Err(refusal) => Err(handler_refusal(DELETE_PARAMETER, refusal)),
    }
}

/// The parameter's configuration as Systems Manager reports it, for drift detection: the
/// `Parameter` its own read answers with, whose `Type`, `Value` and `DataType` are the
/// properties the type declares under those very names. One it no longer holds
/// (`ParameterNotFound`) was deleted.
pub(super) async fn describe_parameter(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "Name": physical_resource_id });
    match call_ssm(ctx, region, "GetParameter", &request).await {
        Ok(answer) => Ok(Some(reported_member(&answer, "Parameter"))),
        Err(refusal) if refusal.code == PARAMETER_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The `PutParameter` request one declaration describes, under the name CloudFormation
/// decided: the members named identically on both sides ([`SSM_PARAMETER_MEMBERS`]), each
/// as the text its API member takes.
pub(super) fn parameter_request(properties: &Value, name: &str) -> Map<String, Value> {
    let mut request = Map::new();
    if let Some(declared) = properties.as_object() {
        for member in SSM_PARAMETER_MEMBERS {
            if let Some(value) = declared.get(*member) {
                request.insert((*member).to_string(), parameter_text(value));
            }
        }
    }
    request.insert("Name".to_string(), Value::String(name.to_string()));
    request
}

/// One declared parameter property as the TEXT its `PutParameter` member takes: every one
/// of them is a string member, and a template may write a value as a number, a boolean, or
/// — for the `Policies` array — as an inline document.
pub(super) fn parameter_text(value: &Value) -> Value {
    match document_text(value).or_else(|| scalar_text(value)) {
        Some(text) => Value::String(text),
        None => value.clone(),
    }
}

/// Call one AWS Systems Manager operation on the awsJson1_1 wire its model declares.
///
/// Reached from [`super::dynamic_references`] too, which resolves a `{{resolve:ssm...}}`
/// dynamic reference through this one spelling of that service's wire rather than a second
/// copy of it.
pub(crate) async fn call_ssm(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operation: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    call_json_rpc(
        ctx,
        SSM_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{SSM_TARGET_PREFIX}.{operation}"),
        request,
    )
    .await
}

// ---------------------------------------------------------------------------
// AWS Step Functions: AWS::StepFunctions::Activity
// ---------------------------------------------------------------------------

/// AWS Step Functions' signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: states`, service shape `AWSStepFunctions`), which speaks `awsJson1_0`.
pub(super) const SFN_SIGNING_NAME: &str = "states";
pub(super) const SFN_TARGET_PREFIX: &str = "AWSStepFunctions";

/// The error code Step Functions answers for an activity ARN no activity answers to
/// (`ActivityDoesNotExist`: "The specified activity does not exist").
pub(super) const ACTIVITY_DOES_NOT_EXIST: &str = "ActivityDoesNotExist";

/// Create the activity an `AWS::StepFunctions::Activity` declares, so a client that
/// deployed the stack and then calls `sfn DescribeActivity` with the ARN the stack
/// publishes reads the activity back rather than an `ActivityDoesNotExist`.
///
/// `Ref` of this type returns the activity's ARN, which is this resource's physical id, and
/// an activity ARN is a total function of the Region and the NAME — so the activity is
/// created under the name CloudFormation decided and the ARN Step Functions answers with is
/// the one the stack already computed. It is reported back as the resource's `Arn`
/// attribute all the same, so the handle the stack publishes is the one that service really
/// minted.
///
/// The type's two remaining properties are `CreateActivity`'s own members: the
/// `EncryptionConfiguration`, whose members Step Functions spells in lower camel case, and
/// the `Tags` list — carrying the tags the declaration asks for plus the ones
/// CloudFormation gives the resource ([`tags_of`]), as the list that model's own `Tag` shape
/// spells, so `sfn ListTagsForResource` on the ARN the stack publishes answers with the
/// stack's tags and the `aws:cloudformation:*` ones.
pub(super) async fn create_activity(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = Map::new();
    request.insert(
        "name".to_string(),
        Value::String(activity_name(resource)),
    );
    if let Some(configuration) = resource.properties.get("EncryptionConfiguration") {
        request.insert(
            "encryptionConfiguration".to_string(),
            lower_camel_members(configuration, 0)?,
        );
    }
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("tags".to_string(), tags_as_list(&tags, LOWER_CASED_TAG));
    }
    let answered = call_sfn(ctx, region, "CreateActivity", &Value::Object(request)).await?;
    let arn = answered
        .get("activityArn")
        .and_then(Value::as_str)
        .filter(|arn| !arn.is_empty())
        .ok_or_else(|| Refusal {
            code: "InternalFailure".to_string(),
            message: "AWS Step Functions answered CreateActivity without an activityArn"
                .to_string(),
        })?;

    let mut attributes = BTreeMap::new();
    attributes.insert("Arn".to_string(), arn.to_string());
    Ok(Created::attributes(attributes))
}

/// Delete the activity a stack created, addressed by its ARN — this resource's physical id,
/// which is what `Ref` returns for the type.
///
/// `DeleteActivity` models no does-not-exist error at all: an activity Step Functions no
/// longer holds is already in the state the delete asks for, and the call says so itself.
pub(super) async fn delete_activity(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "activityArn": physical_resource_id });
    call_sfn(ctx, region, "DeleteActivity", &request).await?;
    Ok(())
}

/// The activity's configuration as Step Functions reports it, for drift detection. One it
/// no longer holds (`ActivityDoesNotExist`, judged on the CODE and never on a message that
/// carries the request's own ARN) was deleted.
///
/// The one property its read reports is the activity's `Name`; an activity holds no other
/// configuration that `DescribeActivity` answers under a name this type declares — the
/// encryption settings it reports are spelled in that service's own member names, and a
/// property the read does not report is not evidence of anything ([`super::drift`]).
pub(super) async fn describe_activity(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "activityArn": physical_resource_id });
    match call_sfn(ctx, region, "DescribeActivity", &request).await {
        Ok(answer) => Ok(Some(reported_as_properties(&answer, &[("Name", "name")]))),
        Err(refusal) if refusal.code == ACTIVITY_DOES_NOT_EXIST => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The NAME of the activity a resource record describes: the `Name` its type's published
/// return values carry, which is the name the template asked for or the
/// `<stack>-<logical id>-<suffix>` name CloudFormation generated — never the physical id,
/// which for this type is the activity's ARN.
///
/// A record with no such attribute (one whose attribute budget was spent) falls back to the
/// last segment of that ARN, which is the activity's name.
pub(super) fn activity_name(resource: &StoredResource) -> String {
    if let Some(name) = resource.attributes.get("Name") {
        return name.clone();
    }
    resource
        .physical_resource_id
        .rsplit(':')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Call one AWS Step Functions operation on the awsJson1_0 wire its model declares.
pub(super) async fn call_sfn(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operation: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    call_json_rpc(
        ctx,
        SFN_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{SFN_TARGET_PREFIX}.{operation}"),
        request,
    )
    .await
}

// ---------------------------------------------------------------------------
// Amazon Cognito user pools: AWS::Cognito::UserPoolGroup
// ---------------------------------------------------------------------------

/// Amazon Cognito user pools' signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: cognito-idp`, service shape `AWSCognitoIdentityProviderService`), which
/// speaks `awsJson1_1`.
pub(super) const COGNITO_IDP_SIGNING_NAME: &str = "cognito-idp";
pub(super) const COGNITO_IDP_TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// The members a group's CONFIGURATION is, as `CreateGroup` and `UpdateGroup` both name
/// them — the type's properties beside the two that make the group's identity, spelled
/// identically on both sides.
pub(super) const GROUP_CONFIGURATION_MEMBERS: &[&str] = &["Description", "Precedence", "RoleArn"];

/// Create the group an `AWS::Cognito::UserPoolGroup` declares, so a client that deployed
/// the stack and then calls `cognito-idp GetGroup` with the pool it declared and the name
/// the stack publishes reads the group back rather than a `ResourceNotFoundException`.
///
/// The type's properties ARE `CreateGroup`'s own members, so they are passed through with
/// the one member CloudFormation decides — the group's NAME, which is this resource's
/// physical id — set to it. A group is addressed by the pair `(UserPoolId, GroupName)`, and
/// the pool half is a required property of the type: a declaration carrying none describes
/// no group that could be created, so it is refused here rather than with a request Amazon
/// Cognito would have to reject.
pub(super) async fn create_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let mut request = group_configuration(&resource.properties);
    request.insert("UserPoolId".to_string(), Value::String(group_pool(resource)?));
    request.insert(
        "GroupName".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    call_cognito_idp(ctx, region, "CreateGroup", &Value::Object(request)).await?;
    // The type publishes no `Fn::GetAtt` return value of its own: what identifies the
    // group is the pair the stack already records (its physical id and its declared
    // `UserPoolId`), so there is no attribute to adopt from the answer.
    Ok(Created::attributes(BTreeMap::new()))
}

/// Apply a CHANGED `AWS::Cognito::UserPoolGroup` declaration to the group the stack already
/// has, through Amazon Cognito's own `UpdateGroup` — which "updates any of the properties
/// for precedence, IAM role, or description" of the named group.
///
/// `Ok(None)` is a change that call cannot make, so the deployment reports it as not
/// applied rather than as a success that dropped it:
///
/// * a declaration that moves the group to a DIFFERENT user pool — a group belongs to one
///   pool, and `UpdateGroup` takes the pool only to address the group;
/// * a declaration that no longer states a `Precedence` or a `RoleArn` the group was
///   created with: a member `UpdateGroup` leaves out keeps its stored value, and neither
///   can be sent empty (`RoleArn` is `@length(min: 20)`, `Precedence` an integer), so
///   nothing in that call unsets them. A dropped `Description` IS sent, as the empty string
///   its own `@length(min: 0)` admits.
pub(super) async fn update_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    let pool = group_pool(resource)?;
    if declared.get("UserPoolId").and_then(scalar_text) != Some(pool.clone()) {
        return Ok(None);
    }
    let mut request = group_configuration(declared);
    for member in GROUP_CONFIGURATION_MEMBERS {
        if request.contains_key(*member) || resource.properties.get(*member).is_none() {
            continue;
        }
        // A member the declaration DROPPED. Only a description can be emptied through
        // this call.
        if *member == "Description" {
            request.insert((*member).to_string(), Value::String(String::new()));
        } else {
            return Ok(None);
        }
    }
    request.insert("UserPoolId".to_string(), Value::String(pool));
    request.insert(
        "GroupName".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    call_cognito_idp(ctx, region, "UpdateGroup", &Value::Object(request)).await?;
    Ok(Some(Created::attributes(BTreeMap::new())))
}

/// Delete the group a stack created, addressed by the pair that identifies it: the pool its
/// declaration names and its name, which is this resource's physical id.
///
/// A group Amazon Cognito no longer holds — and a pool it no longer holds, which cannot
/// hold the group either — is already in the state the delete asks for; both are that
/// service's `ResourceNotFoundException`, judged on the CODE and never on a message.
pub(super) async fn delete_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let request = serde_json::json!({
        "UserPoolId": group_pool(resource)?,
        "GroupName": resource.physical_resource_id.clone(),
    });
    match call_cognito_idp(ctx, region, "DeleteGroup", &request).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The group's configuration as Amazon Cognito reports it, for drift detection. One it no
/// longer holds (`ResourceNotFoundException`) was deleted.
///
/// `GetGroup` answers a `GroupType` whose members carry the very names this type declares
/// its properties with, so the answer maps onto the property vocabulary one for one.
pub(super) async fn describe_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({
        "UserPoolId": group_pool(resource)?,
        "GroupName": resource.physical_resource_id.clone(),
    });
    match call_cognito_idp(ctx, region, "GetGroup", &request).await {
        Ok(answer) => {
            let Some(group) = answer.get("Group") else {
                return Ok(None);
            };
            Ok(Some(reported_as_properties(
                group,
                &[
                    ("GroupName", "GroupName"),
                    ("UserPoolId", "UserPoolId"),
                    ("Description", "Description"),
                    ("Precedence", "Precedence"),
                    ("RoleArn", "RoleArn"),
                ],
            )))
        }
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The three configuration members a declaration states, as the request carries them. The
/// type and the API spell them identically, so each value goes through as the template wrote
/// it — except `Precedence`, which `PrecedenceType` models as an INTEGER while a template may
/// write it as a quoted YAML scalar or feed it through a `Ref` to a `String` parameter: it is
/// read through [`whole_number`], this module's one spelling rule for a property carrying a
/// whole number. A property the declaration leaves out is absent from the request; a
/// `Precedence` that is no whole number at all is left as written, so the owning service
/// refuses the value the template really declared rather than a silently changed one.
pub(super) fn group_configuration(properties: &Value) -> Map<String, Value> {
    let mut request = Map::new();
    for member in GROUP_CONFIGURATION_MEMBERS {
        let Some(value) = properties.get(*member) else {
            continue;
        };
        let carried = match *member {
            "Precedence" => {
                whole_number(Some(value)).map_or_else(|| value.clone(), Value::from)
            }
            _ => value.clone(),
        };
        request.insert((*member).to_string(), carried);
    }
    request
}

/// The user pool a group's declaration names — the other half of the group's identity.
///
/// `UserPoolId` is a required property of the type, so a declaration carrying none is
/// refused here, naming that property, rather than sent as a request the owning service
/// would reject.
pub(super) fn group_pool(resource: &StoredResource) -> Result<String, Refusal> {
    property(resource, "UserPoolId").ok_or_else(|| Refusal {
        code: MISSING_PROPERTY_CODE.to_string(),
        message: format!("{} declares no UserPoolId", resource.resource_type),
    })
}

/// Call one Amazon Cognito user-pools operation on the awsJson1_1 wire its model declares.
pub(super) async fn call_cognito_idp(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operation: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    call_json_rpc(
        ctx,
        COGNITO_IDP_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{COGNITO_IDP_TARGET_PREFIX}.{operation}"),
        request,
    )
    .await
}

// ---------------------------------------------------------------------------
// Amazon S3 Vectors: AWS::S3Vectors::VectorBucket / ::Index
// ---------------------------------------------------------------------------

/// Amazon S3 Vectors' signing name, from its model (`sigv4Name: s3vectors`, service shape
/// `S3Vectors`), which speaks `restJson1` — so its calls ride on [`call_rest_json`], each
/// on the URI its own operation's `@http` binding declares.
pub(super) const S3VECTORS_SIGNING_NAME: &str = "s3vectors";

/// The URIs the S3 Vectors operations a stack makes are bound to, as that model's `@http`
/// traits declare them (`POST /CreateVectorBucket` and so on: every operation of this
/// service is a POST to a URI named after it).
pub(super) const S3VECTORS_CREATE_VECTOR_BUCKET_URI: &str = "/CreateVectorBucket";
pub(super) const S3VECTORS_GET_VECTOR_BUCKET_URI: &str = "/GetVectorBucket";
pub(super) const S3VECTORS_DELETE_VECTOR_BUCKET_URI: &str = "/DeleteVectorBucket";
pub(super) const S3VECTORS_CREATE_INDEX_URI: &str = "/CreateIndex";
pub(super) const S3VECTORS_GET_INDEX_URI: &str = "/GetIndex";
pub(super) const S3VECTORS_DELETE_INDEX_URI: &str = "/DeleteIndex";

/// The error code S3 Vectors answers for a vector bucket or a vector index it does not
/// hold (`NotFoundException`, the modelled 404 of its reads and deletes).
pub(super) const S3VECTORS_NOT_FOUND: &str = "NotFoundException";

/// The members of an `AWS::S3Vectors::Index`'s own declaration that are `CreateIndex`'s own
/// members under the same name with a lower-case initial, and whose values are scalars that
/// go through as the template spells them.
///
/// Source: `CreateIndex`'s input members in that service's model (`indexName`, `dataType`,
/// `dimension`, `distanceMetric`), against the upper-camel property names a CloudFormation
/// template spells. The parent vector bucket is NOT one of them: it is resolved from the
/// resource's own ARN ([`vector_index_identity`]).
pub(super) const S3VECTORS_INDEX_SCALAR_MEMBERS: &[(&str, &str)] = &[
    ("DataType", "dataType"),
    ("Dimension", "dimension"),
    ("DistanceMetric", "distanceMetric"),
];

/// Create the vector bucket an `AWS::S3Vectors::VectorBucket` declares, so a client that
/// deployed the stack and then calls `s3vectors GetVectorBucket` with the ARN the stack
/// publishes reads the bucket back rather than a `NotFoundException`.
///
/// `Ref` of this type returns the bucket's ARN, which is this resource's physical id, and
/// such an ARN is a total function of the Region and the bucket's NAME — so the bucket is
/// created under the name CloudFormation decided ([`vector_bucket_name`]) and the ARN S3
/// Vectors answers with is the one the stack already computed. It is reported back as the
/// resource's `VectorBucketArn` attribute all the same, so the handle the stack publishes
/// is the one that service really minted.
///
/// The type's remaining properties are `CreateVectorBucket`'s own members: the
/// `EncryptionConfiguration`, whose members S3 Vectors spells in lower camel case, and the
/// tags — the ones the declaration asks for plus the ones CloudFormation gives the resource
/// ([`tags_of`]), as the MAP that request takes, so `s3vectors ListTagsForResource` on the
/// ARN the stack publishes answers with the stack's own tags.
pub(super) async fn create_vector_bucket(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = Map::new();
    request.insert(
        "vectorBucketName".to_string(),
        Value::String(vector_bucket_name(resource)),
    );
    if let Some(configuration) = resource.properties.get("EncryptionConfiguration") {
        request.insert(
            "encryptionConfiguration".to_string(),
            lower_camel_members(configuration, 0)?,
        );
    }
    let tags = tags_as_map(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Object(tags));
    }
    let answered = call_s3vectors(
        ctx,
        region,
        S3VECTORS_CREATE_VECTOR_BUCKET_URI,
        &Value::Object(request),
    )
    .await?;
    let arn = answered
        .get("vectorBucketArn")
        .and_then(Value::as_str)
        .filter(|arn| !arn.is_empty())
        .ok_or_else(|| Refusal {
            code: "InternalFailure".to_string(),
            message: "Amazon S3 Vectors answered CreateVectorBucket without a vectorBucketArn"
                .to_string(),
        })?;
    let mut attributes = BTreeMap::new();
    attributes.insert("VectorBucketArn".to_string(), arn.to_string());
    Ok(Created::attributes(attributes))
}

/// Delete the vector bucket a stack created, addressed by its ARN — this resource's
/// physical id, which is what `Ref` returns for the type. A bucket S3 Vectors no longer
/// holds (`NotFoundException`, judged on the CODE and never on a message that carries the
/// request's own name) is already in the state the delete asks for; one that still holds
/// vector indexes is NOT deleted (`ConflictException`), and that refusal is what the stack
/// reports.
pub(super) async fn delete_vector_bucket(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "vectorBucketArn": physical_resource_id });
    match call_s3vectors(ctx, region, S3VECTORS_DELETE_VECTOR_BUCKET_URI, &request).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == S3VECTORS_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The vector bucket's configuration as Amazon S3 Vectors reports it, for drift detection:
/// `GetVectorBucket`'s own record, read by the ARN that is this resource's physical id. One
/// it no longer holds (`NotFoundException`) was deleted.
///
/// The one property its read reports under the name this type declares is the bucket's
/// `VectorBucketName`; the encryption settings it reports are spelled in that service's own
/// member names, and a property the read does not report is not evidence of anything
/// ([`super::drift`]).
pub(super) async fn describe_vector_bucket(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "vectorBucketArn": physical_resource_id });
    match call_s3vectors(ctx, region, S3VECTORS_GET_VECTOR_BUCKET_URI, &request).await {
        Ok(answer) => Ok(answer.get("vectorBucket").map(|bucket| {
            reported_as_properties(bucket, &[("VectorBucketName", "vectorBucketName")])
        })),
        Err(refusal) if refusal.code == S3VECTORS_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// Create the vector index an `AWS::S3Vectors::Index` declares, in the vector bucket its
/// own ARN names.
///
/// `Ref` of this type returns the index's ARN, which is this resource's physical id and
/// which carries both names S3 Vectors keys an index by
/// (`bucket/<bucket>/index/<index>`) — so [`vector_index_identity`] reads the pair back off
/// it and the index is created under exactly the names the stack already published.
///
/// The type's remaining properties are `CreateIndex`'s own members: the three scalars
/// [`S3VECTORS_INDEX_SCALAR_MEMBERS`] names, and the `MetadataConfiguration` and
/// `EncryptionConfiguration` structures, whose members S3 Vectors spells in lower camel
/// case.
pub(super) async fn create_index(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let properties = resource
        .properties
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);
    let (bucket, index) = vector_index_identity(resource);
    let mut request = Map::new();
    if let Some(bucket) = bucket {
        request.insert("vectorBucketName".to_string(), Value::String(bucket));
    }
    request.insert("indexName".to_string(), Value::String(index));
    copy_members(&properties, S3VECTORS_INDEX_SCALAR_MEMBERS, &mut request);
    for (property, member) in [
        ("MetadataConfiguration", "metadataConfiguration"),
        ("EncryptionConfiguration", "encryptionConfiguration"),
    ] {
        if let Some(configuration) = properties.get(property) {
            request.insert(member.to_string(), lower_camel_members(configuration, 0)?);
        }
    }
    let tags = tags_as_map(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Object(tags));
    }
    let answered =
        call_s3vectors(ctx, region, S3VECTORS_CREATE_INDEX_URI, &Value::Object(request))
            .await?;
    let arn = answered
        .get("indexArn")
        .and_then(Value::as_str)
        .filter(|arn| !arn.is_empty())
        .ok_or_else(|| Refusal {
            code: "InternalFailure".to_string(),
            message: "Amazon S3 Vectors answered CreateIndex without an indexArn".to_string(),
        })?;
    let mut attributes = BTreeMap::new();
    attributes.insert("IndexArn".to_string(), arn.to_string());
    Ok(Created::attributes(attributes))
}

/// Delete the vector index a stack created, addressed by its ARN — this resource's physical
/// id. One S3 Vectors no longer holds (`NotFoundException`) is already in the state the
/// delete asks for.
pub(super) async fn delete_index(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "indexArn": physical_resource_id });
    match call_s3vectors(ctx, region, S3VECTORS_DELETE_INDEX_URI, &request).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == S3VECTORS_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The vector index's configuration as Amazon S3 Vectors reports it, for drift detection:
/// `GetIndex`'s own record, read by the ARN that is this resource's physical id. One it no
/// longer holds (`NotFoundException`) was deleted.
///
/// The properties its read reports under the names this type declares are the ones named
/// identically on both sides up to their initial letter; a property the read does not
/// report is not evidence of anything ([`super::drift`]).
pub(super) async fn describe_index(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "indexArn": physical_resource_id });
    match call_s3vectors(ctx, region, S3VECTORS_GET_INDEX_URI, &request).await {
        Ok(answer) => Ok(answer.get("index").map(|index| {
            let mut members = vec![
                ("VectorBucketName", "vectorBucketName"),
                ("IndexName", "indexName"),
            ];
            members.extend_from_slice(S3VECTORS_INDEX_SCALAR_MEMBERS);
            reported_as_properties(index, &members)
        })),
        Err(refusal) if refusal.code == S3VECTORS_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The NAME of the vector bucket a resource record describes: the `VectorBucketName` the
/// deployment gave it, which is the name the template asked for or the
/// `<stack>-<logical id>-<suffix>` name CloudFormation generated — never the physical id,
/// which for this type is the bucket's ARN.
///
/// A record with no such attribute (one whose attribute budget was spent) falls back to the
/// bucket segment of that ARN, which is the bucket's name.
pub(super) fn vector_bucket_name(resource: &StoredResource) -> String {
    if let Some(name) = resource.attributes.get("VectorBucketName") {
        return name.clone();
    }
    vector_bucket_of(&resource.physical_resource_id).unwrap_or_default()
}

/// The vector bucket and index a provisioned `AWS::S3Vectors::Index` record names: the two
/// segments of the index ARN that is this type's physical id
/// (`bucket/<bucket>/index/<index>`), which is how S3 Vectors keys an index.
///
/// A record whose physical id is NOT such an ARN names no bucket here either — the
/// declaration named none, so the deployment left the index with CloudFormation's own
/// generated name ([`super::template`]) — and the create then carries the index name alone,
/// so what the stack reports is S3 Vectors' own refusal of a request that addresses no
/// vector bucket rather than one invented here.
pub(super) fn vector_index_identity(resource: &StoredResource) -> (Option<String>, String) {
    let parsed = arn_resource(&resource.physical_resource_id)
        .and_then(|resource| resource.strip_prefix(S3VECTORS_BUCKET_SEGMENT))
        .and_then(|rest| rest.split_once("/index/"))
        .filter(|(bucket, index)| {
            !bucket.is_empty()
                && !bucket.contains('/')
                && !index.is_empty()
                && !index.contains('/')
        });
    match parsed {
        Some((bucket, index)) => (Some(bucket.to_string()), index.to_string()),
        None => (
            None,
            resource
                .attributes
                .get("IndexName")
                .cloned()
                .unwrap_or_else(|| resource.physical_resource_id.clone()),
        ),
    }
}

/// The bucket name an Amazon S3 Vectors vector-bucket ARN carries, or `None` for anything
/// that is not one.
pub(super) fn vector_bucket_of(arn: &str) -> Option<String> {
    arn_bucket_of(arn, S3VECTORS_BUCKET_SEGMENT)
}

/// The bucket name an ARN whose resource portion is `<segment><name>` carries, or `None` for
/// anything that is not such an ARN — a resource that does not open with that segment, an
/// empty name, or a name that is really a path to something INSIDE the bucket (an index, a
/// table), which names the bucket but is not its identifier.
///
/// The ONE reading of a bucket-segmented ARN in this provisioner, because more than one of
/// the S3 bucket VARIANTS is keyed by one: a vector bucket's ARN and a table bucket's ARN
/// differ only in the ARN namespace their own services publish, so the two cannot come to
/// read one differently.
pub(crate) fn arn_bucket_of(arn: &str, segment: &str) -> Option<String> {
    let bucket = arn_resource(arn)?.strip_prefix(segment)?;
    (!bucket.is_empty() && !bucket.contains('/')).then(|| bucket.to_string())
}

/// The RESOURCE portion of an ARN: everything past the fifth `:`, which is where every ARN
/// puts the resource it names.
///
/// Visible to the whole crate because more than one provisioner reads one: a resource's
/// published FULL NAME is the resource portion of its own ARN
/// ([`crate::state::provisioning::elastic_load_balancing`]), so the two cannot come to read
/// an ARN differently.
pub(crate) fn arn_resource(arn: &str) -> Option<&str> {
    let mut fields = arn.splitn(6, ':');
    let (Some("arn"), Some(_partition), Some(_service), Some(_region), Some(_account)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    fields.next()
}

/// Call one Amazon S3 Vectors operation on the `restJson1` wire its model declares: a POST
/// to the operation's own URI, carrying the request document.
pub(super) async fn call_s3vectors(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    uri: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    call_rest_json(
        ctx,
        S3VECTORS_SIGNING_NAME,
        region,
        "POST",
        uri,
        Some(request),
    )
    .await
}

// ---------------------------------------------------------------------------
// Amazon EventBridge Scheduler: AWS::Scheduler::ScheduleGroup / ::Schedule
// ---------------------------------------------------------------------------

/// EventBridge Scheduler's signing name, from its model (`sigv4Name: scheduler`, service
/// shape `AWSChronosService`), which speaks `restJson1` — so its calls ride on
/// [`call_rest_json`], each on the URI its own operation's `@http` binding declares.
pub(super) const SCHEDULER_SIGNING_NAME: &str = "scheduler";

/// The URIs the EventBridge Scheduler operations a stack makes are bound to, as that
/// model's `@http` traits declare them: `POST|GET|DELETE /schedule-groups/{Name}` and
/// `POST|GET|DELETE /schedules/{Name}`, the latter naming the group in the `groupName`
/// query parameter its read and its delete bind it to.
pub(super) const SCHEDULER_SCHEDULE_GROUPS_URI: &str = "/schedule-groups";
pub(super) const SCHEDULER_SCHEDULES_URI: &str = "/schedules";

/// The URI that model binds its three tagging operations to (`GET|POST|DELETE
/// /tags/{ResourceArn}`), the delete naming the keys it drops in the `TagKeys` query
/// parameter it binds them to.
pub(super) const SCHEDULER_TAGS_URI: &str = "/tags";

/// The error code EventBridge Scheduler answers for a schedule group or a schedule it does
/// not hold (`ResourceNotFoundException`, the modelled 404 of its reads and its deletes).
pub(super) const SCHEDULER_RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The members of a `CreateSchedule` request that are TIMESTAMPS (`StartDate` and
/// `EndDate`, each `@timestamp` in that model), which a template spells as an ISO-8601
/// instant and the wire carries as a number ([`epoch_seconds`]).
pub(super) const SCHEDULER_INSTANT_MEMBERS: &[&str] = &["StartDate", "EndDate"];

/// Create the schedule group an `AWS::Scheduler::ScheduleGroup` declares, so a client that
/// deployed the stack and then calls `scheduler GetScheduleGroup` with the name the stack
/// publishes reads the group back rather than a `ResourceNotFoundException`.
///
/// "Ref returns the schedule group name", which is this resource's physical id and the one
/// member `CreateScheduleGroup` carries in its URI (`POST /schedule-groups/{Name}`), so the
/// group is created under the name CloudFormation decided. The type's one other property is
/// that create's own `Tags` member — the tags the declaration asks for plus the ones
/// CloudFormation gives the resource ([`tags_of`]), as the `[{Key, Value}]` list that
/// model's own `Tag` shape spells, so `scheduler ListTagsForResource` on the ARN the stack
/// publishes answers with the stack's tags and the `aws:cloudformation:*` ones.
///
/// The group's ARN is reported back as its `Arn` attribute, so the handle the stack
/// publishes is the one that service really minted.
pub(super) async fn create_schedule_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = Map::new();
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("Tags".to_string(), tags_as_list(&tags, UPPER_CASED_TAG));
    }
    let answered = call_rest_json(
        ctx,
        SCHEDULER_SIGNING_NAME,
        region,
        "POST",
        &format!(
            "{SCHEDULER_SCHEDULE_GROUPS_URI}/{}",
            percent_encoded(&resource.physical_resource_id)
        ),
        Some(&Value::Object(request)),
    )
    .await?;
    let arn = answered
        .get("ScheduleGroupArn")
        .and_then(Value::as_str)
        .filter(|arn| !arn.is_empty())
        .ok_or_else(|| Refusal {
            code: "InternalFailure".to_string(),
            message: "EventBridge Scheduler answered CreateScheduleGroup without a \
                      ScheduleGroupArn"
                .to_string(),
        })?;
    let mut attributes = BTreeMap::new();
    attributes.insert("Arn".to_string(), arn.to_string());
    Ok(Created::attributes(attributes))
}

/// Delete the schedule group a stack created, addressed by its name — this resource's
/// physical id. A group EventBridge Scheduler no longer holds
/// (`ResourceNotFoundException`, judged on the CODE and never on a message that carries
/// the request's own name) is already in the state the delete asks for.
pub(super) async fn delete_schedule_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let uri = format!(
        "{SCHEDULER_SCHEDULE_GROUPS_URI}/{}",
        percent_encoded(physical_resource_id)
    );
    match call_rest_json(ctx, SCHEDULER_SIGNING_NAME, region, "DELETE", &uri, None).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == SCHEDULER_RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The schedule group's configuration as EventBridge Scheduler reports it, for drift
/// detection: `GetScheduleGroup`'s own record. One it no longer holds
/// (`ResourceNotFoundException`) was deleted.
///
/// The one property its read reports under the name this type declares is the group's
/// `Name`; the tags it holds are answered by a tagging call of its own, and a property the
/// read does not report is not evidence of anything ([`super::drift`]).
pub(super) async fn describe_schedule_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    Ok(read_schedule_group(ctx, region, physical_resource_id)
        .await?
        .map(|answer| reported_as_properties(&answer, &[("Name", "Name")])))
}

/// `GetScheduleGroup`'s whole answer for the group of `name`, or `None` for one EventBridge
/// Scheduler no longer holds (`ResourceNotFoundException`, judged on the CODE).
///
/// The one read of a group in this module, so the drift comparison above and the ARN its
/// tagging calls address it by ([`retag_schedule_group`]) come from the same answer rather
/// than from two spellings of the same request.
pub(super) async fn read_schedule_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
) -> Result<Option<Value>, Refusal> {
    let uri = format!(
        "{SCHEDULER_SCHEDULE_GROUPS_URI}/{}",
        percent_encoded(name)
    );
    match call_rest_json(ctx, SCHEDULER_SIGNING_NAME, region, "GET", &uri, None).await {
        Ok(answer) => Ok(Some(answer)),
        Err(refusal) if refusal.code == SCHEDULER_RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// Apply a CHANGED `AWS::Scheduler::ScheduleGroup` declaration to the group the stack
/// already has.
///
/// The type publishes exactly two properties. Its `Name` IS the group's identity — the
/// physical id a change to which replaces the resource rather than reaching here — and its
/// `Tags` are no member of any call that changes a group that exists, so they travel
/// through that service's own tagging calls instead ([`retag_schedule_group`]). A
/// declaration whose change is its tags is therefore applied with nothing asked of
/// EventBridge Scheduler here, and one that states some other property differently is
/// reported as NOT applied rather than as a success that dropped it.
pub(super) fn update_schedule_group(resource: &StoredResource, declared: &Value) -> Option<Created> {
    if schedule_group_configuration(&resource.properties) != schedule_group_configuration(declared)
    {
        return None;
    }
    Some(Created::attributes(BTreeMap::new()))
}

/// One schedule-group declaration less its `Tags`: the half of it a configuration change is
/// judged on, the other half being the one the tagging calls carry.
pub(super) fn schedule_group_configuration(properties: &Value) -> Map<String, Value> {
    match properties {
        Value::Object(object) => object
            .iter()
            .filter(|(member, _)| member.as_str() != "Tags")
            .map(|(member, value)| (member.clone(), value.clone()))
            .collect(),
        _ => Map::new(),
    }
}

/// Bring the tags of the schedule group a resource record names to the ones this deployment
/// gives it, through EventBridge Scheduler's own `TagResource` and `UntagResource`.
///
/// The set is the declaration's own `Tags` beside the ones CloudFormation propagates
/// ([`tags_of`]) — the same set the CREATE applied — so a key outside it is one this
/// deployment no longer gives the resource and is removed, and one in it with another value
/// is re-valued. Where the two already agree nothing is called at all, which is what makes
/// this safe to ask on every deployment.
///
/// Those calls address a group by its ARN, which is the handle that service minted rather
/// than one rebuilt here, so it is read back from the group itself
/// ([`read_schedule_group`]). A group EventBridge Scheduler no longer holds has no tags to
/// reconcile.
pub(super) async fn retag_schedule_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
    propagated: &[(String, String)],
) -> Result<(), Refusal> {
    let Some(arn) = read_schedule_group(ctx, region, &resource.physical_resource_id)
        .await?
        .as_ref()
        .and_then(|group| group.get("Arn"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
    else {
        return Ok(());
    };
    let uri = format!("{SCHEDULER_TAGS_URI}/{}", percent_encoded(&arn));
    // That service answers its tag read as the same `[{Key, Value}]` list a declaration's
    // `Tags` property is written as, so one reader serves both sides.
    let held = declared_tags(
        &call_rest_json(ctx, SCHEDULER_SIGNING_NAME, region, "GET", &uri, None).await?,
    );
    let desired = tags_of(declared, propagated);
    let (added, removed) = tag_delta(&held, &desired);
    if !added.is_empty() {
        call_rest_json(
            ctx,
            SCHEDULER_SIGNING_NAME,
            region,
            "POST",
            &uri,
            Some(&serde_json::json!({ "Tags": tags_as_list(&added, UPPER_CASED_TAG) })),
        )
        .await?;
    }
    if !removed.is_empty() {
        // `TagKeys` is an `@httpQuery` LIST, which travels as the parameter repeated once
        // per key rather than as one joined value.
        let keys = removed
            .iter()
            .map(|key| format!("TagKeys={}", percent_encoded(key)))
            .collect::<Vec<String>>()
            .join("&");
        call_rest_json(
            ctx,
            SCHEDULER_SIGNING_NAME,
            region,
            "DELETE",
            &format!("{uri}?{keys}"),
            None,
        )
        .await?;
    }
    Ok(())
}

/// Create the schedule an `AWS::Scheduler::Schedule` declares, in the group its
/// declaration names.
///
/// The type's properties ARE `CreateSchedule`'s own members — `Description`, `EndDate`,
/// `FlexibleTimeWindow`, `GroupName`, `KmsKeyArn`, `ScheduleExpression`,
/// `ScheduleExpressionTimezone`, `StartDate`, `State` and `Target` are named identically on
/// both sides, and the correspondence holds all the way down (a `Target`'s `Arn`, `RoleArn`,
/// `Input`, `DeadLetterConfig`, `RetryPolicy` and the five templated parameter structures;
/// a `FlexibleTimeWindow`'s `Mode` and `MaximumWindowInMinutes` —
/// `smithy_query(shape="Target,FlexibleTimeWindow")`) — so the evaluated property document
/// IS the request ([`schedule_request`]).
///
/// "Ref returns the schedule name", so the physical id stays the name the stack computed,
/// and the schedule's ARN is reported back as its `Arn` attribute.
pub(super) async fn create_schedule(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let answered = call_rest_json(
        ctx,
        SCHEDULER_SIGNING_NAME,
        region,
        "POST",
        &format!(
            "{SCHEDULER_SCHEDULES_URI}/{}",
            percent_encoded(&resource.physical_resource_id)
        ),
        Some(&schedule_request(&resource.properties)?),
    )
    .await?;
    Ok(Created::attributes(schedule_arn_of(
        &answered,
        "CreateSchedule",
    )?))
}

/// Apply a CHANGED `AWS::Scheduler::Schedule` declaration to the schedule the stack already
/// has, through EventBridge Scheduler's own `UpdateSchedule` (`PUT /schedules/{Name}`).
///
/// That call "uses all values, including empty values, specified in the request and
/// overrides the existing schedule", so a member the new declaration leaves out is set back
/// to its system default — which is exactly what the declaration now states about it. The
/// request is therefore the same document the create builds from a declaration
/// ([`schedule_request`]), and the ARN that service answers with is reported back as the
/// resource's `Arn` attribute.
///
/// The pair EventBridge Scheduler keys a schedule by is its name AND its group, and no call
/// of that service moves a schedule from one group to another: a declaration that names
/// another group is applied the way CloudFormation applies a change no in-place call can
/// make — the schedule is created where the declaration now puts it, and the one it left is
/// deleted — so that service ends up holding exactly the declared schedule and the group it
/// left holds nothing of this stack's.
pub(super) async fn update_schedule(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    let held_group = scheduled_group_of(&resource.properties);
    if held_group != scheduled_group_of(declared) {
        let declaring = StoredResource {
            properties: declared.clone(),
            ..resource.clone()
        };
        // Created before the one it replaces is deleted, so the schedule the stack
        // publishes exists throughout the change.
        let created = create_schedule(ctx, region, &declaring).await?;
        delete_schedule_in(ctx, region, &resource.physical_resource_id, &held_group).await?;
        return Ok(Some(created));
    }
    let answered = call_rest_json(
        ctx,
        SCHEDULER_SIGNING_NAME,
        region,
        "PUT",
        &format!(
            "{SCHEDULER_SCHEDULES_URI}/{}",
            percent_encoded(&resource.physical_resource_id)
        ),
        Some(&schedule_request(declared)?),
    )
    .await?;
    Ok(Some(Created::attributes(schedule_arn_of(
        &answered,
        "UpdateSchedule",
    )?)))
}

/// The request body `CreateSchedule` and `UpdateSchedule` take for one declaration: the
/// evaluated property document itself, less the `Name` both of them bind to their URI, with
/// each timestamp member carried as the wire carries a timestamp ([`epoch_seconds`]).
///
/// The one place a declaration becomes a request, so the create and the in-place update of
/// the same declaration cannot build two different schedules out of it.
pub(super) fn schedule_request(properties: &Value) -> Result<Value, Refusal> {
    let mut request = match properties {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    request.remove("Name");
    for member in SCHEDULER_INSTANT_MEMBERS {
        let Some(declared) = request.get(*member).cloned() else {
            continue;
        };
        // A template that already wrote the number goes through as written; only the
        // ISO-8601 TEXT a template spells a timestamp property with is converted, and text
        // that is not an instant is refused here rather than sent to be rejected.
        let Some(text) = declared.as_str() else {
            continue;
        };
        let instant = epoch_seconds(text).ok_or_else(|| Refusal {
            code: "ValidationException".to_string(),
            message: format!("{member} must be an ISO-8601 instant"),
        })?;
        request.insert((*member).to_string(), instant);
    }
    Ok(Value::Object(request))
}

/// The `Arn` attribute a schedule write reports: the `ScheduleArn` the named call answered
/// with, which is the handle EventBridge Scheduler really minted for it.
pub(super) fn schedule_arn_of(answered: &Value, call: &str) -> Result<BTreeMap<String, String>, Refusal> {
    let arn = answered
        .get("ScheduleArn")
        .and_then(Value::as_str)
        .filter(|arn| !arn.is_empty())
        .ok_or_else(|| Refusal {
            code: "InternalFailure".to_string(),
            message: format!("EventBridge Scheduler answered {call} without a ScheduleArn"),
        })?;
    let mut attributes = BTreeMap::new();
    attributes.insert("Arn".to_string(), arn.to_string());
    Ok(attributes)
}

/// Delete the schedule a stack created, addressed by the pair EventBridge Scheduler keys
/// one by: its name — this resource's physical id — and the group holding it
/// ([`scheduled_group_of`]).
pub(super) async fn delete_schedule(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    delete_schedule_in(
        ctx,
        region,
        &resource.physical_resource_id,
        &scheduled_group_of(&resource.properties),
    )
    .await
}

/// Delete the schedule of `name` in `group`. One that service no longer holds
/// (`ResourceNotFoundException`, judged on the CODE and never on a message that carries the
/// request's own name) is already in the state the delete asks for, which is also the state
/// a deleted group leaves its schedules in ("deleting a schedule group results in
/// EventBridge Scheduler deleting all schedules associated with the group").
pub(super) async fn delete_schedule_in(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
    group: &str,
) -> Result<(), Refusal> {
    let uri = schedule_uri(name, group);
    match call_rest_json(ctx, SCHEDULER_SIGNING_NAME, region, "DELETE", &uri, None).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == SCHEDULER_RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The schedule's configuration as EventBridge Scheduler reports it, for drift detection:
/// `GetSchedule`'s own record, read by the same name-and-group pair the delete uses. One it
/// no longer holds (`ResourceNotFoundException`) was deleted.
///
/// Every member that read reports is spelled exactly as this type's property is, so the
/// comparison is the declaration against what the service really holds; a property the read
/// does not report is not evidence of anything ([`super::drift`]).
pub(super) async fn describe_schedule(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let uri = schedule_uri(
        &resource.physical_resource_id,
        &scheduled_group_of(&resource.properties),
    );
    match call_rest_json(ctx, SCHEDULER_SIGNING_NAME, region, "GET", &uri, None).await {
        Ok(answer) => Ok(Some(reported_as_properties(
            &answer,
            &[
                ("Description", "Description"),
                ("FlexibleTimeWindow", "FlexibleTimeWindow"),
                ("GroupName", "GroupName"),
                ("KmsKeyArn", "KmsKeyArn"),
                ("Name", "Name"),
                ("ScheduleExpression", "ScheduleExpression"),
                ("ScheduleExpressionTimezone", "ScheduleExpressionTimezone"),
                ("State", "State"),
                ("Target", "Target"),
            ],
        ))),
        Err(refusal) if refusal.code == SCHEDULER_RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The URI that addresses ONE schedule: its name in the path and its group in the
/// `groupName` query parameter `GetSchedule` and `DeleteSchedule` bind it to.
pub(super) fn schedule_uri(name: &str, group: &str) -> String {
    format!(
        "{SCHEDULER_SCHEDULES_URI}/{}?groupName={}",
        percent_encoded(name),
        percent_encoded(group)
    )
}

/// The schedule group one `AWS::Scheduler::Schedule` property document puts its schedule
/// in: the `GroupName` it names, else the account's `default` group — "if you omit this, the
/// default schedule group is used" — which is the group the create lands it in and half of
/// the pair EventBridge Scheduler keys a schedule by.
pub(super) fn scheduled_group_of(properties: &Value) -> String {
    properties
        .get("GroupName")
        .and_then(Value::as_str)
        .filter(|group| !group.is_empty())
        .unwrap_or(DEFAULT_SCHEDULE_GROUP)
        .to_string()
}

/// One declared instant as the seconds since the epoch a `restJson1` timestamp member is
/// carried as (that protocol's default timestamp format, which the member declares no
/// `@timestampFormat` over), or `None` for text that is not an ISO-8601 instant.
pub(super) fn epoch_seconds(declared: &str) -> Option<Value> {
    let instant = ::aws_smithy_types::DateTime::from_str(
        declared,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()?;
    ::serde_json::Number::from_f64(instant.as_secs_f64()).map(Value::Number)
}

/// One timestamp a `restJson1` answer carries, as the ISO-8601 instant a template value
/// spells one with — the reverse of [`epoch_seconds`] above, and the ONE reading of that
/// direction, so two arms handed the same answer report the same text.
///
/// A timestamp is a VALUE on the wire rather than text: a JSON protocol carries one as the
/// seconds since the epoch unless the member's own `@timestampFormat` makes it a date-time
/// string. A number is therefore rendered as the instant here, and a value the answering
/// service already wrote as text is reported as it came; anything else is no instant at all,
/// which is what `None` reports.
///
/// The rendering is at MICROSECOND precision. A number holds the fraction of a second in a
/// binary form that carries no exact nanosecond count, so printing every digit of it would
/// publish noise below the precision the number has — and a client comparing this text
/// against the timestamp member it came from would read two instants a fraction of a
/// microsecond apart. Microseconds is the precision a reader of such a number keeps, so the
/// fraction is rounded to it, carrying a whole extra second where that rounds up.
pub(super) fn instant_text(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return (!text.is_empty()).then(|| text.to_string());
    }
    let seconds = value.as_f64().filter(|seconds| seconds.is_finite())?;
    let whole = seconds.floor();
    let micros = ((seconds - whole) * MICROS_PER_SECOND).round() as u32;
    let (whole, micros) = if f64::from(micros) >= MICROS_PER_SECOND {
        (whole + 1.0, 0)
    } else {
        (whole, micros)
    };
    ::aws_smithy_types::DateTime::from_secs_and_nanos(whole as i64, micros * NANOS_PER_MICRO)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()
}

/// How many microseconds one second holds, and how many nanoseconds one microsecond does:
/// the precision [`instant_text`] renders an instant carried as a number at.
const MICROS_PER_SECOND: f64 = 1_000_000.0;
const NANOS_PER_MICRO: u32 = 1_000;

// ---------------------------------------------------------------------------
// AWS Certificate Manager: AWS::CertificateManager::AcmeEndpoint
// ---------------------------------------------------------------------------

/// ACM's signing name and `X-Amz-Target` prefix, from its model (`sigv4Name: acm`,
/// service shape `CertificateManager`), which speaks `awsJson1_1`.
pub(super) const ACM_SIGNING_NAME: &str = "acm";
pub(super) const ACM_TARGET_PREFIX: &str = "CertificateManager";

/// Create the ACME endpoint an `AWS::CertificateManager::AcmeEndpoint` declares.
///
/// The type's properties ARE `CreateAcmeEndpoint`'s own members, spelled identically on
/// both sides — `AuthorizationBehavior`, `Contact`, `CertificateAuthority`,
/// `CertificateTags` and `Tags` (`smithy_query(operation="CreateAcmeEndpoint")`) — so the
/// evaluated property document IS the request. There is no member for a name or an
/// identifier, so nothing CloudFormation computed is sent with it.
///
/// `Tags` is the one member not carried verbatim: the endpoint carries the tags
/// CloudFormation gives it beside the declared ones ([`tags_of`]), as the `[{Key, Value}]`
/// list ACM's own `Tag` shape spells.
///
/// The endpoint's identifier is ACM's OWN to mint — the create takes no identifier and
/// answers with `AcmeEndpointArn` — so that answer is reported back as the resource's
/// physical id, which is what a client then passes to `DescribeAcmeEndpoint`, and as the
/// ARN attributes read off the resource, so neither reports a value the naming rules
/// computed instead.
pub(super) async fn create_acme_endpoint(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = match &resource.properties {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("Tags".to_string(), tags_as_list(&tags, UPPER_CASED_TAG));
    }
    let reported = call_json_rpc(
        ctx,
        ACM_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ACM_TARGET_PREFIX}.CreateAcmeEndpoint"),
        &Value::Object(request),
    )
    .await?;
    let Some(arn) = reported.get("AcmeEndpointArn").and_then(Value::as_str) else {
        // The create answered 2xx without the ARN its output models. Nothing addresses an
        // ACME endpoint without one, so this is reported as a failure rather than recorded
        // as a resource with an identifier nothing minted.
        return Err(Refusal {
            code: "InternalFailure".to_string(),
            message: "CreateAcmeEndpoint answered no AcmeEndpointArn".to_string(),
        });
    };
    let mut attributes = BTreeMap::new();
    attributes.insert("Arn".to_string(), arn.to_string());
    attributes.insert("AcmeEndpointArn".to_string(), arn.to_string());
    Ok(Created {
        physical_resource_id: Some(arn.to_string()),
        attributes,
    })
}

/// Delete the ACME endpoint a stack created.
///
/// `DeleteAcmeEndpoint` is `@idempotent` and its model declares no not-found error: an ARN
/// ACM holds no endpoint for is already in the state the delete asks for, and ACM answers
/// that call successfully, so there is no not-found code to read here.
pub(super) async fn delete_acme_endpoint(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "AcmeEndpointArn": physical_resource_id });
    call_json_rpc(
        ctx,
        ACM_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ACM_TARGET_PREFIX}.DeleteAcmeEndpoint"),
        &request,
    )
    .await
    .map(|_| ())
}

/// The endpoint's configuration as ACM reports it, for drift detection. An ARN ACM no
/// longer holds an endpoint for (`ResourceNotFoundException`) was deleted.
///
/// Only the members `DescribeAcmeEndpoint` reports in the SAME shape the type declares
/// them in are reported, because a property the actual document does not carry is not
/// evidence of anything (`super::drift::differs`): `AuthorizationBehavior`, `Contact` and
/// `CertificateAuthority`, each the identical member on both sides. `Tags` is not reported
/// at all — the describe answers none, and `CertificateTags` comes back as a tag LIST
/// whose order ACM decides, so comparing it would read an ordering as drift.
pub(super) async fn describe_acme_endpoint(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "AcmeEndpointArn": physical_resource_id });
    let reported = match call_json_rpc(
        ctx,
        ACM_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ACM_TARGET_PREFIX}.DescribeAcmeEndpoint"),
        &request,
    )
    .await
    {
        Ok(reported) => reported,
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => return Ok(None),
        Err(refusal) => return Err(refusal),
    };
    let Some(endpoint) = reported.get("AcmeEndpoint") else {
        return Ok(Some(Value::Object(Map::new())));
    };
    Ok(Some(reported_as_properties(
        endpoint,
        &[
            ("AuthorizationBehavior", "AuthorizationBehavior"),
            ("Contact", "Contact"),
            ("CertificateAuthority", "CertificateAuthority"),
        ],
    )))
}

// ---------------------------------------------------------------------------
// AWS Certificate Manager: AWS::CertificateManager::Certificate
// ---------------------------------------------------------------------------

/// The properties of `AWS::CertificateManager::Certificate` that are `RequestCertificate`'s
/// own members, spelled identically on both sides: the domain the certificate covers, the
/// further names it covers, how the domains are to be validated, the private CA that is to
/// issue it and the key algorithm it is to be generated with
/// (`smithy_query(operation="RequestCertificate")`).
pub(super) const ACM_CERTIFICATE_MEMBERS: &[(&str, &str)] = &[
    ("DomainName", "DomainName"),
    ("SubjectAlternativeNames", "SubjectAlternativeNames"),
    ("ValidationMethod", "ValidationMethod"),
    ("CertificateAuthorityArn", "CertificateAuthorityArn"),
    ("KeyAlgorithm", "KeyAlgorithm"),
];

/// The members of one entry of the type's `DomainValidationOptions`, which are the members
/// of ACM's own `DomainValidationOption` under the same names.
pub(super) const ACM_DOMAIN_VALIDATION_OPTION_MEMBERS: &[(&str, &str)] = &[
    ("DomainName", "DomainName"),
    ("ValidationDomain", "ValidationDomain"),
];

/// The property of a `DomainValidationOptions` entry this provisioner does not map onto the
/// create's request: `HostedZoneId` is not a member of ACM's `DomainValidationOption` at
/// all — it names the Route 53 hosted zone CloudFormation itself publishes the validation
/// record into, which this emulator does not do — so a declaration that states one is
/// refused rather than deployed as a certificate whose validation record nothing publishes.
pub(super) const ACM_HOSTED_ZONE_ID: &str = "HostedZoneId";

/// The type's one property that is a member of the create's `Options` rather than of the
/// request itself (`CertificateOptions$CertificateTransparencyLoggingPreference`).
pub(super) const ACM_TRANSPARENCY_LOGGING: &str = "CertificateTransparencyLoggingPreference";

/// The `CertificateStatus` values this arm reads: the status a requested certificate enters
/// and the one it is usable in ("A certificate enters status PENDING_VALIDATION upon being
/// requested", `CertificateDetail$Status`).
pub(super) const CERTIFICATE_PENDING_VALIDATION: &str = "PENDING_VALIDATION";
pub(super) const CERTIFICATE_ISSUED: &str = "ISSUED";

/// Request the certificate an `AWS::CertificateManager::Certificate` declares, so a client
/// that reads the stack's physical resource id can call ACM with it (`DescribeCertificate`,
/// `AddTagsToCertificate`) rather than being handed an identifier ACM never minted.
///
/// The type's properties are NOT the create's own members throughout, so they are mapped
/// one by one ([`ACM_CERTIFICATE_MEMBERS`] for the ones named identically, `Tags` as ACM's
/// own tag list, `CertificateTransparencyLoggingPreference` into the create's `Options`,
/// and each `DomainValidationOptions` entry onto ACM's `DomainValidationOption`), and a
/// property with no member at all is refused ([`ACM_HOSTED_ZONE_ID`]) rather than dropped.
/// Nothing the declaration does not state is sent.
///
/// The certificate's identifier is ACM's OWN to mint — the create takes no identifier and
/// answers with `CertificateArn`, and "Ref returns the certificate ARN" — so that answer is
/// reported back as the resource's physical id, which is what a client then passes to
/// `DescribeCertificate`.
pub(super) async fn request_certificate(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let properties = resource
        .properties
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);
    let mut request = Map::new();
    copy_members(&properties, ACM_CERTIFICATE_MEMBERS, &mut request);
    if let Some(Value::Array(options)) = properties.get("DomainValidationOptions") {
        let mut mapped: Vec<Value> = Vec::with_capacity(options.len());
        for option in options {
            if option.get(ACM_HOSTED_ZONE_ID).is_some() {
                return Err(Refusal {
                    code: NOT_IMPLEMENTED.to_string(),
                    message: format!(
                        "this emulator does not implement the {ACM_HOSTED_ZONE_ID} property of \
                         {}'s DomainValidationOptions",
                        resource.resource_type
                    ),
                });
            }
            mapped.push(mapped_members(option, ACM_DOMAIN_VALIDATION_OPTION_MEMBERS));
        }
        request.insert("DomainValidationOptions".to_string(), Value::Array(mapped));
    }
    if let Some(preference) = properties.get(ACM_TRANSPARENCY_LOGGING) {
        let mut options = Map::new();
        options.insert(ACM_TRANSPARENCY_LOGGING.to_string(), preference.clone());
        request.insert("Options".to_string(), Value::Object(options));
    }
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("Tags".to_string(), tags_as_list(&tags, UPPER_CASED_TAG));
    }
    let reported = call_json_rpc(
        ctx,
        ACM_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ACM_TARGET_PREFIX}.RequestCertificate"),
        &Value::Object(request),
    )
    .await?;
    let Some(arn) = reported.get("CertificateArn").and_then(Value::as_str) else {
        // The create answered 2xx without the ARN its output models. Nothing addresses a
        // certificate without one, so this is reported as a failure rather than recorded as
        // a resource with an identifier nothing minted.
        return Err(Refusal {
            code: "InternalFailure".to_string(),
            message: "RequestCertificate answered no CertificateArn".to_string(),
        });
    };
    Ok(Created {
        physical_resource_id: Some(arn.to_string()),
        attributes: BTreeMap::new(),
    })
}

/// Whether ACM has issued the certificate a stack requested.
///
/// "A certificate enters status PENDING_VALIDATION upon being requested"
/// (`CertificateDetail$Status`) and the certificate cannot serve traffic until ACM issues
/// it, so `ISSUED` is the state the resource's creation is finished in, `PENDING_VALIDATION`
/// is the state it is still coming up in, and every other status is one no wait will bring
/// out of: a validation that timed out, a request ACM failed, a revoked or expired
/// certificate. A certificate ACM no longer holds is in the same position — nothing is going
/// to issue it.
pub(super) async fn certificate_readiness(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Readiness, Refusal> {
    let request = serde_json::json!({ "CertificateArn": physical_resource_id });
    let reported = match call_json_rpc(
        ctx,
        ACM_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ACM_TARGET_PREFIX}.DescribeCertificate"),
        &request,
    )
    .await
    {
        Ok(reported) => reported,
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => {
            return Ok(Readiness::Failed(format!(
                "ACM holds no certificate for {physical_resource_id}"
            )))
        }
        Err(refusal) => return Err(refusal),
    };
    let detail = reported.get("Certificate");
    let status = detail
        .and_then(|detail| detail.get("Status"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    match status {
        CERTIFICATE_ISSUED => Ok(Readiness::Ready),
        CERTIFICATE_PENDING_VALIDATION => Ok(Readiness::Pending(
            CERTIFICATE_PENDING_VALIDATION.to_string(),
        )),
        other => {
            // `FailureReason` "exists only when the certificate status is FAILED", so it is
            // reported where ACM gave one and the status speaks for itself otherwise.
            let reason = detail
                .and_then(|detail| detail.get("FailureReason"))
                .and_then(Value::as_str);
            Ok(Readiness::Failed(match reason {
                Some(reason) => format!(
                    "ACM reports certificate {physical_resource_id} as {other}: {reason}"
                ),
                None => format!("ACM reports certificate {physical_resource_id} as {other}"),
            }))
        }
    }
}

/// Delete the certificate a stack requested.
///
/// A certificate ACM no longer holds (`ResourceNotFoundException`) is already in the state
/// the delete asks for, so that answer is a success. Every other refusal is ACM's own and
/// is reported: a certificate still in use by another service is its
/// `ResourceInUseException`, which is a delete that really did not happen.
pub(super) async fn delete_certificate(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "CertificateArn": physical_resource_id });
    match call_json_rpc(
        ctx,
        ACM_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ACM_TARGET_PREFIX}.DeleteCertificate"),
        &request,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The certificate's configuration as ACM reports it, for drift detection. An ARN ACM no
/// longer holds a certificate for (`ResourceNotFoundException`) was deleted.
///
/// Only the members `DescribeCertificate` reports in the SAME shape the type declares them
/// in are reported, because a property the actual document does not carry is not evidence of
/// anything (`super::drift::differs`): `DomainName`, `KeyAlgorithm` and
/// `CertificateAuthorityArn`, each the identical member on both sides.
/// `SubjectAlternativeNames` is not reported — ACM answers the whole name set of the
/// certificate, which carries the primary domain too and in an order ACM decides, so
/// comparing it would read ACM's own answer as drift — and neither is `Tags`, which that
/// read does not carry at all.
pub(super) async fn describe_certificate(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "CertificateArn": physical_resource_id });
    let reported = match call_json_rpc(
        ctx,
        ACM_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ACM_TARGET_PREFIX}.DescribeCertificate"),
        &request,
    )
    .await
    {
        Ok(reported) => reported,
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => return Ok(None),
        Err(refusal) => return Err(refusal),
    };
    let Some(detail) = reported.get("Certificate") else {
        return Ok(Some(Value::Object(Map::new())));
    };
    Ok(Some(reported_as_properties(
        detail,
        &[
            ("DomainName", "DomainName"),
            ("KeyAlgorithm", "KeyAlgorithm"),
            ("CertificateAuthorityArn", "CertificateAuthorityArn"),
        ],
    )))
}

// ---------------------------------------------------------------------------
// Amazon Web Services Private CA: AWS::ACMPCA::CertificateAuthority
// ---------------------------------------------------------------------------

/// Amazon Web Services Private CA's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: acm-pca`, service shape `ACMPrivateCA`), which speaks `awsJson1_1`.
pub(super) const ACMPCA_SIGNING_NAME: &str = "acm-pca";
pub(super) const ACMPCA_TARGET_PREFIX: &str = "ACMPrivateCA";

/// The properties of `AWS::ACMPCA::CertificateAuthority` that are members of the create's
/// `CertificateAuthorityConfiguration` — "name and bit size of the private key algorithm,
/// the name of the signing algorithm, and X.500 certificate subject information" — each
/// spelled identically on both sides, so the declared value travels as written
/// (`smithy_query(operation="CreateCertificateAuthority")`).
pub(super) const ACMPCA_CA_CONFIGURATION_MEMBERS: &[(&str, &str)] = &[
    ("KeyAlgorithm", "KeyAlgorithm"),
    ("SigningAlgorithm", "SigningAlgorithm"),
    ("Subject", "Subject"),
    ("CsrExtensions", "CsrExtensions"),
];

/// The type's properties that are the create's OWN members, spelled identically the same
/// way: the revocation mechanisms the CA supports, whether it issues general-purpose or
/// short-lived certificates, and the compliance standard its key is handled under.
pub(super) const ACMPCA_CA_MEMBERS: &[(&str, &str)] = &[
    ("RevocationConfiguration", "RevocationConfiguration"),
    ("UsageMode", "UsageMode"),
    ("KeyStorageSecurityStandard", "KeyStorageSecurityStandard"),
];

/// The one property whose name is not the member it reaches: the type publishes the CA's
/// kind as `Type` and the create takes it as `CertificateAuthorityType` (`ROOT` or
/// `SUBORDINATE`).
pub(super) const ACMPCA_CA_TYPE: &str = "Type";

/// The `CertificateAuthorityStatus` member a created CA is reported in while that service
/// is still making it ("Private CA is pending creation"), and the one a CA that could not
/// be created reaches.
pub(super) const CERTIFICATE_AUTHORITY_CREATING: &str = "CREATING";
pub(super) const CERTIFICATE_AUTHORITY_FAILED: &str = "FAILED";

/// Create the private CA an `AWS::ACMPCA::CertificateAuthority` declares, so a client that
/// reads the stack's physical resource id — or an output that exports it — holds the ARN
/// Amazon Web Services Private CA minted and can call that service with it
/// (`DescribeCertificateAuthority`, `IssueCertificate`, `PutPolicy`) rather than an
/// identifier no CA answers to.
///
/// The type's properties are the create's own members under the same names, in two groups:
/// the three that make up the CA's `CertificateAuthorityConfiguration`
/// ([`ACMPCA_CA_CONFIGURATION_MEMBERS`]) and the three the request carries itself
/// ([`ACMPCA_CA_MEMBERS`]), plus the CA's kind ([`ACMPCA_CA_TYPE`]) and the tag list the
/// create takes as its own member. Nothing the declaration does not state is sent.
///
/// The CA's identifier is that service's OWN to mint — the create takes no identifier and
/// answers with `CertificateAuthorityArn`, and "Ref returns the Amazon Resource Name (ARN)
/// of the private CA" — so that answer is reported back as the resource's physical id AND
/// as its `Arn`/`<Type>Arn` attributes, which is what `Fn::GetAtt` then resolves to.
pub(super) async fn create_certificate_authority(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let properties = resource
        .properties
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);
    let mut configuration = Map::new();
    copy_members(&properties, ACMPCA_CA_CONFIGURATION_MEMBERS, &mut configuration);
    let mut request = Map::new();
    request.insert(
        "CertificateAuthorityConfiguration".to_string(),
        Value::Object(configuration),
    );
    copy_members(&properties, ACMPCA_CA_MEMBERS, &mut request);
    if let Some(kind) = properties.get(ACMPCA_CA_TYPE) {
        request.insert("CertificateAuthorityType".to_string(), kind.clone());
    }
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("Tags".to_string(), tags_as_list(&tags, UPPER_CASED_TAG));
    }
    let reported = call_json_rpc(
        ctx,
        ACMPCA_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ACMPCA_TARGET_PREFIX}.CreateCertificateAuthority"),
        &Value::Object(request),
    )
    .await?;
    let Some(arn) = reported
        .get("CertificateAuthorityArn")
        .and_then(Value::as_str)
    else {
        // The create answered 2xx without the ARN its output models. Nothing addresses a
        // private CA without one, so this is reported as a failure rather than recorded as
        // a resource with an identifier nothing minted.
        return Err(Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: "CreateCertificateAuthority answered no CertificateAuthorityArn"
                .to_string(),
        });
    };
    // The type's OTHER published return value: the certification request that service
    // built for the CA, which is what an `AWS::ACMPCA::Certificate` of the same stack
    // signs. That service serves it once the CA it accepted is created, so the provider
    // asks until it does ([`certificate_authority_csr`]) rather than publishing an
    // attribute it has not read.
    let csr = certificate_authority_csr(ctx, region, arn).await?;
    let mut attributes: ::std::collections::BTreeMap<String, String> =
        super::template::arn_attributes(&resource.resource_type, arn)
            .into_iter()
            .collect();
    attributes.insert("CertificateSigningRequest".to_string(), csr);
    Ok(Created {
        physical_resource_id: Some(arn.to_string()),
        attributes,
    })
}

/// How long a provider waits for a value Amazon Web Services Private CA only has once work
/// it has already accepted has finished, and how often it asks for it.
///
/// Two of this service's values are owed on its own clock: the certification request of a
/// CA it is still creating (its `RequestInProgressException`, because the request is not
/// available while the CA is `CREATING`) and the body of a certificate it is still issuing.
/// A provider cannot publish a return value it has not read — a resource of the same stack
/// signs that request, and another imports that certificate — so it asks again until that
/// service answers, exactly as the AWS Batch arms wait for a resource of theirs
/// ([`BATCH_DELETION_CHECK_INTERVAL`]), and reports the wait running out as the failure it
/// is rather than publishing a value nothing produced.
pub(super) const ACMPCA_PENDING_VALUE_INTERVAL: ::std::time::Duration =
    ::std::time::Duration::from_millis(50);
pub(super) const ACMPCA_PENDING_VALUE_CHECKS: usize = 40;

/// The error code Amazon Web Services Private CA answers while work it accepted is still
/// going on.
pub(super) const ACMPCA_REQUEST_IN_PROGRESS: &str = "RequestInProgressException";

/// The certification request Amazon Web Services Private CA built for the CA `ca_arn`,
/// waited for while that service is still creating the CA.
pub(super) async fn certificate_authority_csr(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    ca_arn: &str,
) -> Result<String, Refusal> {
    let request = serde_json::json!({ "CertificateAuthorityArn": ca_arn });
    for _ in 0..ACMPCA_PENDING_VALUE_CHECKS {
        match call_json_rpc(
            ctx,
            ACMPCA_SIGNING_NAME,
            region,
            AWS_JSON_1_1,
            &format!("{ACMPCA_TARGET_PREFIX}.GetCertificateAuthorityCsr"),
            &request,
        )
        .await
        {
            Ok(reported) => {
                return reported
                    .get("Csr")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
                    .ok_or_else(|| Refusal {
                        code: UNIDENTIFIED_FAILURE.to_string(),
                        message: "GetCertificateAuthorityCsr answered no Csr".to_string(),
                    })
            }
            Err(refusal) if refusal.code == ACMPCA_REQUEST_IN_PROGRESS => {
                ::tokio::time::sleep(ACMPCA_PENDING_VALUE_INTERVAL).await;
            }
            Err(refusal) => return Err(refusal),
        }
    }
    Err(Refusal {
        code: ACMPCA_REQUEST_IN_PROGRESS.to_string(),
        message: format!(
            "Amazon Web Services Private CA did not answer the certificate signing request \
             of {ca_arn}"
        ),
    })
}

// ---------------------------------------------------------------------------
// Amazon Web Services Private CA: AWS::ACMPCA::Certificate
// ---------------------------------------------------------------------------

/// The properties of `AWS::ACMPCA::Certificate` that are members of `IssueCertificate`
/// under the same names: the CA asked to sign, the algorithm it signs with, the template
/// that says what KIND of certificate this is, how long it is valid for, and the X.509
/// fields the declaration passes through (`smithy_query(operation="IssueCertificate")`).
pub(super) const ACMPCA_CERTIFICATE_MEMBERS: &[(&str, &str)] = &[
    ("CertificateAuthorityArn", "CertificateAuthorityArn"),
    ("SigningAlgorithm", "SigningAlgorithm"),
    ("TemplateArn", "TemplateArn"),
    ("Validity", "Validity"),
    ("ValidityNotBefore", "ValidityNotBefore"),
    ("ApiPassthrough", "ApiPassthrough"),
];

/// Have the private CA a declaration names SIGN the certification request it carries, so
/// the stack really holds the certificate the declaration asks for.
///
/// `IssueCertificate` takes the request as a BLOB (`Csr`), while the type publishes it as
/// the PEM `CertificateSigningRequest` property — which is what an
/// `Fn::GetAtt <Ca>.CertificateSigningRequest` of the same stack resolves to — so the
/// property's text is what travels, base64 as the awsJson wire carries a blob.
///
/// The certificate's ARN is that service's OWN to mint (it carries the serial it
/// allocated), so it is reported as the resource's physical id and as its `Arn` attribute;
/// the BODY it signed is the type's other published return value, read back through
/// `GetCertificate` once that service has finished issuing it, because the resource that
/// ACTIVATES a CA with this certificate reads it off that attribute.
pub(super) async fn issue_certificate(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let properties = resource
        .properties
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);
    let mut request = Map::new();
    copy_members(&properties, ACMPCA_CERTIFICATE_MEMBERS, &mut request);
    let Some(csr) = properties
        .get("CertificateSigningRequest")
        .and_then(Value::as_str)
    else {
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: "AWS::ACMPCA::Certificate declares no CertificateSigningRequest to sign"
                .to_string(),
        });
    };
    request.insert(
        "Csr".to_string(),
        Value::String(::aws_smithy_types::base64::encode(csr.as_bytes())),
    );
    let ca_arn = properties
        .get("CertificateAuthorityArn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let reported = call_json_rpc(
        ctx,
        ACMPCA_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ACMPCA_TARGET_PREFIX}.IssueCertificate"),
        &Value::Object(request),
    )
    .await?;
    let Some(certificate_arn) = reported.get("CertificateArn").and_then(Value::as_str) else {
        return Err(Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: "IssueCertificate answered no CertificateArn".to_string(),
        });
    };
    let certificate = issued_certificate(ctx, region, &ca_arn, certificate_arn).await?;
    let mut attributes: ::std::collections::BTreeMap<String, String> =
        super::template::arn_attributes(&resource.resource_type, certificate_arn)
            .into_iter()
            .collect();
    attributes.insert("Certificate".to_string(), certificate);
    Ok(Created {
        physical_resource_id: Some(certificate_arn.to_string()),
        attributes,
    })
}

/// The body of the certificate `certificate_arn` names, waited for while that service is
/// still issuing it: issuance completes on that service's own clock, and `GetCertificate`
/// answers its `RequestInProgressException` until the certificate is retrievable.
pub(super) async fn issued_certificate(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    ca_arn: &str,
    certificate_arn: &str,
) -> Result<String, Refusal> {
    let request = serde_json::json!({
        "CertificateAuthorityArn": ca_arn,
        "CertificateArn": certificate_arn,
    });
    for _ in 0..ACMPCA_PENDING_VALUE_CHECKS {
        match call_json_rpc(
            ctx,
            ACMPCA_SIGNING_NAME,
            region,
            AWS_JSON_1_1,
            &format!("{ACMPCA_TARGET_PREFIX}.GetCertificate"),
            &request,
        )
        .await
        {
            Ok(reported) => {
                return reported
                    .get("Certificate")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
                    .ok_or_else(|| Refusal {
                        code: UNIDENTIFIED_FAILURE.to_string(),
                        message: "GetCertificate answered no Certificate".to_string(),
                    })
            }
            Err(refusal) if refusal.code == ACMPCA_REQUEST_IN_PROGRESS => {
                ::tokio::time::sleep(ACMPCA_PENDING_VALUE_INTERVAL).await;
            }
            Err(refusal) => return Err(refusal),
        }
    }
    Err(Refusal {
        code: ACMPCA_REQUEST_IN_PROGRESS.to_string(),
        message: format!("Amazon Web Services Private CA did not finish issuing {certificate_arn}"),
    })
}

// ---------------------------------------------------------------------------
// Amazon Web Services Private CA: AWS::ACMPCA::CertificateAuthorityActivation
// ---------------------------------------------------------------------------

/// The `CertificateAuthorityStatus` member a CA that holds its own certificate is in, which
/// is the status importing one leaves it in.
pub(super) const CERTIFICATE_AUTHORITY_ACTIVE: &str = "ACTIVE";
