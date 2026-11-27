// SPDX-License-Identifier: CC0-1.0

//! The per-type provisioning arms of the Amazon Bedrock AgentCore agent RUNTIME and MEMORY a
//! template declares, plus the readers every arm of that family shares. One file of
//! [`super`]'s per-type arms, dispatched to by its `Provisioner`, beside the gateway's, its
//! targets' and the two sandbox types' own arms ([`super::bedrock_agentcore_gateways`]) and
//! the workload identity's ([`super::platform_services`]).
//!
//! ## Why these resources are really created
//!
//! Infrastructure as code is how these resources are normally made, and every AgentCore API
//! a client calls next addresses one by the identifier that service MINTED for it:
//! `GetAgentRuntime`, `UpdateAgentRuntime` and `InvokeAgentRuntime` take the
//! `agentRuntimeId`, `GetMemory` the `memoryId`, `GetGateway` and every call on a gateway's
//! targets the `gatewayId`, and the sandbox reads their own ids. A stack that reported one
//! of them created while nothing created it publishes an identifier that resolves to
//! nothing, so the readiness wait, the update and the first invocation a client makes with
//! the deployed stack all fail against a resource the stack said it had. Creating each
//! resource where it belongs is what makes the stack's report of it true.
//!
//! ## Every identity in this family is the owning service's
//!
//! None of its creates takes an identifier: each MINTS one and answers it
//! (`agentRuntimeId`, `memoryId`, `gatewayId`, `targetId`, `codeInterpreterId`,
//! `browserId`), so [`super::super::template::identity_is_service_assigned`] names all six
//! types and the identifier CloudFormation's naming rules computed is only what a reference
//! was resolved against until the create answered. A gateway TARGET's identity is the PAIR
//! every call on one binds — the gateway holding it and the target's own id — so that
//! resource's physical id is the two joined
//! ([`template::agentcore_gateway_target_identity`]), exactly as a Bedrock data source's is
//! ([`super::bedrock_data_sources`]).
//!
//! ## What the declaration is
//!
//! Every property of these types is the API member of the same name with a lower-cased
//! initial, all the way down (`AgentRuntimeArtifact.ContainerConfiguration.ContainerUri` is
//! `agentRuntimeArtifact.containerConfiguration.containerUri`), so the declared document IS
//! the request once its member names are re-cased ([`camel_cased`]) — the same relation the
//! Bedrock arms beside this file rely on. Two kinds of property are lifted out of that walk:
//! the ones whose own KEYS are data rather than member names (a tag map, a map of
//! environment variables), which travel verbatim, and the one member CloudFormation decides
//! — the resource's NAME ([`agentcore_name`]).
//!
//! The tag set reaches each create as the tag MAP its own `tags` member takes, so a resource
//! a stack creates carries the tags CloudFormation gives it beside the ones the declaration
//! asks for. No type of this family has a [`super::Provisioner::retag`] arm, so a later
//! deployment that changes only the tags leaves the resource carrying the set it was created
//! with.
//!
//! No `clientToken` is sent by any arm. That member is an `@idempotencyToken`, so a token
//! derived from the resource would make the create of a REPLACEMENT resource replay the one
//! the stack already had instead of making a new one.

use super::*;

/// The status an agent runtime, a gateway, a gateway target and either sandbox are reported
/// READY in (`AgentRuntimeStatus$READY`, `GatewayStatus$READY`, `TargetStatus$READY`,
/// `CodeInterpreterStatus$READY`, `BrowserStatus$READY`).
pub(super) const AGENTCORE_READY: &str = "READY";

/// The status a memory is reported ready in — that family's own spelling
/// (`MemoryStatus$ACTIVE`, beside `CREATING`, `UPDATING`, `FAILED` and `DELETING`).
const MEMORY_ACTIVE: &str = "ACTIVE";

/// The statuses a resource of the runtime, memory, gateway or sandbox families is still
/// SETTLING in: the two each of those enums documents for a resource its own service is
/// still working on.
pub(super) const AGENTCORE_SETTLING: &[&str] = &["CREATING", "UPDATING"];

