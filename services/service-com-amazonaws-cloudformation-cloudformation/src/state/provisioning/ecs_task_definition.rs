// SPDX-License-Identifier: CC0-1.0

//! `AWS::ECS::TaskDefinition`: the task definition a stack declares, REGISTERED in Amazon
//! ECS itself.
//!
//! This is the per-type half of provisioning for one resource type, the half
//! [`super::super::provisioner`] holds for every other type this emulator creates; it lives
//! in its own module beside [`super::iot_authorizer`] for the reason that module records —
//! the catalogue file is far past the length a change to this repository may touch — and is
//! reached through the same seam ([`super::Owner`]).
//!
//! ## Why the task definition is really registered
//!
//! "Ref returns the Amazon Resource Name (ARN) of the task definition", and that ARN is the
//! `taskDefinition` handle every Amazon ECS call that runs or reads one takes:
//! `DescribeTaskDefinition`, `RunTask`, an `AWS::ECS::Service`'s `TaskDefinition` and an
//! EventBridge target's `EcsParameters.TaskDefinitionArn` are each addressed by it. A stack
//! that reported the resource created while nothing registered it publishes an ARN that
//! resolves to nothing, so the first read a client makes against the value the stack handed
//! it is a not-found that points at the caller rather than at the stack.
//!
//! The ARN also carries the REVISION, and the revision is Amazon ECS's to allocate: "the
//! revision is a version number of a task definition in a family", incremented per
//! registration. Nothing in a template decides it, so the identity this arm reports is the
//! one that service answered ([`super::super::template::identity_is_service_assigned`] names
//! the type) and the ARN CloudFormation's naming rules computed is only the stand-in a
//! declaration is resolved against until then.
//!
//! ## The properties
//!
//! The type's properties are `RegisterTaskDefinition`'s own members in UPPER camel case, all
//! the way down ([`register_members`]) — the same re-casing `AWS::ECS::Cluster` needs — with
//! three kinds of exception this module carries:
//!
//! * `Family` ADDRESSES the registration rather than travelling beside the others: it is the
//!   family the revision belongs to, and a declaration that states none is registered under
//!   the name CloudFormation computed for the resource ([`declared_family`]);
//! * `Tags` is rebuilt rather than carried, so the revision carries the tags CloudFormation
//!   gives the resource beside the declared ones, as the `{key, value}` list Amazon ECS's own
//!   `Tag` shape spells;
//! * four nested members are NOT the property with its initial letter lower-cased
//!   ([`RENAMED_MEMBERS`]), and the maps whose keys are a caller's own DATA rather than
//!   modelled member names are carried through verbatim ([`DATA_MAP_MEMBERS`]) so a docker
//!   label or a log-driver option keeps the key the template wrote.
//!
//! A declared property this arm does not carry is REFUSED rather than dropped
//! ([`refuse_unapplied_properties`]): a revision registered with less than its declaration
//! asked for is the shallow success this arm exists to avoid.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::{applicable_tags, call_json_rpc, Created, Readiness, Refusal, Removal, AWS_JSON_1_1};
use crate::state::provisioner::tags_of;
use crate::state::StoredResource;

/// The SigV4 signing name of the service that owns the type, which is what routes the
/// in-process call to Amazon ECS's own handlers, and the `X-Amz-Target` prefix its model
/// publishes (`sigv4Name: ecs`, service shape `AmazonEC2ContainerServiceV20141113`). It
/// speaks `awsJson1_1`.
const ECS_SIGNING_NAME: &str = "ecs";
const ECS_TARGET_PREFIX: &str = "AmazonEC2ContainerServiceV20141113";

/// The code a refusal of this module's own carries for a declaration it cannot act on.
const NOT_IMPLEMENTED: &str = "NotImplemented";

/// The code a refusal carries for a declaration this module cannot read at all.
const INVALID_REQUEST: &str = "InvalidRequest";

/// How deep a declared property document may nest before it is refused rather than walked:
/// the template reader's own bound, so no document a stored template can carry reaches it.
const MAX_PROPERTY_NESTING: usize = crate::state::template::MAX_TEMPLATE_DEPTH;

/// The property naming the FAMILY the revision belongs to, which addresses the registration
/// rather than travelling beside the other members.
const FAMILY: &str = "Family";

