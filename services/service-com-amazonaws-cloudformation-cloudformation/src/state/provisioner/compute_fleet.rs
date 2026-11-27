// SPDX-License-Identifier: CC0-1.0

//! The per-type provisioning arms of the FLEET types a stack launches instances with: the
//! Amazon EC2 LAUNCH TEMPLATE that holds the launch parameters, and the Amazon EC2 Auto
//! Scaling GROUP that launches from one. One file of [`super`]'s per-type arms, dispatched
//! to by its `Provisioner`.
//!
//! ## The rule this file carries
//!
//! A stack reports a resource created only where the service that OWNS the type really
//! holds it, and the identifiers and attributes the stack publishes for it are the ones that
//! service answered. These two types are how a template says "launch instances": the group
//! is the resource `autoscaling:DescribeAutoScalingGroups` answers for and the template the
//! one `ec2:DescribeLaunchTemplates` answers for, so a stack that reached `CREATE_COMPLETE`
//! with either declared is one whose client can address both.
//!
//! The two sides spell the same members differently, and each type is handled as its own API
//! spells it:
//!
//! * `AWS::EC2::LaunchTemplate`'s properties are `CreateLaunchTemplate`'s own members —
//!   `LaunchTemplateName`, `LaunchTemplateData` and `VersionDescription` — but the launch
//!   DATA is a deep structure, and Amazon EC2 speaks `ec2Query`: a structure's members
//!   follow its name (`LaunchTemplateData.ImageId`) and a LIST is flattened under its
//!   member's `xmlName`, indexed from 1 and with no `.member.` indirection
//!   (`LaunchTemplateData.SecurityGroupId.1`). [`push_ec2_member`] is that walk, so a
//!   declaration reaches EC2 as the document it declared rather than as one with its lists
//!   dropped;
//! * `AWS::AutoScaling::AutoScalingGroup`'s properties are `CreateAutoScalingGroup`'s own
//!   members, which is an `awsQuery` call like Amazon RDS's — so the evaluated document is
//!   walked by [`query_members`] — with three exceptions: the group's NAME, which
//!   CloudFormation decides and which is this type's physical id; its `Cooldown`, which that
//!   call takes as `DefaultCooldown`; and its `VPCZoneIdentifier`, which the type declares as
//!   a LIST of subnet ids where the call takes "one or more comma-separated subnet IDs" as
//!   one string.
//!
//! The group's tags are the one tag set here that is not a plain key/value pair: an Auto
//! Scaling group's `Tag` carries `PropagateAtLaunch` as well, "determines whether the tag is
//! added to new instances as they are launched in the group", so a declared tag keeps the
//! value its declaration gave it and a tag no declaration states carries none
//! ([`auto_scaling_group_tags`]). A group that already exists has its tag set reconciled
//! through the two calls that service publishes for one ([`retag_auto_scaling_group`]), since
//! `UpdateAutoScalingGroup` has no member for it.
//!
//! Two of the group's properties are no member of either of its own calls, and reach that
//! service through the pair IT publishes for that child of a group that exists: its lifecycle
//! HOOKS ([`lifecycle_hook_changes`]) and the Amazon SNS NOTIFICATIONS it sends about its own
//! events ([`notification_configuration_changes`]). Both walks build their calls before
//! anything is sent and hand them to the one sender ([`make_group_calls`]).

use super::*;

/// Amazon EC2 Auto Scaling's signing name and the API version a request names beside the
/// action, from its model (`sigv4Name: autoscaling`, service shape `AutoScaling_2011_01_01`,
/// `version: 2011-01-01`), which speaks `awsQuery` — so its calls ride on [`call_query`].
///
/// Shared with the scaling-policy arms ([`super::auto_scaling_policies`]), so the two sets of
/// calls to this service cannot be signed for two different endpoints.
pub(super) const AUTOSCALING_SIGNING_NAME: &str = "autoscaling";
pub(super) const AUTOSCALING_API_VERSION: &str = "2011-01-01";

/// The error code Amazon EC2 answers for a launch template it does not hold
/// (`InvalidLaunchTemplateId.NotFound`, that service's own API error code), which is a delete
/// asking for a state EC2 is already in.
const INVALID_LAUNCH_TEMPLATE_ID_NOT_FOUND: &str = "InvalidLaunchTemplateId.NotFound";

/// The `AWS::EC2::LaunchTemplate` properties `CreateLaunchTemplate` takes under the SAME
/// member name, and whether each is a scalar or the launch DATA structure.
///
/// Source: AWS's CloudFormation resource reference for the type against
/// `smithy_query(query="operation",operation="CreateLaunchTemplate",
/// model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/\
/// com-amazonaws-ec2-amazonec2.json")` — the type publishes `LaunchTemplateData`,
/// `LaunchTemplateName`, `TagSpecifications` and `VersionDescription`, and that call takes
/// each of them under its own name.
const EC2_LAUNCH_TEMPLATE_CREATE_MEMBERS: &[&str] = &["LaunchTemplateData", "VersionDescription"];

/// The `AWS::EC2::LaunchTemplate` properties the arm itself carries: the NAME CloudFormation
/// decides — "if you don't specify a name, CloudFormation generates a unique physical ID and
/// uses that ID for the name" — and the `TagSpecifications` of the TEMPLATE, whose tags reach
/// EC2 through the one call that tags a resource that exists ([`tag_ec2_resource`]), beside
/// the ones CloudFormation propagates.
const EC2_LAUNCH_TEMPLATE_PROPERTIES_NOT_SENT: &[&str] =
    &["LaunchTemplateName", "TagSpecifications"];

/// The `ResourceType` a `TagSpecifications` entry of an `AWS::EC2::LaunchTemplate` names the
/// template itself by: "to tag the launch template, the resource type must be
/// launch-template" (`CreateLaunchTemplate$TagSpecifications`). An entry naming any other
/// resource type tags the resources an instance LAUNCH creates, which the launch data's own
/// `TagSpecifications` carries, so it is not a tag of this resource.
const LAUNCH_TEMPLATE_RESOURCE_TYPE: &str = "launch-template";

/// The element name `ec2Query` flattens one LIST member under: its `xmlName` where the member
/// declares one, and the member's own name otherwise.
///
/// Source: the members of `RequestLaunchTemplateData` and of the structures it nests
/// (`smithy_query(query="shape",shape="RequestLaunchTemplateData")`): a list member carrying
/// an `xmlName` is flattened under THAT name (`SecurityGroupIds` -> `SecurityGroupId.1`,
/// `BlockDeviceMappings` -> `BlockDeviceMapping.1`), which is the protocol's rule, and one
/// carrying none under its own (`Ipv6Addresses.1.Ipv6Address`).
///
/// `pub(in crate::state)` because the arm that launches an `AWS::EC2::Instance`
/// (`crate::state::provisioning::ec2_instance`) writes the same protocol for the same
/// service's `RunInstances`, so the element names and the writer below are shared rather than
/// spelled a second way.
pub(in crate::state) fn ec2_list_element(member: &str) -> &str {
    match member {
        "BlockDeviceMappings" => "BlockDeviceMapping",
        "ElasticGpuSpecifications" => "ElasticGpuSpecification",
        "ElasticInferenceAccelerators" => "ElasticInferenceAccelerator",
        // A network interface's `Groups` is the same `SecurityGroupIdStringList` the launch
        // data's own `SecurityGroupIds` is, under that list's `SecurityGroupId` element name.
        "Groups" | "SecurityGroupIds" => "SecurityGroupId",
        "Ipv4Prefixes" => "Ipv4Prefix",
        "Ipv6Prefixes" => "Ipv6Prefix",
        "LicenseSpecifications" => "LicenseSpecification",
        "NetworkInterfaces" => "NetworkInterface",
        "SecondaryInterfaces" => "SecondaryInterface",
        "SecurityGroups" => "SecurityGroup",
        "TagSpecifications" => "TagSpecification",
        "Tags" => "Tag",
        other => other,
    }
}

/// Write ONE member of an `ec2Query` request: a scalar under `<prefix><member>`, a structure's
/// own members below that name, and a LIST flattened under the element name
/// [`ec2_list_element`] gives it, indexed from 1.
///
/// `prefix` is the path of the structure this member belongs to, ending in `.` (empty at the
/// top level of a request). The walk is bounded by [`MAX_PROPERTY_NESTING`] for the reason
/// that constant records: the document comes from a caller-supplied template, so one nested
/// past the bound is REFUSED rather than walked or quietly truncated.
/// `pub(in crate::state)` for the reason [`ec2_list_element`] records.
pub(in crate::state) fn push_ec2_member(
    parameters: &mut Vec<(String, String)>,
    prefix: &str,
    member: &str,
    declared: &Value,
    depth: usize,
) -> Result<(), Refusal> {
    match declared {
        Value::Array(entries) => {
            let element = ec2_list_element(member);
            for (index, entry) in entries.iter().enumerate() {
                let position = index + 1;
                push_ec2_value(
                    parameters,
                    &format!("{prefix}{element}.{position}"),
                    entry,
                    depth + 1,
                )?;
            }
            Ok(())
        }
        value => push_ec2_value(parameters, &format!("{prefix}{member}"), value, depth + 1),
    }
}