/// The statuses a gateway TARGET is still settling in: the two above, plus the ones that
/// family adds for a target whose tools the gateway is reading from the endpoint it names
/// and one whose authorization the caller has still to complete (`TargetStatus`).
pub(super) const TARGET_SETTLING: &[&str] = &[
    "CREATING",
    "UPDATING",
    "SYNCHRONIZING",
    "CREATE_PENDING_AUTH",
    "UPDATE_PENDING_AUTH",
    "SYNCHRONIZE_PENDING_AUTH",
];

/// The member every AgentCore answer carries a resource's status in.
pub(super) const STATUS_MEMBER: &str = "status";

/// The member the runtime, memory and sandbox answers carry the diagnosis of a FAILED
/// resource in (`failureReason`).
const FAILURE_REASON_MEMBER: &str = "failureReason";

/// The member a gateway's and a target's answers carry that diagnosis in instead: a LIST of
/// reasons (`statusReasons`).
const STATUS_REASONS_MEMBER: &str = "statusReasons";

/// The property every one of these types states its tag set in, which the create takes as a
/// MAP of its own rather than as a member of the declared document.
pub(super) const TAGS_PROPERTY: &str = "Tags";

/// Whether the status the AgentCore control plane reports for `described` is the one its
/// family reports the resource ready in, one it is still settling in, or one no wait will
/// bring it out of — the reasons that service reports travelling with the failure.
pub(super) fn agentcore_readiness(
    described: &str,
    status: &str,
    ready: &str,
    settling: &[&str],
    reason: Option<String>,
) -> Readiness {
    if status == ready {
        return Readiness::Ready;
    }
    if settling.contains(&status) {
        return Readiness::Pending(status.to_string());
    }
    let reported =
        format!("the Amazon Bedrock AgentCore control plane reports {described} as {status}");
    match reason {
        Some(reason) if !reason.is_empty() => Readiness::Failed(format!("{reported}: {reason}")),
        _ => Readiness::Failed(reported),
    }
}

/// The reasons a gateway or a target answer reports for the status it is in, joined into the
/// one sentence a failed resource is reported with (`StatusReasons` is a LIST).
pub(super) fn status_reasons(answered: &Value) -> Option<String> {
    let reasons: Vec<String> = answered
        .get(STATUS_REASONS_MEMBER)
        .and_then(Value::as_array)
        .map(|reasons| reasons.iter().filter_map(scalar_text).collect())
        .unwrap_or_default();
    (!reasons.is_empty()).then(|| reasons.join("; "))
}

/// One declared property document as the request members the AgentCore control plane takes:
/// every member name re-cased ([`camel_cased`]), with the properties named in `verbatim`
/// carried through with their own KEYS untouched — those are caller-chosen data, not member
/// names — and the ones named in `omitted` left out, because the URI carries them or
/// CloudFormation decides them.
///
/// One reader for the create and the update of every type here, so the two halves of a
/// deployment cannot translate the same declaration differently.
pub(super) fn agentcore_members(
    declared: &Value,
    verbatim: &[&str],
    omitted: &[&str],
) -> Result<Map<String, Value>, Refusal> {
    let mut source = match declared {
        Value::Object(members) => members.clone(),
        // A declaration that is not an object states no member of the request.
        _ => Map::new(),
    };
    let mut carried: Vec<(String, Value)> = Vec::new();
    for property in verbatim {
        if let Some(value) = source.remove(*property) {
            carried.push((lower_initial(property), value));
        }
    }
    for property in omitted {
        source.remove(*property);
    }
    let mut request = match camel_cased(&Value::Object(source), 0)? {
        Value::Object(members) => members,
        _ => Map::new(),
    };
    request.extend(carried);
    Ok(request)
}