/// The property carrying the resource's own tags, which is rebuilt rather than carried.
const TAGS: &str = "Tags";

/// The attribute `AWS::ECS::TaskDefinition` publishes through `Fn::GetAtt`
/// ("TaskDefinitionArn: the ARN of the task definition"), and the member Amazon ECS reports
/// it under.
const TASK_DEFINITION_ARN: &str = "TaskDefinitionArn";
const TASK_DEFINITION_ARN_MEMBER: &str = "taskDefinitionArn";

/// The segment an ECS task-definition ARN carries its family and revision after
/// (`arn:<partition>:ecs:<region>:<account>:task-definition/<family>:<revision>`).
const TASK_DEFINITION_ARN_SEGMENT: &str = "task-definition/";

/// Every property `AWS::ECS::TaskDefinition` publishes that travels to
/// `RegisterTaskDefinition` as a member of its own, in the order AWS's resource reference
/// lists them.
///
/// Source: that reference's property list against `RegisterTaskDefinition`'s member list
/// (`smithy_query(query="operation",operation="RegisterTaskDefinition")`), which carries one
/// member per property up to the initial letter's case. [`FAMILY`] and [`TAGS`] are the two
/// properties NOT here, each for the reason this module's own docs record.
const CARRIED_PROPERTIES: &[&str] = &[
    "ContainerDefinitions",
    "Cpu",
    "EnableFaultInjection",
    "EphemeralStorage",
    "ExecutionRoleArn",
    "InferenceAccelerators",
    "IpcMode",
    "Memory",
    "NetworkMode",
    "PidMode",
    "PlacementConstraints",
    "ProxyConfiguration",
    "RequiresCompatibilities",
    "RuntimePlatform",
    "TaskRoleArn",
    "Volumes",
];

/// Every property of the type, which is [`CARRIED_PROPERTIES`] plus the two this module
/// applies its own way. A declared property this list does not carry is one no call here
/// applies, and is refused rather than dropped.
const TASK_DEFINITION_PROPERTIES: &[&str] = &[
    "ContainerDefinitions",
    "Cpu",
    "EnableFaultInjection",
    "EphemeralStorage",
    "ExecutionRoleArn",
    FAMILY,
    "InferenceAccelerators",
    "IpcMode",
    "Memory",
    "NetworkMode",
    "PidMode",
    "PlacementConstraints",
    "ProxyConfiguration",
    "RequiresCompatibilities",
    "RuntimePlatform",
    TAGS,
    "TaskRoleArn",
    "Volumes",
];

/// The nested property names whose API member is NOT the property with its initial letter
/// lower-cased, as (property, member) pairs.
///
/// Source: each owning shape's member list against the property names AWS's resource and
/// property types reference publishes — `smithy_query(query="shape",shape="ProxyConfiguration")`
/// spells the property bag `properties` while the type publishes it as
/// `ProxyConfigurationProperties`, `Volume`'s two file-system configurations are
/// `efsVolumeConfiguration` and `fsxWindowsFileServerVolumeConfiguration` (lower-casing only
/// the initial letter of either property name would spell `eFS…` / `fSx…`), and
/// `EFSVolumeConfiguration`'s file system is `fileSystemId` while that property type
/// publishes `FilesystemId`.
const RENAMED_MEMBERS: &[(&str, &str)] = &[
    ("ProxyConfigurationProperties", "properties"),
    ("EFSVolumeConfiguration", "efsVolumeConfiguration"),
    (
        "FSxWindowsFileServerVolumeConfiguration",
        "fsxWindowsFileServerVolumeConfiguration",
    ),
    ("FilesystemId", "fileSystemId"),
];

/// The API members that are MAPS whose keys are a caller's own data rather than modelled
/// member names, so the entries below one are carried through exactly as the template wrote
/// them.
///
/// Source: the map-typed members of the shapes a task definition reaches — a container
/// definition's `dockerLabels` ("a key/value map of labels to add to the container"), the
/// `options` of its `logConfiguration` and of its `firelensConfiguration` (a log driver's own
/// option names, `awslogs-group` among them), and a docker volume configuration's
/// `driverOpts` and `labels`. Each is named here by the MEMBER, so the test applies after the
/// key has been re-cased.
const DATA_MAP_MEMBERS: &[&str] = &["dockerLabels", "driverOpts", "labels", "options"];