/// Write one VALUE of an `ec2Query` request at the path `path`: a structure's members below
/// it ([`push_ec2_member`]) and a scalar as the parameter itself.
fn push_ec2_value(
    parameters: &mut Vec<(String, String)>,
    path: &str,
    declared: &Value,
    depth: usize,
) -> Result<(), Refusal> {
    if depth >= MAX_PROPERTY_NESTING {
        return Err(too_deeply_nested());
    }
    match declared {
        Value::Object(members) => {
            for (member, value) in members {
                push_ec2_member(parameters, &format!("{path}."), member, value, depth)?;
            }
            Ok(())
        }
        // A list whose entries are themselves lists is no shape any member of these calls
        // declares, so there is no element name to index it under: it is refused rather than
        // flattened into something EC2 would read as a different member.
        Value::Array(_) => Err(too_deeply_nested()),
        scalar => {
            parameters.push((path.to_string(), attribute_text(scalar)));
            Ok(())
        }
    }
}

/// Create the launch template an `AWS::EC2::LaunchTemplate` declares, in Amazon EC2.
///
/// "Ref returns the launch template ID" — an id EC2 ASSIGNS (`lt-` and 17 hexadecimal
/// characters), which is what this answers as the resource's physical id, so the stack's
/// `Ref`, its outputs and the `LaunchTemplate` of an Auto Scaling group of the same template
/// all carry an id EC2 really holds a template for. The two version numbers the type
/// publishes are EC2's as well — the template's latest and default version — so both are read
/// off the create's own answer rather than computed.
pub(super) async fn create_ec2_launch_template(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut parameters = Vec::new();
    if let Some(properties) = resource.properties.as_object() {
        for (property, value) in properties {
            if EC2_LAUNCH_TEMPLATE_PROPERTIES_NOT_SENT.contains(&property.as_str()) {
                continue;
            }
            if !EC2_LAUNCH_TEMPLATE_CREATE_MEMBERS.contains(&property.as_str()) {
                return Err(property_not_sent(EC2_LAUNCH_TEMPLATE, property));
            }
            push_ec2_member(&mut parameters, "", property, value, 0)?;
        }
    }
    // `CreateLaunchTemplate$LaunchTemplateName` is required by that call while the type's own
    // `LaunchTemplateName` is optional, so the template is created under the name the stack
    // gave the resource.
    parameters.push(("LaunchTemplateName".to_string(), declared_name(resource)));
    let answered = call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "CreateLaunchTemplate",
        &parameters,
    )
    .await?;
    let reported = ec2_reported(&answered, "launchTemplate");
    let launch_template_id = reported
        .get("LaunchTemplateId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: "CreateLaunchTemplate answered no launchTemplateId".to_string(),
        })?
        .to_string();
    tag_ec2_resource(
        ctx,
        region,
        &launch_template_id,
        &launch_template_tags(&resource.properties, propagated),
    )
    .await?;
    // The attributes the type publishes: the id EC2 assigned (which this type's
    // `LaunchTemplateId` return value is) and the two version numbers EC2 reported for the
    // template it created.
    let mut attributes = BTreeMap::new();
    attributes.insert("LaunchTemplateId".to_string(), launch_template_id.clone());
    for attribute in ["DefaultVersionNumber", "LatestVersionNumber"] {
        if let Some(value) = reported
            .get(attribute)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            attributes.insert(attribute.to_string(), value.to_string());
        }
    }
    Ok(Created {
        physical_resource_id: Some(launch_template_id),
        attributes,
    })
}

/// The tags a launch template is given: the ones a `TagSpecifications` entry naming the
/// TEMPLATE itself declares ([`LAUNCH_TEMPLATE_RESOURCE_TYPE`]), beside the ones
/// CloudFormation propagates to every resource it creates.
///
/// Built through [`tags_of`], so the merge of declared and propagated tags is the one every
/// other arm makes: the entries' own tag lists are collected into the `Tags` spelling that
/// helper reads.
pub(super) fn launch_template_tags(
    properties: &Value,
    propagated: &[(String, String)],
) -> Vec<(String, String)> {
    let mut declared = Vec::new();
    let specifications = properties
        .get("TagSpecifications")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    for specification in specifications {
        let names_the_template = specification
            .get("ResourceType")
            .and_then(Value::as_str)
            .is_some_and(|resource_type| resource_type == LAUNCH_TEMPLATE_RESOURCE_TYPE);
        if !names_the_template {
            continue;
        }
        if let Some(tags) = specification.get("Tags") {
            declared.push(tags.clone());
        }
    }
    let collected: Vec<Value> = declared
        .into_iter()
        .flat_map(|tags| match tags {
            Value::Array(entries) => entries,
            single => vec![single],
        })
        .collect();
    tags_of(
        &Value::Object(
            [("Tags".to_string(), Value::Array(collected))]
                .into_iter()
                .collect(),
        ),
        propagated,
    )
}

/// Delete the launch template a stack created, addressed by the id EC2 assigned it — this
/// resource's physical id. A template EC2 no longer holds
/// (`InvalidLaunchTemplateId.NotFound`) is already in the state the delete asks for.
pub(super) async fn delete_ec2_launch_template(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    launch_template_id: &str,
) -> Result<(), Refusal> {
    let parameters = vec![(
        "LaunchTemplateId".to_string(),
        launch_template_id.to_string(),
    )];
    match call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "DeleteLaunchTemplate",
        &parameters,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == INVALID_LAUNCH_TEMPLATE_ID_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The launch template's configuration as Amazon EC2 reports it, for drift detection. A
/// template it no longer holds (`InvalidLaunchTemplateId.NotFound`) was deleted.
///
/// A launch template is reported as a structure whose scalar members are the template's own
/// identity and version numbers — its launch DATA is a version's, which this read does not
/// carry — so those members are the actual configuration the declared one is compared
/// against, exactly as a DB instance's are ([`describe_db_instance`]).
pub(super) async fn describe_ec2_launch_template(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    launch_template_id: &str,
) -> Result<Option<Value>, Refusal> {
    let parameters = vec![(
        "LaunchTemplateId.1".to_string(),
        launch_template_id.to_string(),
    )];
    match call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "DescribeLaunchTemplates",
        &parameters,
    )
    .await
    {
        Ok(answered) => Ok(Some(Value::Object(ec2_reported_entry(
            &answered,
            "launchTemplates",
        )))),
        Err(refusal) if refusal.code == INVALID_LAUNCH_TEMPLATE_ID_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The members `CreateAutoScalingGroup` takes, from Amazon EC2 Auto Scaling's own model
/// (`smithy_query(query="operation",operation="CreateAutoScalingGroup",
/// model_path="services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/\
/// smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json")`), with each list's
/// element name from its list shape's member `@xmlName` — none of these lists fixes one, so
/// every one carries the Smithy default, `member`.
///
/// This is the catalogue the declaration of an `AWS::AutoScaling::AutoScalingGroup` is walked
/// against: the type's properties ARE these members, so a property whose name is one of them
/// (or renames to one, [`AUTO_SCALING_GROUP_RENAMED_PROPERTIES`]) is passed through under it.
const CREATE_AUTO_SCALING_GROUP_MEMBERS: &[(&str, QueryMember)] = &[
    ("AvailabilityZoneDistribution", QueryMember::Structure),
    ("AvailabilityZoneIds", QueryMember::List("member")),
    ("AvailabilityZoneImpairmentPolicy", QueryMember::Structure),
    ("AvailabilityZones", QueryMember::List("member")),
    ("CapacityRebalance", QueryMember::Scalar),
    ("CapacityReservationSpecification", QueryMember::Structure),
    ("Context", QueryMember::Scalar),
    ("DefaultCooldown", QueryMember::Scalar),
    ("DefaultInstanceWarmup", QueryMember::Scalar),
    ("DeletionProtection", QueryMember::Scalar),
    ("DesiredCapacity", QueryMember::Scalar),
    ("DesiredCapacityType", QueryMember::Scalar),
    ("HealthCheckGracePeriod", QueryMember::Scalar),
    ("HealthCheckType", QueryMember::Scalar),
    ("InstanceId", QueryMember::Scalar),
    ("InstanceLifecyclePolicy", QueryMember::Structure),
    ("InstanceMaintenancePolicy", QueryMember::Structure),
    ("LaunchConfigurationName", QueryMember::Scalar),
    ("LaunchTemplate", QueryMember::Structure),
    ("LifecycleHookSpecificationList", QueryMember::List("member")),
    ("LoadBalancerNames", QueryMember::List("member")),
    ("MaxInstanceLifetime", QueryMember::Scalar),
    ("MaxSize", QueryMember::Scalar),
    ("MinSize", QueryMember::Scalar),
    ("MixedInstancesPolicy", QueryMember::Structure),
    ("NewInstancesProtectedFromScaleIn", QueryMember::Scalar),
    ("PlacementGroup", QueryMember::Scalar),
    ("ServiceLinkedRoleARN", QueryMember::Scalar),
    ("SkipZonalShiftValidation", QueryMember::Scalar),
    ("TargetGroupARNs", QueryMember::List("member")),
    ("TerminationPolicies", QueryMember::List("member")),
    ("TrafficSources", QueryMember::List("member")),
];

/// The one `AWS::AutoScaling::AutoScalingGroup` property whose `CreateAutoScalingGroup`
/// member is spelled differently: the type's `Cooldown` is that call's `DefaultCooldown`
/// ("the amount of time, in seconds, between one scaling activity ending and another one
/// starting"). Every other property is its member's own name.
const AUTO_SCALING_GROUP_RENAMED_PROPERTIES: &[(&str, &str)] = &[("Cooldown", "DefaultCooldown")];

/// The `AWS::AutoScaling::AutoScalingGroup` properties the walk does NOT turn into a
/// `CreateAutoScalingGroup` member, each for a reason of its own. Every other property the
/// catalogue does not carry is refused rather than dropped ([`query_members`]).
const AUTO_SCALING_GROUP_PROPERTIES_NOT_SENT: &[&str] = &[
    // The name CloudFormation decides, which is this resource's physical id ("Ref returns
    // the name of the Auto Scaling group"): the arm sets the member from that name, so the
    // walk must not also push the declared spelling of it.
    "AutoScalingGroupName",
    // The notifications the group sends about its own events, which are no member of that
    // create at all: each declared entry reaches that service through the one call it
    // publishes for the notifications of a group that exists
    // ([`notification_configuration_changes`]).
    NOTIFICATION_CONFIGURATIONS,
    // The tag list carries the declared tags AND the ones CloudFormation propagates, with
    // the `PropagateAtLaunch` an Auto Scaling group's tag carries, so the arm builds it out
    // of both ([`auto_scaling_group_tags`]).
    "Tags",
    // A LIST of subnet ids where the call takes them as one comma-separated string, so the
    // arm joins them ([`comma_separated_subnets`]).
    "VPCZoneIdentifier",
];

/// Create the Auto Scaling group an `AWS::AutoScaling::AutoScalingGroup` declares, in Amazon
/// EC2 Auto Scaling, and report the ARN that service built for it.
///
/// "Ref returns the name of the Auto Scaling group", which is the name the declaration states
/// or the one CloudFormation generated — so the group is created under this resource's own
/// physical id and no identifier is taken over. The group's ARN is Auto Scaling's, carrying
/// the resource id IT minted, so it is read back from that service rather than computed: the
/// read is also what makes the create's success a resource that service really holds.
///
/// The group's declared NOTIFICATIONS are provisioned by a call per declared entry after the
/// group itself is created ([`notification_configuration_changes`]), that property being no
/// member of the create: so a configuration the owning service refuses fails THIS resource
/// while leaving behind the group the create already made, with no configuration on it.
pub(super) async fn create_auto_scaling_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let name = declared_name(resource);
    let mut parameters = query_members(
        &resource.properties,
        CREATE_AUTO_SCALING_GROUP_MEMBERS,
        AUTO_SCALING_GROUP_RENAMED_PROPERTIES,
        AUTO_SCALING_GROUP_PROPERTIES_NOT_SENT,
        AUTOSCALING_GROUP,
    )?;
    parameters.push(("AutoScalingGroupName".to_string(), name.clone()));
    if let Some(subnets) = comma_separated_subnets(&resource.properties) {
        parameters.push(("VPCZoneIdentifier".to_string(), subnets));
    }
    parameters.extend(auto_scaling_group_tags(&resource.properties, propagated));
    // The notification calls this declaration owes, built BEFORE anything is sent, exactly as
    // the create's own parameters are: an entry carrying a member that call does not take
    // refuses the resource with nothing created at all
    // ([`notification_configuration_changes`]). A group that was never deployed holds no
    // configuration to remove, so these are writes alone.
    let notifications =
        notification_configuration_changes(&name, &Value::Null, &resource.properties)?;
    call_query(
        ctx,
        AUTOSCALING_SIGNING_NAME,
        region,
        AUTOSCALING_API_VERSION,
        "CreateAutoScalingGroup",
        &parameters,
    )
    .await?;
    make_group_calls(ctx, region, notifications).await?;
    let reported = describe_auto_scaling_group(ctx, region, &name)
        .await?
        .ok_or_else(|| Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: format!(
                "CreateAutoScalingGroup was accepted but Amazon EC2 Auto Scaling reports no \
                 group named {name}"
            ),
        })?;
    let mut attributes = BTreeMap::new();
    if let Some(arn) = reported
        .get("AutoScalingGroupARN")
        .and_then(Value::as_str)
        .filter(|arn| !arn.is_empty())
    {
        attributes.extend(super::template::arn_attributes(
            &resource.resource_type,
            arn,
        ));
    }
    Ok(Created::attributes(attributes))
}