/// Carry a request member a declaration states as a SCALAR in the one member of the
/// STRUCTURE the API takes it as.
///
/// A resource type states a configuration whose API shape is a structure with a single
/// required member as that member's own scalar — one value, so the type publishes one
/// property for it — while the API takes the structure. The two are the same statement, so
/// the scalar is carried into `member` of that structure rather than handed to the API where
/// an object belongs, which the call refuses as a type error and the deployment reports as a
/// failed resource. A declaration that already states the structure is left exactly as it is,
/// and one stating nothing at all adds nothing.
fn scalar_in_member(request: &mut Map<String, Value>, property: &str, member: &str) {
    let Some(Value::String(scalar)) = request.get(property) else {
        return;
    };
    let mut held = Map::new();
    held.insert(member.to_string(), Value::String(scalar.clone()));
    request.insert(property.to_string(), Value::Object(held));
}

/// The tag MAP a create carries: the tags the declaration asks for beside the ones
/// CloudFormation gives the resource ([`tags_of`]), or `None` where there are none at all.
pub(super) fn agentcore_tags(
    properties: &Value,
    propagated: &[(String, String)],
) -> Option<Value> {
    let tags = tags_as_map(&tags_of(properties, propagated));
    (!tags.is_empty()).then_some(Value::Object(tags))
}

/// The NAME one of these resources is created under: the name its declaration states, or —
/// for a declaration that states none — the LABEL half of the identifier CloudFormation's
/// naming rules computed for the resource, which is the generated
/// `<stack>-<logical id>-<unique id>` name already reduced to the alphabet that family's own
/// name member admits ([`super::super::template::naming_rules`]).
///
/// The computed identifier is `<label>-<suffix>`, so the label is everything up to its last
/// `-`; a value of any other shape is taken whole, which is what a create then judges.
pub(super) fn agentcore_name(
    declared: &Value,
    property: &str,
    resource: &StoredResource,
) -> String {
    let stated = declared_scalar(declared, property);
    if !stated.is_empty() {
        return stated;
    }
    let identity = resource.physical_resource_id.as_str();
    match identity.rsplit_once('-') {
        Some((label, _)) if !label.is_empty() => label.to_string(),
        _ => identity.to_string(),
    }
}

/// The refusal an answer carrying no identifier for the resource it created is reported as:
/// there would be no handle a client could call the control plane with, and nothing for the
/// stack to publish.
pub(super) fn no_identifier_answered(described: &str) -> Refusal {
    Refusal {
        code: "InternalFailure".to_string(),
        message: format!(
            "the Amazon Bedrock AgentCore control plane answered {described} without an \
             identifier"
        ),
    }
}