/// The two members Amazon ECS's own `Tag` structure spells a tag's key and value with
/// (`smithy_query(query="shape",shape="Tag")`).
const TAG_MEMBERS: (&str, &str) = ("key", "value");

/// Register the revision an `AWS::ECS::TaskDefinition` declares, in Amazon ECS itself — so a
/// client that deployed the stack can pass `!Ref <task definition>` straight to
/// `DescribeTaskDefinition`, to `RunTask`, or to an EventBridge target's
/// `EcsParameters.TaskDefinitionArn`.
///
/// What this answers as the resource's identity is the ARN Amazon ECS reported, revision and
/// all: that service allocates the revision, so the stand-in the naming rules computed is
/// replaced by the identifier really in force.
///
/// `Err` is that service's own refusal of the registration — a Fargate task size it does not
/// offer, a container definition missing its image, a family name outside the character set —
/// which is what the resource fails with, because a registration that did not answer 2xx
/// registered nothing.
pub(super) async fn create(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unapplied_properties(&resource.properties, &resource.resource_type)?;
    let mut request = register_members(&resource.properties)?;
    request.insert(
        lower_camel(FAMILY),
        Value::String(declared_family(resource)),
    );
    // The revision carries the tags its declaration asks for plus the ones CloudFormation
    // gives the resource, as `RegisterTaskDefinition`'s own `tags` list — less the keys in
    // AWS's reserved namespace, which `RegisterTaskDefinition` refuses to a caller like the
    // other reserved-namespace APIs.
    let tags = applicable_tags(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert("tags".to_string(), ecs_tag_list(&tags));
    }
    let answered = call_ecs(ctx, region, "RegisterTaskDefinition", &Value::Object(request)).await?;
    let Some(arn) = answered
        .get("taskDefinition")
        .and_then(|definition| definition.get(TASK_DEFINITION_ARN_MEMBER))
        .and_then(Value::as_str)
        .filter(|arn| !arn.is_empty())
    else {
        // A registration that identified nothing is no created resource: the resource fails
        // rather than the stack publishing an identifier no call of that service takes.
        return Err(Refusal {
            code: INVALID_REQUEST.to_string(),
            message: format!(
                "Amazon ECS reported no {TASK_DEFINITION_ARN_MEMBER} for the \
                 {} it registered",
                resource.resource_type
            ),
        });
    };
    let mut attributes = BTreeMap::new();
    attributes.insert(TASK_DEFINITION_ARN.to_string(), arn.to_string());
    Ok(Created {
        physical_resource_id: Some(arn.to_string()),
        attributes,
    })
}

/// Whether Amazon ECS reports the registered revision ready.
///
/// Nothing is asked, and nothing waits: `RegisterTaskDefinition` answers the revision
/// `ACTIVE` — a task can be run from it that instant — so there is no transient state to
/// settle out of.
pub(super) fn ready() -> Readiness {
    Readiness::Ready
}

/// Whether Amazon ECS has finished removing a deregistered revision.
///
/// Nothing is asked: `DeregisterTaskDefinition` marks the revision `INACTIVE` as it answers,
/// and that service reports no state a deregistered one is still being removed in.
pub(super) fn removal() -> Removal {
    Removal::Gone
}

/// Apply a CHANGED declaration to the revision the stack already has.
///
/// `Ok(None)` always: a task-definition revision is IMMUTABLE — Amazon ECS publishes no call
/// that edits one, and every property of the type requires the resource to be replaced
/// ([`super::super::template::replacement_properties`] names them all), so a deployment that
/// states one differently registers a NEW revision rather than reaching this. Answering
/// `None` is what keeps a change this arm cannot make from being reported as applied.
pub(super) fn update() -> Option<Created> {
    None
}

/// Deregister the revision a stack registered, addressed by the ARN Amazon ECS knows it by.
///
/// The revision itself is kept — "INACTIVE task definitions remain discoverable in your
/// account indefinitely" — which is what that service does with a deregistered one; a
/// revision already out of `ACTIVE` is reported as it stands, so a second deregistration is
/// the success a repeated teardown needs.
pub(super) async fn delete(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "taskDefinition": resource.physical_resource_id });
    call_ecs(ctx, region, "DeregisterTaskDefinition", &request).await?;
    Ok(())
}