/// The subnets an `AWS::AutoScaling::AutoScalingGroup` declares as its `VPCZoneIdentifier`,
/// as the one comma-separated string `CreateAutoScalingGroup` takes them as.
///
/// The type declares "a list of subnet IDs for a virtual private cloud (VPC) where instances
/// in the Auto Scaling group can be created" while the API member is "one or more
/// comma-separated subnet IDs", so the list is joined. A declaration that spells one subnet
/// without the sequence syntax is the one-entry list of itself, exactly as it is for every
/// other list property ([`push_list`]).
fn comma_separated_subnets(properties: &Value) -> Option<String> {
    let declared = properties.get("VPCZoneIdentifier")?;
    let subnets: Vec<String> = match declared {
        Value::Array(entries) => entries.iter().map(attribute_text).collect(),
        single => vec![attribute_text(single)],
    };
    let joined = subnets.join(",");
    (!joined.is_empty()).then_some(joined)
}

/// The `Tags` member of `CreateAutoScalingGroup`, as the awsQuery parameters that call takes
/// it in.
///
/// The tag set is the one [`tags_of`] builds for every type — the declared tags, in
/// declaration order, with the ones CloudFormation propagates after them — and each entry
/// also carries the `PropagateAtLaunch` its own declaration states, which is a member of this
/// service's `Tag` alone. A tag no declaration states carries none, so Auto Scaling applies
/// that member's own default rather than a value invented here.
fn auto_scaling_group_tags(
    properties: &Value,
    propagated: &[(String, String)],
) -> Vec<(String, String)> {
    let mut parameters = Vec::new();
    for (index, (key, value)) in tags_of(properties, propagated).iter().enumerate() {
        let position = index + 1;
        parameters.push((format!("Tags.member.{position}.Key"), key.clone()));
        parameters.push((format!("Tags.member.{position}.Value"), value.clone()));
        if let Some(propagate) = declared_propagate_at_launch(properties, key) {
            parameters.push((
                format!("Tags.member.{position}.PropagateAtLaunch"),
                propagate,
            ));
        }
    }
    parameters
}

/// The `PropagateAtLaunch` the declaration states for the tag keyed `key`, where it states
/// one: the member of an Auto Scaling group's own `Tag` that "determines whether the tag is
/// added to new instances as they are launched in the group".
fn declared_propagate_at_launch(properties: &Value, key: &str) -> Option<String> {
    let declared = properties.get("Tags")?.as_array()?;
    declared
        .iter()
        .find(|tag| tag.get("Key").and_then(Value::as_str) == Some(key))
        .and_then(|tag| tag.get("PropagateAtLaunch"))
        .map(attribute_text)
}

/// Delete the Auto Scaling group a stack created, addressed by the name it was created under
/// — this resource's physical id.
///
/// A group Auto Scaling no longer holds is already in the state the delete asks for, which is
/// read from that service's own report of the group rather than from the refusal: every
/// absent-group refusal of this API is the one `ValidationError` code, so the code alone
/// cannot tell an absent group from a request it refused for another reason.
///
/// `ForceDelete` is set because CloudFormation deletes the group AND the instances it
/// launched: without it, Auto Scaling refuses the delete of a group that still has instances
/// (`ResourceInUse`), and the stack would be left reporting a resource it could not remove.
pub(super) async fn delete_auto_scaling_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
) -> Result<(), Refusal> {
    if describe_auto_scaling_group(ctx, region, name).await?.is_none() {
        return Ok(());
    }
    let parameters = vec![
        ("AutoScalingGroupName".to_string(), name.to_string()),
        ("ForceDelete".to_string(), "true".to_string()),
    ];
    call_query(
        ctx,
        AUTOSCALING_SIGNING_NAME,
        region,
        AUTOSCALING_API_VERSION,
        "DeleteAutoScalingGroup",
        &parameters,
    )
    .await
    .map(|_| ())
}

/// The members `UpdateAutoScalingGroup` takes, from the same model: that call "updates the
/// configuration for the specified Auto Scaling group", and the members it does NOT carry
/// are the ones a group's configuration cannot be changed through
/// ([`AUTO_SCALING_GROUP_CREATE_ONLY_PROPERTIES`]).
const UPDATE_AUTO_SCALING_GROUP_MEMBERS: &[(&str, QueryMember)] = &[
    ("AvailabilityZoneDistribution", QueryMember::Structure),
    ("AvailabilityZoneIds", QueryMember::List("member")),
    ("AvailabilityZoneImpairmentPolicy", QueryMember::Structure),
    ("AvailabilityZones", QueryMember::List("member")),
    ("CapacityRebalance", QueryMember::Scalar),
    ("CapacityReservationSpecification", QueryMember::Structure),
    ("Context", QueryMember::Scalar),
    ("DefaultCooldown", QueryMember::Scalar),
    ("DefaultInstanceWarmup", QueryMember::Scalar),
    ("DeletionProtection", QueryMember::Scalar),
    ("DesiredCapacity", QueryMember::Scalar),
    ("DesiredCapacityType", QueryMember::Scalar),
    ("HealthCheckGracePeriod", QueryMember::Scalar),
    ("HealthCheckType", QueryMember::Scalar),
    ("InstanceLifecyclePolicy", QueryMember::Structure),
    ("InstanceMaintenancePolicy", QueryMember::Structure),
    ("LaunchConfigurationName", QueryMember::Scalar),
    ("LaunchTemplate", QueryMember::Structure),
    ("MaxInstanceLifetime", QueryMember::Scalar),
    ("MaxSize", QueryMember::Scalar),
    ("MinSize", QueryMember::Scalar),
    ("MixedInstancesPolicy", QueryMember::Structure),
    ("NewInstancesProtectedFromScaleIn", QueryMember::Scalar),
    ("PlacementGroup", QueryMember::Scalar),
    ("ServiceLinkedRoleARN", QueryMember::Scalar),
    ("SkipZonalShiftValidation", QueryMember::Scalar),
    ("TerminationPolicies", QueryMember::List("member")),
];