/// Read one AgentCore resource at `uri`, or `None` where the control plane no longer holds it
/// (`ResourceNotFoundException`, judged on the CODE and never on a message that carries the
/// request's own values).
pub(super) async fn read_agentcore(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operation: &str,
    uri: &str,
) -> Result<Option<Value>, Refusal> {
    match call_rest_json_named(
        ctx,
        AGENTCORE_SIGNING_NAME,
        region,
        operation,
        "GET",
        uri,
        None,
    )
    .await
    {
        Ok(answer) => Ok(Some(answer)),
        Err(refusal) if refusal.code == AGENTCORE_RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// Delete one AgentCore resource at `uri`. One the control plane no longer holds
/// (`ResourceNotFoundException`) is already in the state the delete asks for.
pub(super) async fn delete_agentcore(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operation: &str,
    uri: &str,
) -> Result<(), Refusal> {
    match call_rest_json_named(
        ctx,
        AGENTCORE_SIGNING_NAME,
        region,
        operation,
        "DELETE",
        uri,
        None,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == AGENTCORE_RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// Whether the control plane has finished REMOVING the resource a stack deleted: each of
/// these deletes answers `202 Accepted` with the resource in a `DELETING` status, so it is
/// still there when the delete answers and the deletion is finished once that service no
/// longer holds it.
pub(super) async fn agentcore_removal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operation: &str,
    uri: &str,
) -> Result<Removal, Refusal> {
    match read_agentcore(ctx, region, operation, uri).await? {
        None => Ok(Removal::Gone),
        Some(answered) => Ok(Removal::Pending(
            reported_text(&answered, STATUS_MEMBER).unwrap_or_else(|| "DELETING".to_string()),
        )),
    }
}

// ---------------------------------------------------------------------------
// AWS::BedrockAgentCore::Runtime
// ---------------------------------------------------------------------------

/// The URI the runtime COLLECTION is bound to (`PUT /runtimes/`, trailing slash included, as
/// `CreateAgentRuntime`'s own `@http` binding spells it).
const RUNTIMES_URI: &str = "/runtimes/";

/// The property an `AWS::BedrockAgentCore::Runtime` states the runtime's name in, which is
/// the member `CreateAgentRuntime` takes it as.
const RUNTIME_NAME_PROPERTY: &str = "AgentRuntimeName";

/// The property whose own KEYS are data rather than member names: the environment the
/// runtime runs with is a map a caller chose the keys of.
const RUNTIME_VERBATIM_PROPERTIES: &[&str] = &["EnvironmentVariables"];

/// The properties no member of the runtime's create or update carries from the declared
/// document: the tag map, which travels as that call's own `tags` member, and the name, which
/// is the one member CloudFormation decides.
const RUNTIME_OMITTED_PROPERTIES: &[&str] = &[TAGS_PROPERTY, RUNTIME_NAME_PROPERTY];

/// The member the runtime's create and update take the server protocol in, and the member of
/// THAT STRUCTURE the protocol itself is stated in.
///
/// `protocolConfiguration` is a structure whose one required member is `serverProtocol`
/// (source: smithy_query query="shape" shape="ProtocolConfiguration"), while the resource
/// type states the protocol as the SCALAR that member takes (`ProtocolConfiguration: HTTP`),
/// so the declared scalar is carried in that member ([`scalar_in_member`]) rather than handed
/// to a structure member as a string.
const RUNTIME_PROTOCOL_MEMBER: &str = "protocolConfiguration";
/// See [`RUNTIME_PROTOCOL_MEMBER`].
const RUNTIME_SERVER_PROTOCOL_MEMBER: &str = "serverProtocol";

/// The members an answered runtime reports, as `(attribute, member)`: the three values this
/// type publishes as return values, read off the answer rather than recomputed here.
const RUNTIME_ATTRIBUTES: &[(&str, &str)] = &[
    ("AgentRuntimeId", "agentRuntimeId"),
    ("AgentRuntimeArn", "agentRuntimeArn"),
    ("AgentRuntimeVersion", "agentRuntimeVersion"),
];

/// The attribute carrying the id that service minted, which is this resource's physical id.
const RUNTIME_ID_ATTRIBUTE: &str = "AgentRuntimeId";

/// The properties `GetAgentRuntime` reports back under the same names, for drift detection
/// ([`super::drift`]): the scalars that read answers which a declaration also states.
const RUNTIME_REPORTED_PROPERTIES: &[(&str, &str)] = &[
    (RUNTIME_NAME_PROPERTY, "agentRuntimeName"),
    ("Description", "description"),
    ("RoleArn", "roleArn"),
];

/// The URI ONE runtime is addressed at (`/runtimes/{agentRuntimeId}/`), the label escaped as
/// a single path segment ([`percent_encoded`]) so a value carrying URI structure addresses
/// one runtime rather than another path.
fn runtime_uri(agent_runtime_id: &str) -> String {
    format!("{RUNTIMES_URI}{}/", percent_encoded(agent_runtime_id))
}

/// Create the agent runtime an `AWS::BedrockAgentCore::Runtime` declares, so a client that
/// deployed the stack reads it back through `GetAgentRuntime` by the `AgentRuntimeId` the
/// stack published rather than a `ResourceNotFoundException`.
pub(super) async fn create_agentcore_runtime(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = agentcore_members(
        &resource.properties,
        RUNTIME_VERBATIM_PROPERTIES,
        RUNTIME_OMITTED_PROPERTIES,
    )?;
    scalar_in_member(
        &mut request,
        RUNTIME_PROTOCOL_MEMBER,
        RUNTIME_SERVER_PROTOCOL_MEMBER,
    );
    request.insert(
        "agentRuntimeName".to_string(),
        Value::String(agentcore_name(
            &resource.properties,
            RUNTIME_NAME_PROPERTY,
            resource,
        )),
    );
    if let Some(tags) = agentcore_tags(&resource.properties, propagated) {
        request.insert("tags".to_string(), tags);
    }
    let answered = call_rest_json_named(
        ctx,
        AGENTCORE_SIGNING_NAME,
        region,
        "CreateAgentRuntime",
        "PUT",
        RUNTIMES_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    let attributes = runtime_attributes(&answered)?;
    Ok(Created {
        physical_resource_id: attributes.get(RUNTIME_ID_ATTRIBUTE).cloned(),
        attributes,
    })
}

/// Whether the control plane has finished bringing up the runtime a stack created.
///
/// `CreateAgentRuntime` answers `202 Accepted` with the runtime `CREATING` — it serves no
/// invocation in that status — so the resource's creation is held open until that service
/// reports it `READY`; `UPDATING` is the same wait one deployment later. Every other status
/// is one no wait will bring it out of, as is a runtime that service no longer holds.
pub(super) async fn agentcore_runtime_readiness(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Readiness, Refusal> {
    let Some(runtime) = read_agentcore_runtime(ctx, region, physical_resource_id).await? else {
        return Ok(Readiness::Failed(format!(
            "the Amazon Bedrock AgentCore control plane holds no agent runtime \
             {physical_resource_id}"
        )));
    };
    Ok(agentcore_readiness(
        &format!("agent runtime {physical_resource_id}"),
        &reported_text(&runtime, STATUS_MEMBER).unwrap_or_default(),
        AGENTCORE_READY,
        AGENTCORE_SETTLING,
        reported_text(&runtime, FAILURE_REASON_MEMBER),
    ))
}

/// Apply a changed declaration to the runtime the stack created, through
/// `UpdateAgentRuntime` — the one call that changes a runtime that exists, and the one that
/// publishes a new `agentRuntimeVersion` for it.
///
/// It takes the whole declared configuration by the id that service minted, so a member the
/// declaration no longer states leaves the runtime without it rather than keeping the one an
/// earlier deployment gave it.
///
/// `Ok(None)` is a change that call cannot make, so the deployment reports it as not applied
/// rather than as a success that dropped it: `UpdateAgentRuntime` models no name member at
/// all, so a declaration stating another `AgentRuntimeName` is a different runtime.
pub(super) async fn update_agentcore_runtime(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    let identity = resource.physical_resource_id.as_str();
    let declared_name = declared_scalar(declared, RUNTIME_NAME_PROPERTY);
    if !declared_name.is_empty() {
        let created_name = read_agentcore_runtime(ctx, region, identity)
            .await?
            .and_then(|runtime| reported_text(&runtime, "agentRuntimeName"));
        if created_name.is_some_and(|name| name != declared_name) {
            return Ok(None);
        }
    }
    let mut request = agentcore_members(
        declared,
        RUNTIME_VERBATIM_PROPERTIES,
        RUNTIME_OMITTED_PROPERTIES,
    )?;
    scalar_in_member(
        &mut request,
        RUNTIME_PROTOCOL_MEMBER,
        RUNTIME_SERVER_PROTOCOL_MEMBER,
    );
    let answered = call_rest_json_named(
        ctx,
        AGENTCORE_SIGNING_NAME,
        region,
        "UpdateAgentRuntime",
        "PUT",
        &runtime_uri(identity),
        Some(&Value::Object(request)),
    )
    .await?;
    Ok(Some(Created::attributes(runtime_attributes(&answered)?)))
}

/// Delete the runtime a stack created, by the id that service minted for it — this resource's
/// physical id.
pub(super) async fn delete_agentcore_runtime(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    delete_agentcore(
        ctx,
        region,
        "DeleteAgentRuntime",
        &runtime_uri(physical_resource_id),
    )
    .await
}

/// Whether the control plane has finished removing the runtime a stack deleted.
pub(super) async fn agentcore_runtime_removal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Removal, Refusal> {
    agentcore_removal(
        ctx,
        region,
        "GetAgentRuntime",
        &runtime_uri(physical_resource_id),
    )
    .await
}

/// The runtime's configuration as the control plane reports it, for drift detection:
/// `GetAgentRuntime`'s own answer, read by the id that is this resource's physical id. One it
/// no longer holds was deleted.
pub(super) async fn describe_agentcore_runtime(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    Ok(read_agentcore_runtime(ctx, region, physical_resource_id)
        .await?
        .map(|runtime| reported_as_properties(&runtime, RUNTIME_REPORTED_PROPERTIES)))
}

/// The runtime the control plane holds under `physical_resource_id`, or `None` for one it
/// does not.
async fn read_agentcore_runtime(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    read_agentcore(
        ctx,
        region,
        "GetAgentRuntime",
        &runtime_uri(physical_resource_id),
    )
    .await
}

/// The attributes one answered runtime reports ([`RUNTIME_ATTRIBUTES`]).
fn runtime_attributes(answered: &Value) -> Result<BTreeMap<String, String>, Refusal> {
    let attributes = reported_attributes(answered, RUNTIME_ATTRIBUTES);
    if !attributes.contains_key(RUNTIME_ID_ATTRIBUTE) {
        return Err(no_identifier_answered("an agent runtime"));
    }
    Ok(attributes)
}

// ---------------------------------------------------------------------------
// AWS::BedrockAgentCore::Memory
// ---------------------------------------------------------------------------

/// The URI `CreateMemory` is bound to (`POST /memories/create`).
const CREATE_MEMORY_URI: &str = "/memories/create";

/// The member the memory calls answer the memory structure in.
const MEMORY_MEMBER: &str = "memory";

/// The property an `AWS::BedrockAgentCore::Memory` states the memory's name in.
const MEMORY_NAME_PROPERTY: &str = "Name";

/// The properties no member of `CreateMemory` carries from the declared document: the tag
/// map, which travels as that call's own `tags` member, and the name CloudFormation decides.
const MEMORY_OMITTED_PROPERTIES: &[&str] = &[TAGS_PROPERTY, MEMORY_NAME_PROPERTY];

/// The members an answered memory reports, as `(attribute, member)`: the two values this type
/// publishes as return values.
const MEMORY_ATTRIBUTES: &[(&str, &str)] = &[("MemoryId", "id"), ("MemoryArn", "arn")];

/// The attribute carrying the id that service minted, which is this resource's physical id.
const MEMORY_ID_ATTRIBUTE: &str = "MemoryId";

/// The properties `GetMemory` reports back under the same names, for drift detection.
const MEMORY_REPORTED_PROPERTIES: &[(&str, &str)] = &[
    (MEMORY_NAME_PROPERTY, "name"),
    ("Description", "description"),
    ("EventExpiryDuration", "eventExpiryDuration"),
    ("MemoryExecutionRoleArn", "memoryExecutionRoleArn"),
    ("EncryptionKeyArn", "encryptionKeyArn"),
];

/// The URI ONE memory's DETAILS are read at (`GET /memories/{memoryId}/details`).
fn memory_details_uri(memory_id: &str) -> String {
    format!("/memories/{}/details", percent_encoded(memory_id))
}

/// The URI ONE memory is DELETED at (`DELETE /memories/{memoryId}/delete`).
fn memory_delete_uri(memory_id: &str) -> String {
    format!("/memories/{}/delete", percent_encoded(memory_id))
}

/// Create the memory an `AWS::BedrockAgentCore::Memory` declares, so a client that deployed
/// the stack reads it back through `GetMemory` by the `MemoryId` the stack published rather
/// than a `ResourceNotFoundException`.
pub(super) async fn create_agentcore_memory(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = agentcore_members(&resource.properties, &[], MEMORY_OMITTED_PROPERTIES)?;
    request.insert(
        "name".to_string(),
        Value::String(agentcore_name(
            &resource.properties,
            MEMORY_NAME_PROPERTY,
            resource,
        )),
    );
    if let Some(tags) = agentcore_tags(&resource.properties, propagated) {
        request.insert("tags".to_string(), tags);
    }
    let answered = call_rest_json_named(
        ctx,
        AGENTCORE_SIGNING_NAME,
        region,
        "CreateMemory",
        "POST",
        CREATE_MEMORY_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    let attributes = memory_attributes(&answered)?;
    Ok(Created {
        physical_resource_id: attributes.get(MEMORY_ID_ATTRIBUTE).cloned(),
        attributes,
    })
}

/// Whether the control plane has finished bringing up the memory a stack created.
///
/// `CreateMemory` answers `202 Accepted` with the memory `CREATING` — it holds no event in
/// that status — so the resource's creation is held open until that service reports it
/// `ACTIVE`, this family's own spelling of ready.
pub(super) async fn agentcore_memory_readiness(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Readiness, Refusal> {
    let Some(memory) = read_agentcore_memory(ctx, region, physical_resource_id).await? else {
        return Ok(Readiness::Failed(format!(
            "the Amazon Bedrock AgentCore control plane holds no memory {physical_resource_id}"
        )));
    };
    Ok(agentcore_readiness(
        &format!("memory {physical_resource_id}"),
        &reported_text(&memory, STATUS_MEMBER).unwrap_or_default(),
        MEMORY_ACTIVE,
        AGENTCORE_SETTLING,
        reported_text(&memory, FAILURE_REASON_MEMBER),
    ))
}

/// Delete the memory a stack created, by the id that service minted for it.
pub(super) async fn delete_agentcore_memory(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    delete_agentcore(
        ctx,
        region,
        "DeleteMemory",
        &memory_delete_uri(physical_resource_id),
    )
    .await
}

/// Whether the control plane has finished removing the memory a stack deleted.
pub(super) async fn agentcore_memory_removal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Removal, Refusal> {
    match read_agentcore_memory(ctx, region, physical_resource_id).await? {
        None => Ok(Removal::Gone),
        Some(memory) => Ok(Removal::Pending(
            reported_text(&memory, STATUS_MEMBER).unwrap_or_else(|| "DELETING".to_string()),
        )),
    }
}

/// The memory's configuration as the control plane reports it, for drift detection.
pub(super) async fn describe_agentcore_memory(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    Ok(read_agentcore_memory(ctx, region, physical_resource_id)
        .await?
        .map(|memory| reported_as_properties(&memory, MEMORY_REPORTED_PROPERTIES)))
}

/// The memory structure the control plane holds under `physical_resource_id`, or `None` for
/// one it does not: `GetMemory` answers the memory under its own `memory` member.
async fn read_agentcore_memory(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    Ok(read_agentcore(
        ctx,
        region,
        "GetMemory",
        &memory_details_uri(physical_resource_id),
    )
    .await?
    .and_then(|answer| answer.get(MEMORY_MEMBER).cloned()))
}

/// The attributes one answered memory reports ([`MEMORY_ATTRIBUTES`]), read off the `memory`
/// structure the create answers.
fn memory_attributes(answered: &Value) -> Result<BTreeMap<String, String>, Refusal> {
    let Some(memory) = answered.get(MEMORY_MEMBER) else {
        return Err(no_identifier_answered("a memory"));
    };
    let attributes = reported_attributes(memory, MEMORY_ATTRIBUTES);
    if !attributes.contains_key(MEMORY_ID_ATTRIBUTE) {
        return Err(no_identifier_answered("a memory"));
    }
    Ok(attributes)
}