/// The family the revision belongs to: the declared `Family`, and otherwise the family the
/// identifier CloudFormation computed for the resource carries — which is the generated name
/// a declaration naming no family is registered under.
fn declared_family(resource: &StoredResource) -> String {
    if let Some(family) = resource
        .properties
        .get(FAMILY)
        .and_then(Value::as_str)
        .filter(|family| !family.is_empty())
    {
        return family.to_string();
    }
    family_of(&resource.physical_resource_id)
}

/// The family segment of a computed task-definition ARN
/// (`…:task-definition/<family>:<revision>`), or the whole value where it is no such ARN —
/// which is then what the registration asks for, and Amazon ECS judges whether it names a
/// family.
fn family_of(identifier: &str) -> String {
    let Some((_, path)) = identifier.split_once(TASK_DEFINITION_ARN_SEGMENT) else {
        return identifier.to_string();
    };
    match path.rsplit_once(':') {
        Some((family, _)) => family.to_string(),
        None => path.to_string(),
    }
}

/// The members `RegisterTaskDefinition` takes from the declared properties, each under that
/// call's own member name and with every member name INSIDE it re-cased the same way
/// ([`api_members`]).
fn register_members(properties: &Value) -> Result<Map<String, Value>, Refusal> {
    let mut request = Map::new();
    for property in CARRIED_PROPERTIES {
        let Some(value) = properties.get(*property).filter(|value| !value.is_null()) else {
            continue;
        };
        request.insert(lower_camel(property), api_members(value, 0)?);
    }
    Ok(request)
}

/// One tag set as the `[{key, value}]` list Amazon ECS's own `Tag` shape spells.
fn ecs_tag_list(tags: &[(String, String)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| {
                let mut tag = Map::new();
                tag.insert(TAG_MEMBERS.0.to_string(), Value::String(key.clone()));
                tag.insert(TAG_MEMBERS.1.to_string(), Value::String(value.clone()));
                Value::Object(tag)
            })
            .collect(),
    )
}

/// `name` with its initial letter lower-cased, or the member [`RENAMED_MEMBERS`] names for
/// it: the one spelling rule between a property of this type and the API member it is.
fn lower_camel(name: &str) -> String {
    if let Some((_, member)) = RENAMED_MEMBERS
        .iter()
        .find(|(property, _)| *property == name)
    {
        return (*member).to_string();
    }
    let mut characters = name.chars();
    match characters.next() {
        Some(first) => first.to_lowercase().chain(characters).collect(),
        None => String::new(),
    }
}

/// One property value with every member name inside it spelled as the API member it is
/// ([`lower_camel`]), and the entries of a map whose keys are a caller's own DATA carried
/// through verbatim ([`DATA_MAP_MEMBERS`]).
///
/// `depth` is how many levels are already above this one, so the outermost call passes `0`;
/// the document is caller-supplied, so one nested past [`MAX_PROPERTY_NESTING`] is REFUSED
/// rather than walked or quietly truncated.
fn api_members(value: &Value, depth: usize) -> Result<Value, Refusal> {
    if depth >= MAX_PROPERTY_NESTING {
        return Err(Refusal {
            code: INVALID_REQUEST.to_string(),
            message: format!(
                "a property of {} nests more than the {MAX_PROPERTY_NESTING} levels this \
                 emulator reads, so the task definition was not registered",
                crate::state::template::ECS_TASK_DEFINITION
            ),
        });
    }
    match value {
        Value::Object(members) => {
            let mut converted = Map::new();
            for (name, member) in members {
                let member_name = lower_camel(name);
                // The VALUE of a data map is carried as written: re-casing it would rename a
                // docker label or a log-driver option the template wrote.
                let carried = if DATA_MAP_MEMBERS.contains(&member_name.as_str()) {
                    member.clone()
                } else {
                    api_members(member, depth + 1)?
                };
                converted.insert(member_name, carried);
            }
            Ok(Value::Object(converted))
        }
        Value::Array(elements) => {
            let mut converted = Vec::with_capacity(elements.len());
            for element in elements {
                converted.push(api_members(element, depth + 1)?);
            }
            Ok(Value::Array(converted))
        }
        scalar => Ok(scalar.clone()),
    }
}