/// The `AWS::AutoScaling::AutoScalingGroup` properties no call this arm makes applies to a
/// group that EXISTS, so a declaration that changes one is asking for a change this emulator
/// does not make to the group: the instance a group was built from, which is only ever a
/// group's SOURCE, and the attachments, each of which Amazon EC2 Auto Scaling
/// changes through a pair of calls of its own (`AttachLoadBalancers`/`DetachLoadBalancers`,
/// `AttachLoadBalancerTargetGroups`/`DetachLoadBalancerTargetGroups`,
/// `AttachTrafficSources`/`DetachTrafficSources`) that no
/// arm here is written on. Such a change is REFUSED rather than reported applied: the group
/// would be left as it is, and a stack reporting the change as applied would be reporting a
/// group it does not have.
///
/// Named for what it decides — which changes no arm here applies — rather than "create-only",
/// because CloudFormation itself does change a group's attachments: what is missing is
/// an arm for them, not a call of that service's.
///
/// The group's TAG SET is not among them: a tag set is carried by no configuration-update call
/// of any owning service, and this one changes a group's tags through the pair it publishes for
/// a group that exists — so a changed declaration of them is applied after the configuration
/// change, by the re-tagging arm every other type's tags travel through
/// ([`retag_auto_scaling_group`]). Nor are the group's lifecycle HOOKS: that property has its
/// own pair of calls and this arm applies a changed declaration of it through them
/// ([`lifecycle_hook_changes`]).
const AUTO_SCALING_GROUP_CHANGES_NO_ARM_APPLIES: &[&str] = &[
    "InstanceId",
    "LoadBalancerNames",
    "TargetGroupARNs",
    "TrafficSources",
];

/// The `AWS::AutoScaling::AutoScalingGroup` properties an UPDATE does not write into the
/// `UpdateAutoScalingGroup` request: the group's NAME and its `VPCZoneIdentifier`, which this
/// arm pushes itself; the lifecycle hooks, which reach that service through their own two calls
/// ([`lifecycle_hook_changes`]); the NOTIFICATIONS, which reach it through their own two
/// calls ([`notification_configuration_changes`]); the tag set, which reaches that service
/// through the two
/// calls it publishes for the tags of a group that exists ([`retag_auto_scaling_group`]); and
/// the attachments and the source instance, which no member of that call carries at all. A
/// declaration that CHANGES one of the last two is refused before anything is sent
/// ([`AUTO_SCALING_GROUP_CHANGES_NO_ARM_APPLIES`]), so what is left out here is either applied
/// by a call of its own or a property this revision restates unchanged — which asks nothing of
/// the update.
const AUTO_SCALING_GROUP_PROPERTIES_NOT_UPDATED: &[&str] = &[
    "AutoScalingGroupName",
    "InstanceId",
    "LifecycleHookSpecificationList",
    "LoadBalancerNames",
    NOTIFICATION_CONFIGURATIONS,
    "Tags",
    "TargetGroupARNs",
    "TrafficSources",
    "VPCZoneIdentifier",
];

/// Apply a CHANGED `AWS::AutoScaling::AutoScalingGroup` declaration to the group that
/// exists, through `UpdateAutoScalingGroup` — "updates the configuration for the specified
/// Auto Scaling group".
///
/// A changed `LifecycleHookSpecificationList` is applied too, through the calls that write
/// and remove ONE hook of a group that exists ([`lifecycle_hook_changes`]): the group's
/// hooks are a property of it that the owning service changes after the group was created,
/// not part of what `UpdateAutoScalingGroup` carries. A changed `NotificationConfigurations`
/// travels the same way, through the two calls that configure and un-configure ONE topic of a
/// group that exists ([`notification_configuration_changes`]).
///
/// A changed `Tags` property is applied too, but not from here: a tag set is carried by no
/// member of this call ([`UPDATE_AUTO_SCALING_GROUP_MEMBERS`]), so it travels through the
/// owning service's own tagging calls after the configuration change, exactly as every other
/// type's tag set does ([`retag_auto_scaling_group`]).
///
/// A declaration that changes a property no call here applies is REFUSED
/// ([`AUTO_SCALING_GROUP_CHANGES_NO_ARM_APPLIES`]): the group would be left as it is, and a
/// stack reporting the change as applied would be reporting a group it does not have.
pub(super) async fn update_auto_scaling_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    if let Some(property) = AUTO_SCALING_GROUP_CHANGES_NO_ARM_APPLIES
        .iter()
        .find(|property| resource.properties.get(**property) != declared.get(**property))
    {
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!(
                "this emulator does not change the {property} of an existing {AUTOSCALING_GROUP}, \
                 which no member of UpdateAutoScalingGroup carries; the resource was left as it \
                 is."
            ),
        });
    }
    let mut parameters = query_members(
        declared,
        UPDATE_AUTO_SCALING_GROUP_MEMBERS,
        AUTO_SCALING_GROUP_RENAMED_PROPERTIES,
        AUTO_SCALING_GROUP_PROPERTIES_NOT_UPDATED,
        AUTOSCALING_GROUP,
    )?;
    let name = resource.physical_resource_id.clone();
    // The hook calls this revision owes, built BEFORE anything is sent, exactly as the
    // update's own parameters are: a specification this arm cannot carry refuses the whole
    // update with the group untouched rather than half-way through it
    // ([`lifecycle_hook_changes`]).
    let hooks = lifecycle_hook_changes(&name, &resource.properties, declared)?;
    // The notification calls, built before anything is sent for the same reason the hook
    // calls are ([`notification_configuration_changes`]).
    let notifications = notification_configuration_changes(&name, &resource.properties, declared)?;
    parameters.push(("AutoScalingGroupName".to_string(), name.clone()));
    if let Some(subnets) = comma_separated_subnets(declared) {
        parameters.push(("VPCZoneIdentifier".to_string(), subnets));
    }
    call_query(
        ctx,
        AUTOSCALING_SIGNING_NAME,
        region,
        AUTOSCALING_API_VERSION,
        "UpdateAutoScalingGroup",
        &parameters,
    )
    .await?;
    make_group_calls(ctx, region, hooks).await?;
    make_group_calls(ctx, region, notifications).await?;
    Ok(Some(Created::attributes(BTreeMap::new())))
}

/// The property an `AWS::AutoScaling::AutoScalingGroup` declares its lifecycle hooks in,
/// which is the member of the same name `CreateAutoScalingGroup` takes — "one or more
/// lifecycle hooks to add to the Auto Scaling group before instances are launched".
const LIFECYCLE_HOOK_SPECIFICATION_LIST: &str = "LifecycleHookSpecificationList";

/// The member that NAMES one hook, in the specification the type declares and in each of the
/// two calls that write and remove a hook of a group that exists.
const LIFECYCLE_HOOK_NAME: &str = "LifecycleHookName";

/// The members of one `LifecycleHookSpecification` beside the hook's name, every one of which
/// `PutLifecycleHook` takes under the SAME name.
///
/// Source: `smithy_query(query="shape",shape="LifecycleHookSpecification")` read against
/// `smithy_query(query="operation",operation="PutLifecycleHook")` on Amazon EC2 Auto Scaling's
/// own model — the specification's members are the hook's name, its `LifecycleTransition`, its
/// `NotificationMetadata`, its `HeartbeatTimeout`, its `DefaultResult`, its
/// `NotificationTargetARN` and its `RoleARN`, and that call takes each of them.
const PUT_LIFECYCLE_HOOK_MEMBERS: &[&str] = &[
    "DefaultResult",
    "HeartbeatTimeout",
    "LifecycleTransition",
    "NotificationMetadata",
    "NotificationTargetARN",
    "RoleARN",
];

/// The entries a declaration states under the list property `property`, in declaration order.
///
/// A declaration that spells ONE entry without the sequence syntax is the one-entry list of
/// itself, exactly as it is for every other list property ([`push_list`]), and a declaration
/// that states the property as an EMPTY list asks for a group with none of that child at all.
///
/// THE ONE reading of such a property for the children of the group that travel beside its own
/// calls — its lifecycle hooks and its notification configurations — so the two walks cannot
/// drift apart about what a declaration states.
fn declared_group_children(properties: &Value, property: &str) -> Vec<Value> {
    match properties.get(property) {
        Some(Value::Array(entries)) => entries.clone(),
        None | Some(Value::Null) => Vec::new(),
        Some(single) => vec![single.clone()],
    }
}

/// The text one entry states for `member`, or `None` where it states none.
///
/// An entry that names neither the hook nor the topic it is about is sent exactly as it was
/// declared, so the owning service's own validation of that required member is what answers for
/// it rather than a reading invented here. THE ONE such reading, for the reason
/// [`declared_group_children`] is the one reading of the list.
fn declared_child_member(entry: &Value, member: &str) -> Option<String> {
    entry
        .get(member)
        .map(attribute_text)
        .filter(|text| !text.is_empty())
}

/// The hook specifications a declaration states, in declaration order.
fn declared_lifecycle_hooks(properties: &Value) -> Vec<Value> {
    declared_group_children(properties, LIFECYCLE_HOOK_SPECIFICATION_LIST)
}

/// The name one specification gives its hook, or `None` where it states none.
fn lifecycle_hook_name(specification: &Value) -> Option<String> {
    declared_child_member(specification, LIFECYCLE_HOOK_NAME)
}

/// The calls one revision of a property the owning service provisions through calls of its
/// OWN owes Amazon EC2 Auto Scaling: the awsQuery parameters of each call, paired with the
/// action it is made under, in the order they are to be made.
///
/// Built by the walk of that property — the group's lifecycle hooks
/// ([`lifecycle_hook_changes`]) or its notification configurations
/// ([`notification_configuration_changes`]) — and made by the one sender
/// ([`make_group_calls`]). Each walk puts its REMOVALS first, so a revision that renames or
/// moves a child cannot run into that service's per-group ceiling on the way through.
type GroupCalls = Vec<(&'static str, Vec<(String, String)>)>;

/// The most hooks of one group this walk writes or removes, and so the most calls one stack
/// update fans out into per direction: "if you exceed your maximum limit of lifecycle hooks,
/// which by default is 50 per Auto Scaling group, the call fails" (`PutLifecycleHook`).
///
/// A declaration stating MORE than that asks for a group no account can have, and the owning
/// service refuses the first hook past its own ceiling — so the walk stops one specification
/// past the bound rather than fanning out the rest: the resource fails either way, with that
/// service's own message, and the number of in-process calls one template can ask for is
/// bounded here rather than by the template's size. The group cannot hold more than this many
/// hooks either, so no more removals than this can be owed.
const MAX_DECLARED_LIFECYCLE_HOOKS: usize = 50;

/// The calls that take the group's hooks from the ones the PREVIOUS revision declared to the
/// ones THIS one does, or the refusal a specification this arm cannot carry earns.
///
/// Built before the update sends anything, as every other refusal of a declaration here is
/// ([`query_members`]): a specification carrying a member that call does not take refuses the
/// update with the group untouched. What the owning service itself refuses mid-sequence — a
/// transition it does not admit, its per-group ceiling on hooks — fails the resource with that
/// service's own message, and the deployment reports the resource as failed and rolls the stack
/// back, exactly as it does for any other arm whose chain of calls stops part-way.
///
/// A hook the previous revision declared that this one no longer does is DELETED, and so is one
/// it KEEPS by name whose specification dropped a member ([`drops_a_member`]); every declared
/// specification is then written through the upsert `PutLifecycleHook` is — the same write the
/// specifications a group is CREATED with land through. A hook no revision of this declaration
/// states is left alone: it is not this resource's to take away. The deletes come first, so a
/// revision that renames a hook cannot run into that service's per-group ceiling on hooks on
/// the way through.
///
/// The declared specifications are indexed by hook NAME once, so what the two revisions have to
/// be compared over is one walk of each list rather than one walk of the new list per entry of
/// the old one, and each hook's name is read off its specification once. Both lists are bounded
/// by [`MAX_DECLARED_LIFECYCLE_HOOKS`], so the work and the calls this builds are bounded for
/// any template.
fn lifecycle_hook_changes(
    name: &str,
    deployed: &Value,
    declared: &Value,
) -> Result<GroupCalls, Refusal> {
    let group = || ("AutoScalingGroupName".to_string(), name.to_string());
    let wanted = declared_lifecycle_hooks(declared);
    // One specification per hook NAME, so the comparison below is a lookup rather than a scan.
    // A declaration that names one hook twice states its last specification for it, which is
    // the hook that specification would leave behind.
    let wanted_by_name: BTreeMap<String, &Value> = wanted
        .iter()
        .take(MAX_DECLARED_LIFECYCLE_HOOKS + 1)
        .filter_map(|specification| {
            lifecycle_hook_name(specification).map(|hook| (hook, specification))
        })
        .collect();
    let mut calls: GroupCalls = Vec::new();
    for held in declared_lifecycle_hooks(deployed)
        .iter()
        .take(MAX_DECLARED_LIFECYCLE_HOOKS)
    {
        let Some(hook) = lifecycle_hook_name(held) else {
            continue;
        };
        let kept = wanted_by_name.get(&hook);
        if kept.is_none_or(|specification| drops_a_member(held, specification)) {
            calls.push((
                "DeleteLifecycleHook",
                vec![group(), (LIFECYCLE_HOOK_NAME.to_string(), hook)],
            ));
        }
    }
    for specification in wanted.iter().take(MAX_DECLARED_LIFECYCLE_HOOKS + 1) {
        let mut parameters = vec![group()];
        for (member, value) in specification.as_object().into_iter().flatten() {
            // A member that call does not take is REFUSED rather than dropped, exactly as a
            // property outside the create's catalogue is ([`query_members`]): a hook written
            // without what its specification asked for is configured differently from what
            // the template declared.
            if member != LIFECYCLE_HOOK_NAME
                && !PUT_LIFECYCLE_HOOK_MEMBERS.contains(&member.as_str())
            {
                return Err(property_not_sent(AUTOSCALING_GROUP, member));
            }
            parameters.push((member.clone(), attribute_text(value)));
        }
        calls.push(("PutLifecycleHook", parameters));
    }
    Ok(calls)
}

/// Whether the specification `held` stated a member the revision's `wanted` no longer does.
///
/// Such a hook is DELETED before it is written again: `PutLifecycleHook` "creates or updates a
/// lifecycle hook" and leaves a member the request omits at the value the hook already carries,
/// so the upsert alone cannot take a member AWAY — and a group keeping a value its template no
/// longer declares is configured differently from what the template describes. Written again
/// from the specification alone, every member the revision dropped takes the value a hook
/// created without it carries, which is the same reading the in-place arms of the other
/// services make of a property a revision dropped ([`ec2_in_place::update_ec2_vpc`]).
fn drops_a_member(held: &Value, wanted: &Value) -> bool {
    held.as_object()
        .into_iter()
        .flatten()
        .any(|(member, _)| wanted.get(member).is_none())
}

/// Make the calls a walk of one property of the group built, in the order it built them:
/// each of the per-child calls Amazon EC2 Auto Scaling publishes for a group that EXISTS —
/// the two that write and remove one lifecycle hook ([`lifecycle_hook_changes`]), and the two
/// that configure and un-configure one notification topic
/// ([`notification_configuration_changes`]).
///
/// THE ONE copy of "make this sequence of calls to the owning service", so a property that
/// travels beside the group's own create or update cannot be sent over a second path of its
/// own, and a call refused mid-sequence fails the resource the same way whichever walk built
/// it.
///
/// Idempotent, like every other in-place arm: a revision that restates what it already
/// declared re-writes exactly that, which is what those upserts are for.
async fn make_group_calls(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    calls: GroupCalls,
) -> Result<(), Refusal> {
    for (action, parameters) in &calls {
        call_query(
            ctx,
            AUTOSCALING_SIGNING_NAME,
            region,
            AUTOSCALING_API_VERSION,
            action,
            parameters,
        )
        .await?;
    }
    Ok(())
}

/// The property an `AWS::AutoScaling::AutoScalingGroup` declares the Amazon SNS notifications
/// of its own events in, and the two members of ONE entry of it: the topic that entry
/// notifies and the events it is told about.
///
/// Source: AWS's CloudFormation resource reference for the type read against
/// `smithy_query(query="operation",operation="PutNotificationConfiguration",
/// model_path="services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/\
/// smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json")` — the type publishes
/// `NotificationConfigurations`, a list whose entry carries a `TopicARN` and the
/// `NotificationTypes` that topic is to be sent, and that call takes each of them under its
/// own name beside the group's.
const NOTIFICATION_CONFIGURATIONS: &str = "NotificationConfigurations";
const NOTIFICATION_TOPIC_ARN: &str = "TopicARN";
const NOTIFICATION_TYPES: &str = "NotificationTypes";

/// The most notification configurations of one group this walk writes or removes, and so the
/// most calls one deployment fans out into per direction: "if you exceed your maximum limit of
/// SNS topics, which is 10 per Auto Scaling group, the call fails"
/// (`PutNotificationConfiguration`).
///
/// A declaration stating MORE than that asks for a group no account can have, and the owning
/// service refuses the first topic past its own ceiling — so the walk stops one entry past the
/// bound rather than fanning out the rest, exactly as the hook walk does
/// ([`MAX_DECLARED_LIFECYCLE_HOOKS`]). The group cannot hold more than this many
/// configurations either, so no more removals than this can be owed.
const MAX_DECLARED_NOTIFICATION_CONFIGURATIONS: usize = 10;

/// The notification entries a declaration states, in declaration order.
fn declared_notification_configurations(properties: &Value) -> Vec<Value> {
    declared_group_children(properties, NOTIFICATION_CONFIGURATIONS)
}

/// The topic one entry names, or `None` where it names none.
fn notification_topic_arn(entry: &Value) -> Option<String> {
    declared_child_member(entry, NOTIFICATION_TOPIC_ARN)
}

/// The calls that take the group's notifications from the ones the PREVIOUS revision declared
/// to the ones THIS one does, or the refusal an entry this arm cannot carry earns.
///
/// This property is no member of either of the group's own calls: Amazon EC2 Auto Scaling
/// configures the notifications of a group that EXISTS, through `PutNotificationConfiguration`
/// — "configures an Auto Scaling group to send notifications when specified events take place
/// […] this configuration overwrites any existing configuration" — and
/// `DeleteNotificationConfiguration`, which "deletes the specified notification". So a CREATE
/// provisions each declared entry by a call of its own after the group is there, and an UPDATE
/// reconciles the two revisions through the same pair.
///
/// Built before anything is sent, as every other refusal of a declaration here is
/// ([`query_members`]): an entry carrying a member neither call takes refuses the deployment
/// with nothing sent. What the owning service itself refuses mid-sequence — an event type it
/// does not notify about, a value that names no Amazon SNS topic, its per-group ceiling on
/// topics — fails the resource with that service's own message, the group left as the call
/// that created it made it.
///
/// A topic the previous revision declared that this one no longer does is UN-CONFIGURED, and
/// every declared entry is then written through the upsert that call is: the write REPLACES
/// that topic's event types, so no entry has to be deleted first for a revision that only
/// narrows the events it asks for — and a declaration naming one topic in two entries
/// configures ONE topic, with the event types of the entry written last. A topic no revision of
/// this declaration states is left alone: it is not this resource's to take away. The removals
/// come first, and each topic is removed ONCE however many entries of the previous revision
/// named it, since what is there to take away is the one configuration those entries left; the
/// order keeps a revision that moves a configuration from one topic to another from running
/// into that service's per-group ceiling on the way through.
///
/// Both lists are bounded by [`MAX_DECLARED_NOTIFICATION_CONFIGURATIONS`], so the work and the
/// calls this builds are bounded for any template.
fn notification_configuration_changes(
    name: &str,
    deployed: &Value,
    declared: &Value,
) -> Result<GroupCalls, Refusal> {
    let group = || ("AutoScalingGroupName".to_string(), name.to_string());
    let wanted = declared_notification_configurations(declared);
    let wanted_topics: Vec<String> = wanted
        .iter()
        .take(MAX_DECLARED_NOTIFICATION_CONFIGURATIONS + 1)
        .filter_map(notification_topic_arn)
        .collect();
    let mut calls: GroupCalls = Vec::new();
    let mut removed: Vec<String> = Vec::new();
    for held in declared_notification_configurations(deployed)
        .iter()
        .take(MAX_DECLARED_NOTIFICATION_CONFIGURATIONS)
    {
        let Some(topic_arn) = notification_topic_arn(held) else {
            continue;
        };
        if !wanted_topics.contains(&topic_arn) && !removed.contains(&topic_arn) {
            removed.push(topic_arn);
        }
    }
    for topic_arn in removed {
        calls.push((
            "DeleteNotificationConfiguration",
            vec![group(), (NOTIFICATION_TOPIC_ARN.to_string(), topic_arn)],
        ));
    }
    for entry in wanted
        .iter()
        .take(MAX_DECLARED_NOTIFICATION_CONFIGURATIONS + 1)
    {
        let mut parameters = vec![group()];
        for (member, value) in entry.as_object().into_iter().flatten() {
            match member.as_str() {
                // The topic is one value and the event types a LIST, written as every other
                // awsQuery list member of this crate is ([`push_list`]): indexed from 1 under
                // the element name its list shape carries, which is the Smithy default here.
                NOTIFICATION_TOPIC_ARN => {
                    parameters.push((member.clone(), attribute_text(value)));
                }
                NOTIFICATION_TYPES => {
                    push_list(&mut parameters, NOTIFICATION_TYPES, "member", value, 0)?;
                }
                // A member that call does not take is REFUSED rather than dropped, exactly as
                // a property outside the create's catalogue is ([`query_members`]): a group
                // configured without what its declaration asked for notifies differently from
                // what the template declared.
                _ => return Err(property_not_sent(AUTOSCALING_GROUP, member)),
            }
        }
        calls.push(("PutNotificationConfiguration", parameters));
    }
    Ok(calls)
}

/// The `ResourceType` one tag of a tagging call names the group by: "the only supported value
/// is `auto-scaling-group`" (`Tag$ResourceType`).
const GROUP_TAG_RESOURCE_TYPE: &str = "auto-scaling-group";

/// One tag the Auto Scaling group is to end up carrying: its key, its value, and the
/// `PropagateAtLaunch` stated for it where one is stated at all.
///
/// That third member is this service's `Tag` alone — "determines whether the tag is added to
/// new instances as they are launched in the group" — so the pair every other service's tag set
/// is reconciled as ([`tag_delta`]) cannot carry it, and the arm below reconciles the triple. A
/// tag no declaration states the flag for carries `None`: its propagation is then the owning
/// service's own default for that member rather than a value invented here, exactly as it is
/// for the tags the CREATE applies ([`auto_scaling_group_tags`]).
type GroupTag = (String, String, Option<String>);

/// The tags this deployment gives the group: the set every type's tags are read as ([`tags_of`]
/// — the declaration's own, in declaration order, then the ones CloudFormation propagates under
/// every key it does not use), each carrying the propagation decision its own declaration
/// states.
fn auto_scaling_group_desired_tags(
    declared: &Value,
    propagated: &[(String, String)],
) -> Vec<GroupTag> {
    tags_of(declared, propagated)
        .into_iter()
        .map(|(key, value)| {
            let propagate = declared_propagate_at_launch(declared, &key);
            (key, value, propagate)
        })
        .collect()
}

/// Whether the group has to be WRITTEN the tag `wanted`: one it does not carry at all, one it
/// carries with another value, or one whose declaration states a propagation decision differing
/// from the one in force.
///
/// A declaration stating no `PropagateAtLaunch` for a key asks nothing about that member, so the
/// tag it already carries is left as it is rather than re-written to a flag chosen here.
fn group_tag_needs_writing(held: &[GroupTag], wanted: &GroupTag) -> bool {
    let (key, value, propagate) = wanted;
    match held.iter().find(|(held_key, _, _)| held_key == key) {
        None => true,
        Some((_, held_value, held_propagate)) => {
            held_value != value
                || propagate
                    .as_ref()
                    .is_some_and(|stated| Some(stated) != held_propagate.as_ref())
        }
    }
}

/// The two members that say WHICH resource one tag of a tagging call is on, beside the KEY that
/// identifies the tag itself: the group's name ("the name of the Auto Scaling group",
/// `Tag$ResourceId`) and the one resource type this service tags
/// ([`GROUP_TAG_RESOURCE_TYPE`]). Both calls below take the tag the same way, so the three are
/// pushed once here.
fn push_group_tag_identity(
    parameters: &mut Vec<(String, String)>,
    position: usize,
    name: &str,
    key: &str,
) {
    parameters.push((
        format!("Tags.member.{position}.ResourceId"),
        name.to_string(),
    ));
    parameters.push((
        format!("Tags.member.{position}.ResourceType"),
        GROUP_TAG_RESOURCE_TYPE.to_string(),
    ));
    parameters.push((format!("Tags.member.{position}.Key"), key.to_string()));
}

/// Bring the tags of the Auto Scaling group a stack already has to the set this deployment
/// gives it, through the two calls Amazon EC2 Auto Scaling publishes for the tags of a group
/// that EXISTS: `CreateOrUpdateTags` "creates or updates tags for the specified Auto Scaling
/// group" — "when you specify a tag with a key that already exists, the operation overwrites
/// the previous tag definition" — and `DeleteTags` "deletes the specified tags".
///
/// This is where a group's tag set is reconciled because no member of `UpdateAutoScalingGroup`
/// carries one ([`UPDATE_AUTO_SCALING_GROUP_MEMBERS`]): the `Tags` the create passed apply to
/// the group that call created and to no group that already exists, and a deployment can state
/// a stack's tags differently with no declaration changing at all — the position every other
/// type whose service tags a resource through calls of its own is in
/// ([`super::Provisioner::retag`]).
///
/// What the group HOLDS is read back from that service rather than guessed from the declaration
/// it was last deployed with, so the two halves are computed against the tags really in force;
/// where they already agree neither call is made at all, which is what makes this safe to ask
/// on every deployment.
pub(super) async fn retag_auto_scaling_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
    propagated: &[(String, String)],
) -> Result<(), Refusal> {
    let name = resource.physical_resource_id.clone();
    let desired = auto_scaling_group_desired_tags(declared, propagated);
    let held = described_group_tags(ctx, region, &name).await?;
    // A key the group carries that this deployment no longer gives it is REMOVED first, so a
    // revision that renames a tag cannot run into the account's per-group tag quota on the way
    // through.
    let removed: Vec<&String> = held
        .iter()
        .map(|(key, _, _)| key)
        .filter(|key| !desired.iter().any(|(wanted, _, _)| wanted == *key))
        .collect();
    if !removed.is_empty() {
        let mut parameters = Vec::new();
        for (index, key) in removed.iter().enumerate() {
            push_group_tag_identity(&mut parameters, index + 1, &name, key);
        }
        call_query(
            ctx,
            AUTOSCALING_SIGNING_NAME,
            region,
            AUTOSCALING_API_VERSION,
            "DeleteTags",
            &parameters,
        )
        .await?;
    }
    let written: Vec<&GroupTag> = desired
        .iter()
        .filter(|wanted| group_tag_needs_writing(&held, wanted))
        .collect();
    if written.is_empty() {
        return Ok(());
    }
    let mut parameters = Vec::new();
    for (index, (key, value, propagate)) in written.iter().enumerate() {
        let position = index + 1;
        push_group_tag_identity(&mut parameters, position, &name, key);
        parameters.push((format!("Tags.member.{position}.Value"), value.clone()));
        if let Some(propagate) = propagate {
            parameters.push((
                format!("Tags.member.{position}.PropagateAtLaunch"),
                propagate.clone(),
            ));
        }
    }
    call_query(
        ctx,
        AUTOSCALING_SIGNING_NAME,
        region,
        AUTOSCALING_API_VERSION,
        "CreateOrUpdateTags",
        &parameters,
    )
    .await
    .map(|_| ())
}