/// The refusal for a declared property this module does not apply, naming which property it
/// was: refusing is what keeps a revision from being reported as registered with less than
/// its declaration asked for.
fn refuse_unapplied_properties(properties: &Value, resource_type: &str) -> Result<(), Refusal> {
    let Value::Object(properties) = properties else {
        return Ok(());
    };
    for property in properties.keys() {
        if TASK_DEFINITION_PROPERTIES.contains(&property.as_str()) {
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

/// Call one Amazon ECS operation on the awsJson1_1 wire its model declares.
async fn call_ecs(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operation: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    call_json_rpc(
        ctx,
        ECS_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ECS_TARGET_PREFIX}.{operation}"),
        request,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The declared properties reach `RegisterTaskDefinition` under that call's own member
    /// names, all the way down — with the four renamed members spelled as the API spells
    /// them, and the keys of a data map left exactly as the template wrote them.
    #[test]
    fn the_declared_properties_are_carried_under_the_api_member_names() {
        let declared = serde_json::json!({
            "Family": "reports",
            "Cpu": "256",
            "Memory": "512",
            "NetworkMode": "awsvpc",
            "RequiresCompatibilities": ["FARGATE"],
            "RuntimePlatform": {"CpuArchitecture": "ARM64", "OperatingSystemFamily": "LINUX"},
            "EphemeralStorage": {"SizeInGiB": 21},
            "ProxyConfiguration": {
                "Type": "APPMESH",
                "ContainerName": "envoy",
                "ProxyConfigurationProperties": [{"Name": "IgnoredUID", "Value": "1337"}]
            },
            "Volumes": [{
                "Name": "shared",
                "EFSVolumeConfiguration": {"FilesystemId": "fs-0123456789abcdef0"}
            }],
            "ContainerDefinitions": [{
                "Name": "worker",
                "Image": "public.ecr.aws/example/worker:1",
                "Essential": true,
                "DockerLabels": {"Team": "reports", "tier": "batch"},
                "LogConfiguration": {
                    "LogDriver": "awslogs",
                    "Options": {"awslogs-group": "/aws/ecs/reports", "awslogs-region": "us-east-1"}
                }
            }]
        });
        let carried = Value::Object(
            register_members(&declared).expect("case \"a full declaration\": the walk converts it"),
        );
        let expected = serde_json::json!({
            "cpu": "256",
            "memory": "512",
            "networkMode": "awsvpc",
            "requiresCompatibilities": ["FARGATE"],
            "runtimePlatform": {"cpuArchitecture": "ARM64", "operatingSystemFamily": "LINUX"},
            "ephemeralStorage": {"sizeInGiB": 21},
            "proxyConfiguration": {
                "type": "APPMESH",
                "containerName": "envoy",
                "properties": [{"name": "IgnoredUID", "value": "1337"}]
            },
            "volumes": [{
                "name": "shared",
                "efsVolumeConfiguration": {"fileSystemId": "fs-0123456789abcdef0"}
            }],
            "containerDefinitions": [{
                "name": "worker",
                "image": "public.ecr.aws/example/worker:1",
                "essential": true,
                "dockerLabels": {"Team": "reports", "tier": "batch"},
                "logConfiguration": {
                    "logDriver": "awslogs",
                    "options": {"awslogs-group": "/aws/ecs/reports", "awslogs-region": "us-east-1"}
                }
            }]
        });
        assert_eq!(
            carried, expected,
            "case \"a full declaration\": expected {expected}, got {carried}"
        );
    }

    /// The family is the declared one, and otherwise the one the computed identifier carries
    /// — which is the name CloudFormation generated for a declaration that names no family.
    #[test]
    fn the_registration_is_made_under_the_declared_family() {
        // (case, the declared properties, the computed physical id, the family registered)
        let rows: &[(&str, Value, &str, &str)] = &[
            (
                "a declared family",
                serde_json::json!({"Family": "reports"}),
                "arn:aws:ecs:us-east-1:000000000000:task-definition/reports:1",
                "reports",
            ),
            (
                "no family at all",
                Value::Null,
                "arn:aws:ecs:us-east-1:000000000000:task-definition/stack-Task-ABC123:1",
                "stack-Task-ABC123",
            ),
            (
                "an empty declared family",
                serde_json::json!({"Family": ""}),
                "arn:aws:ecs:us-east-1:000000000000:task-definition/stack-Task-ABC123:1",
                "stack-Task-ABC123",
            ),
            (
                "a family carrying a hyphen and digits",
                serde_json::json!({"Family": "reports-2024"}),
                "arn:aws:ecs:us-east-1:000000000000:task-definition/reports-2024:7",
                "reports-2024",
            ),
            (
                "an identifier that is no task-definition ARN",
                Value::Null,
                "stack-Task-ABC123",
                "stack-Task-ABC123",
            ),
        ];
        for (case, properties, identifier, expected) in rows {
            let resource = StoredResource {
                logical_id: "Task".to_string(),
                resource_type: crate::state::template::ECS_TASK_DEFINITION.to_string(),
                physical_resource_id: (*identifier).to_string(),
                attributes: BTreeMap::new(),
                metadata: None,
                properties: properties.clone(),
                materialised: false,
                deletion_policy: None,
                update_replace_policy: None,
            };
            let family = declared_family(&resource);
            assert_eq!(
                family, *expected,
                "case {case:?}: expected the family {expected:?}, got {family:?}"
            );
        }
    }

    /// A property no call here applies is refused, naming it; every property the type
    /// publishes is accepted.
    #[test]
    fn an_unapplied_property_is_refused() {
        let declared = serde_json::json!({
            "ContainerDefinitions": [],
            "Cpu": "256",
            "EnableFaultInjection": false,
            "EphemeralStorage": {"SizeInGiB": 21},
            "ExecutionRoleArn": "arn:aws:iam::000000000000:role/exec",
            "Family": "reports",
            "InferenceAccelerators": [],
            "IpcMode": "task",
            "Memory": "512",
            "NetworkMode": "awsvpc",
            "PidMode": "task",
            "PlacementConstraints": [],
            "ProxyConfiguration": {},
            "RequiresCompatibilities": ["FARGATE"],
            "RuntimePlatform": {},
            "Tags": [],
            "TaskRoleArn": "arn:aws:iam::000000000000:role/task",
            "Volumes": []
        });
        assert!(
            refuse_unapplied_properties(&declared, crate::state::template::ECS_TASK_DEFINITION)
                .is_ok(),
            "case \"every published property\": expected it to be applied, got a refusal"
        );
        let unknown = serde_json::json!({"Family": "reports", "Unheard": 1});
        let refusal =
            refuse_unapplied_properties(&unknown, crate::state::template::ECS_TASK_DEFINITION)
                .expect_err("case \"a property no call carries\": expected a refusal");
        assert!(
            refusal.message.contains("Unheard"),
            "case {:?}: expected the refusal to name it, got {:?}",
            "a property no call carries",
            refusal.message
        );
    }

    /// The tag list carries the declaration's own tags and the ones CloudFormation
    /// propagates, under the member names Amazon ECS's `Tag` spells — and never a key in the
    /// reserved namespace, which `RegisterTaskDefinition` refuses to a caller.
    #[test]
    fn the_tag_list_leaves_the_reserved_namespace_alone() {
        let declared = serde_json::json!({"Tags": [{"Key": "Owner", "Value": "reports"}]});
        let propagated = vec![
            ("Stack".to_string(), "ecs-suite".to_string()),
            (
                "aws:cloudformation:logical-id".to_string(),
                "Task".to_string(),
            ),
        ];
        let carried = ecs_tag_list(&applicable_tags(&tags_of(&declared, &propagated)));
        let expected = serde_json::json!([
            {"key": "Owner", "value": "reports"},
            {"key": "Stack", "value": "ecs-suite"}
        ]);
        assert_eq!(
            carried, expected,
            "case \"a declaration beside the propagated tags\": expected {expected}, got \
             {carried}"
        );
    }

    /// A property document nested past the bound is refused rather than walked.
    #[test]
    fn a_property_nested_past_the_bound_is_refused() {
        let mut nested = Value::String("leaf".to_string());
        for _ in 0..=MAX_PROPERTY_NESTING {
            nested = serde_json::json!({ "Member": nested });
        }
        let refusal = api_members(&nested, 0)
            .expect_err("case \"a document past the bound\": expected a refusal");
        assert_eq!(
            refusal.code, INVALID_REQUEST,
            "case \"a document past the bound\": expected the refusal code \
             {INVALID_REQUEST:?}, got {:?}",
            refusal.code
        );
    }
}