/// The tags Amazon EC2 Auto Scaling holds for one group right now, as the triples the arm above
/// reconciles.
///
/// Read off the GROUP's own report — `DescribeAutoScalingGroups` carries "the tags for the Auto
/// Scaling group" as that group's `Tags` list, one `<member>` per tag — so the reconciliation
/// needs no second read and sees exactly what a client reading the group sees. The list is read
/// through the one reader of a list of STRUCTURES this crate has ([`xml_member_structures`]), so
/// a tag reporting one member and not another is read as that tag with the other absent rather
/// than as a pairing of one tag's key with another's value. A group carrying no tag at all
/// reports the empty list, and so does a name that service holds no group for.
async fn described_group_tags(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
) -> Result<Vec<GroupTag>, Refusal> {
    let parameters = vec![(
        "AutoScalingGroupNames.member.1".to_string(),
        name.to_string(),
    )];
    let answered = call_query(
        ctx,
        AUTOSCALING_SIGNING_NAME,
        region,
        AUTOSCALING_API_VERSION,
        "DescribeAutoScalingGroups",
        &parameters,
    )
    .await?;
    Ok(xml_member_structures(&answered, "Tags")
        .into_iter()
        .filter_map(|tag| {
            let key = tag
                .get("Key")
                .and_then(Value::as_str)
                .filter(|key| !key.is_empty())?;
            Some((
                key.to_string(),
                tag.get("Value")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                tag.get("PropagateAtLaunch")
                    .and_then(Value::as_str)
                    .filter(|propagate| !propagate.is_empty())
                    .map(str::to_string),
            ))
        })
        .collect())
}

/// Apply a CHANGED `AWS::EC2::LaunchTemplate` declaration to the template that exists, by
/// creating a NEW VERSION of it — `CreateLaunchTemplateVersion` "creates a new version of a
/// launch template" from the launch data the call carries.
///
/// That is the change the type publishes: its `LaunchTemplateData` is updatable with no
/// interruption, a launch template being a versioned document, so the template keeps the
/// `lt-` id it was created under and its LATEST version moves — which is what an Auto Scaling
/// group reading `LatestVersionNumber` launches from next.
///
/// The template's DEFAULT version is left exactly where it is: nothing in this type's
/// declaration states it, and the version that is default is what "applies when a launch
/// template version is not specified" (`ModifyLaunchTemplate`) — so a deployment that moved it
/// would re-point every launch that names no version, which the template never asked for. Both
/// numbers are read off the template Amazon EC2 then holds rather than counted here.
///
/// A declaration whose launch DATA and version description are unchanged asks for no new
/// version: only the template's TAGS can have changed, and those reach EC2 through the one
/// call that tags a resource that exists ([`super::Provisioner::retag`]).
pub(super) async fn update_ec2_launch_template(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    let mut parameters = Vec::new();
    for member in EC2_LAUNCH_TEMPLATE_CREATE_MEMBERS {
        if let Some(value) = declared.get(member) {
            push_ec2_member(&mut parameters, "", member, value, 0)?;
        }
    }
    let same_version = EC2_LAUNCH_TEMPLATE_CREATE_MEMBERS
        .iter()
        .all(|member| resource.properties.get(member) == declared.get(member));
    if same_version {
        return Ok(Some(Created::attributes(BTreeMap::new())));
    }
    // The template is addressed by the id EC2 minted for it, which is this resource.
    parameters.push((
        "LaunchTemplateId".to_string(),
        resource.physical_resource_id.clone(),
    ));
    // Each call is held in a box of its own, as every call a walk this deep makes is: what the
    // deployment keeps on the stack for this arm is then its own bookkeeping rather than
    // Amazon EC2's whole request chain ([`provisioning::reconcile`]).
    Box::pin(call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "CreateLaunchTemplateVersion",
        &parameters,
    ))
    .await?;
    // The two version numbers the type publishes, as the template EC2 now holds reports them:
    // read back from that service, the create of a version answering the VERSION it made
    // rather than the template's own numbers.
    let described = Box::pin(call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "DescribeLaunchTemplates",
        &[(
            "LaunchTemplateId.1".to_string(),
            resource.physical_resource_id.clone(),
        )],
    ))
    .await?;
    let reported = ec2_reported_entry(&described, "launchTemplates");
    let mut attributes = BTreeMap::new();
    for attribute in ["DefaultVersionNumber", "LatestVersionNumber"] {
        if let Some(value) = reported
            .get(attribute)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            attributes.insert(attribute.to_string(), value.to_string());
        }
    }
    Ok(Some(Created::attributes(attributes)))
}

/// The lifecycle state Amazon EC2 Auto Scaling reports an instance of a group in once that
/// instance is serving: "`InService` — The instance is running and in service"
/// (`Instance$LifecycleState`), which is the state a group's own capacity is counted in.
const LIFECYCLE_IN_SERVICE: &str = "InService";

/// The ids of the Amazon EC2 instances the group `name` reports IN SERVICE right now, in the
/// order that service reports them.
///
/// `DescribeAutoScalingGroups` reports a group's `Instances` as one `<member>` per instance,
/// each carrying that instance's own `InstanceId` and `LifecycleState`, so the list is read
/// through the one reader of a list of STRUCTURES this crate has
/// ([`xml_member_structures`]): each entry is read on its own, so an entry that carries one of
/// the two members and not the other is an entry whose other member is absent, and not a
/// pairing of one instance's id with another's state. The empty list is what a group holding
/// no serving instance reports — and what a name that service holds no group for reports,
/// there being no instance to name for it.
pub(crate) async fn in_service_instance_ids(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
) -> Result<Vec<String>, Refusal> {
    let parameters = vec![(
        "AutoScalingGroupNames.member.1".to_string(),
        name.to_string(),
    )];
    let answered = call_query(
        ctx,
        AUTOSCALING_SIGNING_NAME,
        region,
        AUTOSCALING_API_VERSION,
        "DescribeAutoScalingGroups",
        &parameters,
    )
    .await?;
    Ok(xml_member_structures(&answered, "Instances")
        .into_iter()
        .filter(|instance| {
            instance.get("LifecycleState").and_then(Value::as_str) == Some(LIFECYCLE_IN_SERVICE)
        })
        .filter_map(|instance| {
            instance
                .get("InstanceId")
                .and_then(Value::as_str)
                .filter(|instance_id| !instance_id.is_empty())
                .map(str::to_string)
        })
        .collect())
}

/// What the group `name` holds right now, as the BATCHED replacement of its instances reads it
/// ([`crate::state::provisioning::rolling_update`]): the desired capacity and the maximum size
/// that service reports for it, and every instance it reports, each with its own
/// `Instance$LifecycleState`.
///
/// `None` is that service reporting no group of that name at all, which is not the same answer
/// as a group holding no instance: nothing is replaced in a group that is not there.
///
/// One call for both halves, read as [`in_service_instance_ids`] reads the same answer: the
/// group's scalar members carry its `DesiredCapacity` — "the desired size of the group" — and
/// its `Instances` are one `<member>` per instance, each read on its own so no entry's id is
/// paired with another entry's state.
pub(crate) async fn group_instance_states(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
) -> Result<Option<(usize, usize, Vec<(String, String)>)>, Refusal> {
    let parameters = vec![(
        "AutoScalingGroupNames.member.1".to_string(),
        name.to_string(),
    )];
    let answered = call_query(
        ctx,
        AUTOSCALING_SIGNING_NAME,
        region,
        AUTOSCALING_API_VERSION,
        "DescribeAutoScalingGroups",
        &parameters,
    )
    .await?;
    let reported = xml_scalar_children(&answered, "member");
    if reported.get("AutoScalingGroupName").and_then(Value::as_str) != Some(name) {
        return Ok(None);
    }
    let size = |member: &str| {
        reported
            .get(member)
            .and_then(|capacity| match capacity {
                Value::Number(number) => number.as_u64(),
                Value::String(text) => text.trim().parse().ok(),
                _ => None,
            })
            .and_then(|capacity| usize::try_from(capacity).ok())
            .unwrap_or_default()
    };
    let instances = xml_member_structures(&answered, "Instances")
        .into_iter()
        .filter_map(|instance| {
            let instance_id = instance
                .get("InstanceId")
                .and_then(Value::as_str)
                .filter(|instance_id| !instance_id.is_empty())?
                .to_string();
            let state = instance
                .get("LifecycleState")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            Some((instance_id, state))
        })
        .collect();
    Ok(Some((size("DesiredCapacity"), size("MaxSize"), instances)))
}

/// Take the instances `instance_ids` of the group `name` out of service so the group launches
/// their REPLACEMENTS, which is the one call a batch of a rolling update makes.
///
/// `ShouldDecrementDesiredCapacity` is false because the group keeps its size: "if you do not
/// specify the option to decrement the desired capacity, Amazon EC2 Auto Scaling launches
/// instances to replace the ones that are terminated"
/// (`TerminateInstanceInAutoScalingGroup`) — which is what replaces a batch's instances with
/// ones launched from the configuration the update put in force.
pub(crate) async fn terminate_group_instances(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
    instance_ids: &[String],
) -> Result<(), Refusal> {
    let mut parameters = vec![
        ("AutoScalingGroupName".to_string(), name.to_string()),
        (
            "ShouldDecrementDesiredCapacity".to_string(),
            "false".to_string(),
        ),
    ];
    for (index, instance_id) in instance_ids.iter().enumerate() {
        parameters.push((
            format!("InstanceIds.member.{}", index + 1),
            instance_id.clone(),
        ));
    }
    call_query(
        ctx,
        AUTOSCALING_SIGNING_NAME,
        region,
        AUTOSCALING_API_VERSION,
        "TerminateInstanceInAutoScalingGroup",
        &parameters,
    )
    .await
    .map(|_| ())
}

/// Ask the group `name` to hold `capacity` instances, which is what a batch that has to LAUNCH
/// before it terminates needs — and what puts the declared capacity back once it is through.
///
/// `SetDesiredCapacity` "sets the size of the specified Auto Scaling group"; the group's own
/// cooldown is not honoured, because this is not a scaling activity a policy asked for: it is
/// the capacity CloudFormation needs the group to hold for the length of the replacement.
pub(crate) async fn set_group_capacity(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
    capacity: usize,
) -> Result<(), Refusal> {
    let parameters = vec![
        ("AutoScalingGroupName".to_string(), name.to_string()),
        ("DesiredCapacity".to_string(), capacity.to_string()),
        ("HonorCooldown".to_string(), "false".to_string()),
    ];
    call_query(
        ctx,
        AUTOSCALING_SIGNING_NAME,
        region,
        AUTOSCALING_API_VERSION,
        "SetDesiredCapacity",
        &parameters,
    )
    .await
    .map(|_| ())
}

/// The process that "adds instances to the Auto Scaling group when the group scales out", as
/// that service's own catalog names it (`ProcessType$ProcessName`).
///
/// It is also what launches the REPLACEMENT a terminated instance leaves owed — "if you do not
/// specify the option to decrement the desired capacity, Amazon EC2 Auto Scaling launches
/// instances to replace the ones that are terminated"
/// (`TerminateInstanceInAutoScalingGroup`) — so a group that has it suspended holds the
/// capacity it is short of rather than launching into it, and launches it when the process is
/// resumed.
const AUTOSCALING_LAUNCH_PROCESS: &str = "Launch";

/// Hold the group `name` back from LAUNCHING, which is what lets a batch of a rolling update
/// take its instances out of service before their replacements are launched.
///
/// `SuspendProcesses` "suspends the specified auto scaling processes … for the specified Auto
/// Scaling group", and this names the one process whose suspension withholds the launch
/// ([`AUTOSCALING_LAUNCH_PROCESS`]): everything else the group does — the health it reports,
/// the instances it takes out of service — is left running.
pub(crate) async fn withhold_group_launches(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
) -> Result<(), Refusal> {
    scaling_process_call(ctx, region, name, "SuspendProcesses").await
}

/// Let the group `name` launch again, which is what brings a batch's replacements up.
///
/// `ResumeProcesses` "resumes the specified auto scaling processes": the group then does again
/// what the process does, which is to bring its instances to the capacity it holds — the
/// capacity the batch's termination left it short of.
pub(crate) async fn resume_group_launches(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
) -> Result<(), Refusal> {
    scaling_process_call(ctx, region, name, "ResumeProcesses").await
}

/// The one call both of those make: `ScalingProcessQuery` carries the group and the processes
/// it names, so a suspension and a resumption of the same process differ only in the action.
async fn scaling_process_call(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
    action: &str,
) -> Result<(), Refusal> {
    let parameters = vec![
        ("AutoScalingGroupName".to_string(), name.to_string()),
        (
            "ScalingProcesses.member.1".to_string(),
            AUTOSCALING_LAUNCH_PROCESS.to_string(),
        ),
    ];
    call_query(
        ctx,
        AUTOSCALING_SIGNING_NAME,
        region,
        AUTOSCALING_API_VERSION,
        action,
        &parameters,
    )
    .await
    .map(|_| ())
}

/// The Auto Scaling group as that service reports it NOW, or `None` where it reports no such
/// group: the scalar members of the one entry `DescribeAutoScalingGroups` answers for the
/// name, in the spellings this TYPE declares them in
/// ([`auto_scaling_group_as_the_type_spells_it`]), beside the group's own ARN.
///
/// This is both the read the create confirms itself with and the ACTUAL configuration drift
/// detection compares the declared one against. An awsQuery answer carries each group as a
/// `<member>` of its `AutoScalingGroups` list, and the request names ONE group, so the first
/// such element is that group.
pub(super) async fn describe_auto_scaling_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
) -> Result<Option<Value>, Refusal> {
    let parameters = vec![(
        "AutoScalingGroupNames.member.1".to_string(),
        name.to_string(),
    )];
    let answered = call_query(
        ctx,
        AUTOSCALING_SIGNING_NAME,
        region,
        AUTOSCALING_API_VERSION,
        "DescribeAutoScalingGroups",
        &parameters,
    )
    .await?;
    let reported = xml_scalar_children(&answered, "member");
    Ok(reported
        .get("AutoScalingGroupName")
        .and_then(Value::as_str)
        .is_some_and(|reported_name| reported_name == name)
        .then(|| Value::Object(auto_scaling_group_as_the_type_spells_it(reported))))
}

/// The group Amazon EC2 Auto Scaling reported, in the spellings `AWS::AutoScaling::AutoScalingGroup`
/// declares its properties in: the INVERSE of the two translations the create applies
/// ([`AUTO_SCALING_GROUP_RENAMED_PROPERTIES`] and [`comma_separated_subnets`]).
///
/// What this read is compared against is the DECLARATION, so a property the owning API spells
/// differently has to come back under the name — and in the shape — the type states it in: the
/// type's `Cooldown` is that API's `DefaultCooldown`, and its `VPCZoneIdentifier` is "a list of
/// subnet IDs" where the API reports "one or more comma-separated subnet IDs". Without the
/// projection a group placed in subnets is reported as having drifted from the very
/// declaration it was created from, and a changed cooldown as no drift at all.
fn auto_scaling_group_as_the_type_spells_it(reported: Map<String, Value>) -> Map<String, Value> {
    let mut reported = reported;
    for (property, member) in AUTO_SCALING_GROUP_RENAMED_PROPERTIES {
        if let Some(value) = reported.remove(*member) {
            reported.insert((*property).to_string(), value);
        }
    }
    if let Some(subnets) = reported
        .get(VPC_ZONE_IDENTIFIER)
        .and_then(Value::as_str)
        .map(ToString::to_string)
    {
        let listed: Vec<Value> = subnets
            .split(',')
            .filter(|subnet| !subnet.is_empty())
            .map(|subnet| Value::String(subnet.to_string()))
            .collect();
        reported.insert(VPC_ZONE_IDENTIFIER.to_string(), Value::Array(listed));
    }
    reported
}

/// The property an Auto Scaling group states the subnets it places instances in, which the
/// type declares as a LIST and the API takes and reports as one comma-separated string.
const VPC_ZONE_IDENTIFIER: &str = "VPCZoneIdentifier";

/// The tag Amazon EC2 Auto Scaling puts on every instance it launches, naming the group the
/// instance belongs to: "Amazon EC2 Auto Scaling automatically adds the
/// `aws:autoscaling:groupName` tag to the instances it launches".
///
/// It is how the instances of a group a stack has DELETED are found: the group itself is gone
/// from that service the moment the delete answers, while its instances are still being
/// terminated in Amazon EC2 ([`auto_scaling_group_removal`]). Spelled here rather than
/// imported, because this crate reaches every owning service over its own WIRE and links
/// none of them.
const GROUP_NAME_TAG: &str = "aws:autoscaling:groupName";

/// The instance states an instance a termination has not finished taking away is in: every
/// `InstanceStateName` but `terminated`, which is the one state in which an instance "holds
/// nothing any more" — the reading Amazon EC2's own delete of a subnet and of a security group
/// makes of its instances.
const NOT_YET_TERMINATED: &[&str] = &[
    "pending",
    "running",
    "shutting-down",
    "stopping",
    "stopped",
];

/// The state reported for a group whose instances Amazon EC2 is still terminating, where that
/// service reports no state for one of them: the word a terminating instance carries.
const SHUTTING_DOWN: &str = "shutting-down";

/// Whether Amazon EC2 Auto Scaling has finished removing the group a stack deleted: whether
/// Amazon EC2 has finished terminating the instances that group was holding.
///
/// The delete terminates them as it answers ([`delete_auto_scaling_group`] sends
/// `ForceDelete`), and a termination is not instant: Amazon EC2 reports the instance
/// `shutting-down` and takes it to `terminated` on its own clock, and until it gets there the
/// instance holds the SUBNET it was placed in and the security groups it ran with. So the
/// resource is still being removed while any of those instances is left, which is what keeps
/// a stack from deleting the network it launched them into while they are still in it — a
/// delete that service would refuse for a dependency this very teardown is removing.
///
/// The instances are found by the tag Amazon EC2 Auto Scaling puts on each of them
/// ([`GROUP_NAME_TAG`]), because the group itself is gone from that service already, and only
/// the ones that have not reached `terminated` are counted ([`NOT_YET_TERMINATED`]).
pub(super) async fn auto_scaling_group_removal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
) -> Result<Removal, Refusal> {
    let mut parameters = vec![
        ("Filter.1.Name".to_string(), format!("tag:{GROUP_NAME_TAG}")),
        ("Filter.1.Value.1".to_string(), name.to_string()),
        (
            "Filter.2.Name".to_string(),
            "instance-state-name".to_string(),
        ),
    ];
    for (index, state) in NOT_YET_TERMINATED.iter().enumerate() {
        parameters.push((
            format!("Filter.2.Value.{}", index + 1),
            (*state).to_string(),
        ));
    }
    let answered = call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "DescribeInstances",
        &parameters,
    )
    .await?;
    let still_there = xml_element(&answered, "instanceId")
        .filter(|instance_id| !instance_id.is_empty())
        .is_some();
    if !still_there {
        return Ok(Removal::Gone);
    }
    // The state Amazon EC2 reports the instance in, read as a child of its own
    // `<instanceState>` so no other element of the answer is mistaken for it.
    let reported = xml_scalar_children(&answered, "instanceState")
        .get("name")
        .and_then(Value::as_str)
        .filter(|state| !state.is_empty())
        .unwrap_or(SHUTTING_DOWN)
        .to_string();
    Ok(Removal::Pending(reported))
}
