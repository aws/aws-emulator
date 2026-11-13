// SPDX-License-Identifier: CC0-1.0

//! `AWS::StepFunctions::StateMachine`: the state machine a stack declares, created in AWS
//! Step Functions itself.
//!
//! This is the per-type half of provisioning for one resource type, the half
//! [`super::super::provisioner`] holds for every other type this emulator creates; it lives
//! in its own module beside [`super::lambda_function`] for the reason that module records,
//! and is reached through the same seam ([`super::Owner`]).
//!
//! A client that deployed the stack and then calls `sfn describe-state-machine` with the ARN
//! the stack publishes reads the machine back, and an execution started against that ARN runs
//! the definition the template declared — so a stack is not reported as deployed while the
//! workflow at the heart of it exists nowhere.
//!
//! ## The definition
//!
//! The type states the workflow three ways, and the API takes one: `DefinitionString` is the
//! Amazon States Language document as TEXT, `Definition` is that same document as a template
//! OBJECT ("the Amazon States Language definition of the state machine […] you can specify
//! the definition in JSON or YAML"), and `DefinitionS3Location` names an object holding it.
//! The first two are the definition itself, so each becomes `CreateStateMachine`'s
//! `definition`; the third is refused rather than served with a document nothing read.
//!
//! Over whichever form carried it, `DefinitionSubstitutions` ("specify values for the
//! placeholder variables in your state machine definition") replaces each `${Key}` with the
//! value declared for that key, which is how a template puts the ARN of a function it also
//! declares into the `Resource` of a `Task` state.
//!
//! ## The identifier is an ARN
//!
//! "Ref returns the ARN of the state machine", which is a total function of the Region, the
//! account and the NAME, so the machine is created under the name CloudFormation decided and
//! the ARN Step Functions answers with is the one the stack already computed
//! ([`super::super::template::return_values`]). It is reported back as the resource's `Arn`
//! attribute all the same, so the handle the stack publishes is the one that service really
//! minted.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::{call_json_rpc, Created, Readiness, Refusal, Removal, AWS_JSON_1_0};
use crate::state::StoredResource;

/// The resource type this module provisions.
pub(super) const STEP_FUNCTIONS_STATE_MACHINE: &str = "AWS::StepFunctions::StateMachine";

/// AWS Step Functions' signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: states`, service shape `AWSStepFunctions`), which speaks `awsJson1_0`.
const SFN_SIGNING_NAME: &str = "states";
/// See [`SFN_SIGNING_NAME`].
const SFN_TARGET_PREFIX: &str = "AWSStepFunctions";

/// The error code Step Functions answers for a state machine ARN nothing answers to
/// (`StateMachineDoesNotExist`), which is what says a delete has nothing left to remove.
const STATE_MACHINE_DOES_NOT_EXIST: &str = "StateMachineDoesNotExist";

/// The status `DescribeStateMachine` reports a state machine that is being deleted in
/// (`StateMachineStatus`: ACTIVE | DELETING), which is what a removal is still waiting on.
const STATUS_DELETING: &str = "DELETING";

/// Every property `AWS::StepFunctions::StateMachine` publishes. A declared property this
/// list does not carry is REFUSED rather than dropped, so a state machine is never reported
/// created with less than its declaration asked for.
///
/// Source: the type's own property set, whose members beside the definition forms are
/// `CreateStateMachine`'s own
/// (`smithy_query(query="operation",operation="CreateStateMachine")`).
const STATE_MACHINE_PROPERTIES: &[&str] = &[
    "Definition",
    "DefinitionString",
    "DefinitionSubstitutions",
    "EncryptionConfiguration",
    "LoggingConfiguration",
    "RoleArn",
    "StateMachineName",
    "StateMachineType",
    "Tags",
    "TracingConfiguration",
];

/// The properties that are a CONFIGURATION of the machine: each is the API member of the
/// same name with its initial letter lower-cased, and so is every member inside it, because
/// every name inside one is modelled rather than a caller's own data.
const CONFIGURATION_PROPERTIES: &[&str] = &[
    "EncryptionConfiguration",
    "LoggingConfiguration",
    "TracingConfiguration",
];

/// How deeply a declared configuration object is walked when its member names are
/// re-spelled: deeper than any of the three above nests (a logging destination's log group
/// is two levels in) and a bound on a caller-supplied document all the same.
const MAX_CONFIGURATION_NESTING: usize = 8;

/// The most bytes a state machine definition may carry: the modelled `Definition` shape's
/// own `length` maximum
/// (`smithy_query(query="operation",operation="CreateStateMachine")` gives 1..1048576).
///
/// A declaration states the definition and then has its placeholders filled in
/// ([`substituted`]), so the document this module builds is measured against the same bound
/// the API would measure it against — and a declaration that takes it past the bound is
/// refused here rather than allocated and then sent.
const MAX_DEFINITION_BYTES: usize = 1_048_576;

/// What opens and closes a placeholder variable in a definition (`${Key}`), which
/// [`substituted`] scans for.
const PLACEHOLDER_OPENING: &str = "${";
/// See [`PLACEHOLDER_OPENING`].
const PLACEHOLDER_CLOSING: &str = "}";

/// Create the state machine an `AWS::StepFunctions::StateMachine` declares.
///
/// `Err` is Step Functions' own refusal of the create, which is what the resource fails
/// with: a definition that does not validate (`InvalidDefinition`), a name it will not take,
/// a logging or encryption configuration it refuses.
pub(super) async fn create(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unmapped_properties(resource)?;
    let name = state_machine_name(resource);
    let mut request = Map::new();
    request.insert("name".to_string(), Value::String(name.clone()));
    request.insert(
        "definition".to_string(),
        Value::String(definition(&resource.properties)?),
    );
    request.insert("roleArn".to_string(), Value::String(role_arn(resource)?));
    if let Some(declared) = text_property(&resource.properties, "StateMachineType") {
        request.insert("type".to_string(), Value::String(declared));
    }
    for property in CONFIGURATION_PROPERTIES {
        if let Some(configuration) = resource
            .properties
            .get(*property)
            .filter(|value| !value.is_null())
        {
            request.insert(
                lower_camel(property),
                lower_camel_members(configuration, 0)?,
            );
        }
    }
    let tags = crate::state::provisioner::tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("tags".to_string(), tag_list(&tags));
    }
    let answered = call_sfn(ctx, region, "CreateStateMachine", &Value::Object(request)).await?;
    let arn = answered
        .get("stateMachineArn")
        .and_then(Value::as_str)
        .filter(|arn| !arn.is_empty())
        .ok_or_else(|| Refusal {
            code: "InternalFailure".to_string(),
            message: "AWS Step Functions answered CreateStateMachine without a stateMachineArn"
                .to_string(),
        })?;
    let mut attributes = BTreeMap::new();
    attributes.insert("Arn".to_string(), arn.to_string());
    attributes.insert("Name".to_string(), name);
    Ok(Created::attributes(attributes))
}

/// A created state machine is ready the moment the create answers: `DescribeStateMachine`
/// reports it `ACTIVE` ("the state machine is active"), and an execution may be started
/// against it from then on.
pub(super) fn readiness() -> Readiness {
    Readiness::Ready
}

/// Apply a CHANGED declaration to the state machine the stack already has, through
/// `UpdateStateMachine` — which takes "the Amazon States Language definition", the
/// `roleArn`, and the logging, tracing and encryption configurations.
///
/// A change to a property that cannot be made on the machine that exists is applied by
/// REPLACING it ([`REPLACEMENT_PROPERTIES`], [`replacement`]): the machine's NAME and its
/// TYPE are its identity, which `UpdateStateMachine` carries neither of, so a declaration
/// that changes either is deployed as a new machine whose ARN the stack then publishes.
///
/// `Ok(None)` is left for a change neither call can make, so the deployment reports it as
/// not applied rather than as a success that dropped it:
///
/// * a changed `Tags` set, which `UpdateStateMachine` does not carry — a state machine's
///   tags are written by `TagResource`/`UntagResource`, which no arm here makes yet;
/// * a configuration the new declaration DROPS (`LoggingConfiguration`,
///   `TracingConfiguration`, `EncryptionConfiguration`): `UpdateStateMachine` leaves a member
///   it does not carry as it was, so there is no call in it that resets one to its default.
pub(super) async fn update(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
    propagated: &[(String, String)],
) -> Result<Option<Created>, Refusal> {
    refuse_unmapped_declaration(&resource.resource_type, declared)?;
    let replaced = REPLACEMENT_PROPERTIES.iter().any(|property| {
        text_property(declared, property) != text_property(&resource.properties, property)
    });
    if replaced {
        return replacement(ctx, region, resource, declared, propagated)
            .await
            .map(Some);
    }
    if declared.get("Tags") != resource.properties.get("Tags") {
        return Ok(None);
    }
    let mut request = Map::new();
    request.insert(
        "stateMachineArn".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    request.insert(
        "definition".to_string(),
        Value::String(definition(declared)?),
    );
    if let Some(role) = text_property(declared, "RoleArn") {
        request.insert("roleArn".to_string(), Value::String(role));
    }
    for property in CONFIGURATION_PROPERTIES {
        let stated = declared.get(*property).filter(|value| !value.is_null());
        match stated {
            Some(configuration) => {
                request.insert(
                    lower_camel(property),
                    lower_camel_members(configuration, 0)?,
                );
            }
            // A configuration the machine HAD and this declaration no longer states is a
            // change no member of this call makes.
            None if resource
                .properties
                .get(*property)
                .is_some_and(|held| !held.is_null()) =>
            {
                return Ok(None)
            }
            None => {}
        }
    }
    call_sfn(ctx, region, "UpdateStateMachine", &Value::Object(request)).await?;
    // The machine keeps its ARN across an update, so the handle the stack publishes — and
    // every attribute read off it — is unchanged.
    Ok(Some(Created::default()))
}

/// The properties of this type that cannot be changed on the machine that EXISTS, so a
/// declaration changing one of them is deployed by replacing the machine.
///
/// Both are the machine's identity: `StateMachineName` is the name its ARN is built from,
/// and `StateMachineType` is fixed at creation ("determines whether a Standard or Express
/// state machine is created") — `CreateStateMachine` takes each and `UpdateStateMachine`
/// takes neither.
const REPLACEMENT_PROPERTIES: &[&str] = &["StateMachineName", "StateMachineType"];

/// Deploy the declaration as a NEW state machine, which is what a changed
/// [`REPLACEMENT_PROPERTIES`] property asks for: the create is the same one a first
/// deployment makes ([`create`]), over the properties this deployment declares, and the ARN
/// it answers with is reported as the resource's physical id so the stack publishes the
/// machine that really carries the declared configuration.
///
/// The replacement is created under a name of its own ([`replacement_name`]), because this
/// type's identity IS its name: a machine created under the name the old one still holds
/// would be the same resource, and Step Functions refuses a second machine of that name.
async fn replacement(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut replacement = resource.clone();
    replacement.properties = declared.clone();
    replacement
        .attributes
        .insert("Name".to_string(), replacement_name(resource, declared));
    let mut created = create(ctx, region, &replacement, propagated).await?;
    // The machine the stack now publishes is the one this create minted, so the resource's
    // physical id is its ARN rather than the id the replaced machine had.
    created.physical_resource_id = created.attributes.get("Arn").cloned();
    Ok(created)
}

/// The NAME the replacement is created under: the one the declaration states, and otherwise
/// the generated name the replaced machine carried with a fresh unique segment.
///
/// CloudFormation generates a name for a resource the template does not name, and a
/// REPLACEMENT gets a new one — the old machine still holds the old name until the
/// replacement has been created. The segment is derived from the replaced machine's own ARN
/// and the declaration being deployed, so the same deployment asked for twice names the same
/// replacement (nothing here is drawn fresh per call) while a declaration that replaces the
/// machine again names another.
fn replacement_name(resource: &StoredResource, declared: &Value) -> String {
    if let Some(declared_name) = text_property(declared, "StateMachineName") {
        return declared_name;
    }
    let unique = crate::state::template::content_digest(
        &resource.physical_resource_id,
        &declared.to_string(),
        REPLACEMENT_DIGEST_CHARS,
    );
    let replaced = state_machine_name(resource);
    // The name is `@length 1..80`, so the stem gives up the characters the fresh segment
    // and its separator need.
    let room = MAX_STATE_MACHINE_NAME_CHARS.saturating_sub(unique.chars().count() + 1);
    let stem: String = replaced.chars().take(room).collect();
    format!("{stem}-{unique}")
}

/// How many characters the unique segment of a [`replacement_name`] carries: the width the
/// generated names of this crate use, which is wide enough that two replacements of one
/// machine cannot collide.
const REPLACEMENT_DIGEST_CHARS: usize = 12;

/// The most characters a state machine's name may carry: the modelled `Name` shape's own
/// `length` maximum (`smithy_query(query="operation",operation="CreateStateMachine")` gives
/// 1..80).
const MAX_STATE_MACHINE_NAME_CHARS: usize = 80;

/// Delete the state machine a stack created, addressed by its ARN — this resource's physical
/// id, which is what `Ref` returns for the type.
///
/// One Step Functions no longer holds is already in the state the delete asks for, judged on
/// the CODE and never on a message.
pub(super) async fn delete(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "stateMachineArn": resource.physical_resource_id });
    match call_sfn(ctx, region, "DeleteStateMachine", &request).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == STATE_MACHINE_DOES_NOT_EXIST => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// Whether Step Functions has finished removing a deleted state machine.
///
/// `DeleteStateMachine` "sets the state machine's status to DELETING and begins the deletion
/// process", so the resource is gone only once that service no longer answers for the ARN —
/// which is its own read, never a span this crate assumed.
pub(super) async fn removal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Removal, Refusal> {
    match described(ctx, region, physical_resource_id).await? {
        None => Ok(Removal::Gone),
        Some(machine) => Ok(Removal::Pending(
            machine
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or(STATUS_DELETING)
                .to_string(),
        )),
    }
}

/// The state machine's configuration as Step Functions reports it, for drift detection. One
/// it no longer holds was deleted.
///
/// The properties its read reports are the ones this type declares under a name
/// `DescribeStateMachine` answers for AND that the declaration states as they are stored: the
/// role, the name and the type. A property the read does not report is not evidence of
/// anything ([`super::super::drift`]), which is why the DEFINITION is not among them — the
/// document that service holds is the one the declaration's `DefinitionSubstitutions` filled
/// in, so comparing it against the declared `${Key}` text would report every templated
/// workflow as drifted when nothing drifted.
pub(super) async fn actual_configuration(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let Some(machine) = described(ctx, region, physical_resource_id).await? else {
        return Ok(None);
    };
    let mut reported = Map::new();
    for (property, member) in [
        ("RoleArn", "roleArn"),
        ("StateMachineName", "name"),
        ("StateMachineType", "type"),
    ] {
        if let Some(value) = machine.get(member).filter(|value| !value.is_null()) {
            reported.insert(property.to_string(), value.clone());
        }
    }
    Ok(Some(Value::Object(reported)))
}

/// What `DescribeStateMachine` answers for `physical_resource_id`, or `None` when Step
/// Functions holds no state machine under that ARN.
async fn described(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "stateMachineArn": physical_resource_id });
    match call_sfn(ctx, region, "DescribeStateMachine", &request).await {
        Ok(answer) => Ok(Some(answer)),
        Err(refusal) if refusal.code == STATE_MACHINE_DOES_NOT_EXIST => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The NAME the machine is created under: the `StateMachineName` the template asked for, or
/// the `<stack>-<logical id>-<suffix>` name CloudFormation generated for a resource that
/// names none.
///
/// The generated name travels as the resource's `Name` attribute, because this type's
/// physical id is the machine's ARN; a record with no such attribute (one whose attribute
/// budget was spent) falls back to the last segment of that ARN, which is the name.
fn state_machine_name(resource: &StoredResource) -> String {
    if let Some(declared) = text_property(&resource.properties, "StateMachineName") {
        return declared;
    }
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

/// The role the machine runs its executions as, which the type requires ("RoleArn:
/// Required: Yes"): a declaration naming none describes no state machine that could be
/// created, so it is refused here rather than with a request Step Functions would reject.
fn role_arn(resource: &StoredResource) -> Result<String, Refusal> {
    text_property(&resource.properties, "RoleArn").ok_or_else(|| Refusal {
        code: "InvalidRequest".to_string(),
        message: format!(
            "{} requires a RoleArn, which this declaration does not state",
            resource.resource_type
        ),
    })
}

/// The Amazon States Language document the declaration states, with its placeholder
/// variables replaced.
///
/// The forms are ALTERNATIVES — the type takes one definition — so a declaration stating
/// more than one states no single workflow and is refused rather than deployed with
/// whichever form this emulator happened to prefer.
fn definition(properties: &Value) -> Result<String, Refusal> {
    let stated = match (
        text_property(properties, "DefinitionString"),
        properties.get("Definition").filter(|value| !value.is_null()),
    ) {
        (Some(_), Some(_)) => {
            return Err(Refusal {
                code: "InvalidRequest".to_string(),
                message: format!(
                    "{STEP_FUNCTIONS_STATE_MACHINE} states both a Definition and a \
                     DefinitionString, which name two workflows where the type takes one"
                ),
            })
        }
        (Some(text), None) => text,
        // "Definition: the Amazon States Language definition of the state machine": the
        // template carries the document itself, so its JSON text is the definition.
        (None, Some(document)) => document.to_string(),
        (None, None) => {
            return Err(Refusal {
                code: "InvalidRequest".to_string(),
                message: format!(
                    "{STEP_FUNCTIONS_STATE_MACHINE} requires a Definition or a DefinitionString, \
                     which this declaration does not state"
                ),
            })
        }
    };
    substituted(&stated, properties)
}

/// `definition` with every `${Key}` the declaration's `DefinitionSubstitutions` names
/// replaced by the value declared for that key.
///
/// "DefinitionSubstitutions: a map (string to string) that specifies the mappings for
/// placeholder variables in the state machine definition. This enables the customer to
/// inject values obtained at runtime, for example from intrinsic functions, in the state
/// machine definition." A placeholder no substitution names is left exactly as it stands:
/// the document is the caller's, and an unnamed variable is not this emulator's to empty.
///
/// ONE pass over the definition, with one map lookup per placeholder it carries, and the
/// text a substitution emits is NOT scanned again: a value that itself looks like a
/// placeholder is a value, not a further substitution. That is what keeps the work and the
/// result linear in the definition's own length whatever the declaration's substitution map
/// holds — a re-scanning replacement would let one key's value be expanded by the next,
/// which is growth no bound on the map could cap. The result is bounded as the definition
/// itself is ([`MAX_DEFINITION_BYTES`]), and a declaration whose substitutions take it past
/// that is refused before anything is sent.
fn substituted(definition: &str, properties: &Value) -> Result<String, Refusal> {
    let Some(substitutions) = properties
        .get("DefinitionSubstitutions")
        .and_then(Value::as_object)
    else {
        return too_long(definition.len()).map_or_else(|| Ok(definition.to_string()), Err);
    };
    let mut document = String::with_capacity(definition.len());
    let mut rest = definition;
    while let Some(at) = rest.find(PLACEHOLDER_OPENING) {
        document.push_str(&rest[..at]);
        let after = &rest[at + PLACEHOLDER_OPENING.len()..];
        let Some(end) = after.find(PLACEHOLDER_CLOSING) else {
            // Text that opens a placeholder and never closes it is not one: it is the
            // document's own text, and it is kept exactly as it stands.
            document.push_str(PLACEHOLDER_OPENING);
            document.push_str(after);
            rest = "";
            break;
        };
        let key = &after[..end];
        match substitutions.get(key).and_then(scalar_text) {
            Some(replacement) => document.push_str(&replacement),
            None => {
                document.push_str(PLACEHOLDER_OPENING);
                document.push_str(key);
                document.push_str(PLACEHOLDER_CLOSING);
            }
        }
        rest = &after[end + PLACEHOLDER_CLOSING.len()..];
        if let Some(refusal) = too_long(document.len()) {
            return Err(refusal);
        }
    }
    document.push_str(rest);
    match too_long(document.len()) {
        Some(refusal) => Err(refusal),
        None => Ok(document),
    }
}

/// The refusal for a definition longer than a state machine's may be, or `None` for one
/// inside it.
fn too_long(bytes: usize) -> Option<Refusal> {
    (bytes > MAX_DEFINITION_BYTES).then(|| Refusal {
        code: "InvalidRequest".to_string(),
        message: format!(
            "the definition this declaration states is {bytes} bytes, more than the \
             {MAX_DEFINITION_BYTES} a state machine definition may carry"
        ),
    })
}

/// One scalar property value as the text a request carries it as: a string as it stands, and
/// a number or a boolean as its own JSON text, which is how a substitution of one reaches a
/// definition.
fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(_) | Value::Bool(_) => Some(value.to_string()),
        _ => None,
    }
}

/// One text property of a declaration, or `None` when it states none (or states it empty,
/// which names nothing).
fn text_property(properties: &Value, property: &str) -> Option<String> {
    properties
        .get(property)
        .and_then(scalar_text)
        .filter(|value| !value.is_empty())
}

/// `tags` as the list `CreateStateMachine`'s `tags` member takes, whose `Tag` shape spells
/// its members `key` and `value`.
fn tag_list(tags: &[(String, String)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| {
                serde_json::json!({ "key": key, "value": value })
            })
            .collect(),
    )
}

/// `name` with its initial letter lower-cased, which is how Step Functions spells the member
/// a property of this type names (`LoggingConfiguration` against `loggingConfiguration`).
fn lower_camel(name: &str) -> String {
    let mut characters = name.chars();
    match characters.next() {
        Some(first) => first.to_lowercase().chain(characters).collect(),
        None => String::new(),
    }
}

/// One configuration property with every member name inside it in lower camel case, which is
/// how this service spells the very same members the property names in upper camel case
/// (`Destinations: [{CloudWatchLogsLogGroup: {LogGroupArn}}]` is `destinations:
/// [{cloudWatchLogsLogGroup: {logGroupArn}}]`).
///
/// Applied only to [`CONFIGURATION_PROPERTIES`], every name inside which is modelled: a
/// property carrying a map of caller data would have its keys re-cased too, and none of the
/// three carries one. The walk is bounded because the document is the caller's.
fn lower_camel_members(value: &Value, depth: usize) -> Result<Value, Refusal> {
    if depth >= MAX_CONFIGURATION_NESTING {
        return Err(Refusal {
            code: "InvalidRequest".to_string(),
            message: format!(
                "a configuration of {STEP_FUNCTIONS_STATE_MACHINE} nests more than the \
                 {MAX_CONFIGURATION_NESTING} levels this emulator reads"
            ),
        });
    }
    match value {
        Value::Object(members) => {
            let mut converted = Map::new();
            for (name, member) in members {
                converted.insert(lower_camel(name), lower_camel_members(member, depth + 1)?);
            }
            Ok(Value::Object(converted))
        }
        Value::Array(elements) => {
            let mut converted = Vec::with_capacity(elements.len());
            for element in elements {
                converted.push(lower_camel_members(element, depth + 1)?);
            }
            Ok(Value::Array(converted))
        }
        other => Ok(other.clone()),
    }
}

/// Call one AWS Step Functions operation on the awsJson1_0 wire its model declares.
async fn call_sfn(
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

/// The refusal for a declared property this module does not apply, naming which property it
/// was: refusing is what keeps a state machine from being reported as created with less than
/// its declaration asked for.
fn refuse_unmapped_properties(resource: &StoredResource) -> Result<(), Refusal> {
    refuse_unmapped_declaration(&resource.resource_type, &resource.properties)
}

/// The same, over the properties a deployment DECLARES for a resource that already exists.
fn refuse_unmapped_declaration(resource_type: &str, properties: &Value) -> Result<(), Refusal> {
    let Value::Object(properties) = properties else {
        return Ok(());
    };
    for property in properties.keys() {
        if STATE_MACHINE_PROPERTIES.contains(&property.as_str()) {
            continue;
        }
        return Err(Refusal {
            code: "NotImplemented".to_string(),
            message: format!("this emulator does not apply the {property} property of {resource_type}"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A resource record of this type, as the naming rules leave one.
    fn declared(properties: Value) -> StoredResource {
        StoredResource {
            logical_id: "Poller".to_string(),
            resource_type: STEP_FUNCTIONS_STATE_MACHINE.to_string(),
            physical_resource_id:
                "arn:aws:states:us-west-2:000000000000:stateMachine:stack-Poller-ABC123".to_string(),
            attributes: BTreeMap::new(),
            metadata: None,
            properties,
            materialised: false,
            deletion_policy: None,
            update_replace_policy: None,
        }
    }

    /// Each form the type states the workflow in becomes the one `definition` the API takes,
    /// with the declaration's placeholder variables replaced wherever they appear.
    #[test]
    fn every_definition_form_becomes_the_one_api_member() {
        let cases: &[(&str, Value, &str)] = &[
            (
                "a definition as text",
                serde_json::json!({ "DefinitionString": r#"{"StartAt":"P"}"# }),
                r#"{"StartAt":"P"}"#,
            ),
            (
                "a definition as a template object",
                serde_json::json!({ "Definition": { "StartAt": "P" } }),
                r#"{"StartAt":"P"}"#,
            ),
            (
                "a placeholder the substitutions name",
                serde_json::json!({
                    "DefinitionString": r#"{"Resource":"${JobArn}","Next":"${JobArn}"}"#,
                    "DefinitionSubstitutions": { "JobArn": "arn:aws:lambda:::function:j" },
                }),
                r#"{"Resource":"arn:aws:lambda:::function:j","Next":"arn:aws:lambda:::function:j"}"#,
            ),
            (
                "a placeholder no substitution names",
                serde_json::json!({
                    "DefinitionString": r#"{"Resource":"${Missing}"}"#,
                    "DefinitionSubstitutions": { "JobArn": "a" },
                }),
                r#"{"Resource":"${Missing}"}"#,
            ),
            (
                "a substitution whose value is a number",
                serde_json::json!({
                    "DefinitionString": r#"{"Seconds":${Wait}}"#,
                    "DefinitionSubstitutions": { "Wait": 5 },
                }),
                r#"{"Seconds":5}"#,
            ),
            (
                // A value is a VALUE: text inside one that looks like a placeholder is not
                // substituted again, so no chain of substitutions can expand the document.
                "a substitution whose value names another placeholder",
                serde_json::json!({
                    "DefinitionString": r#"{"Resource":"${First}"}"#,
                    "DefinitionSubstitutions": { "First": "${Second}${Second}", "Second": "x" },
                }),
                r#"{"Resource":"${Second}${Second}"}"#,
            ),
            (
                "text that opens a placeholder and never closes it",
                serde_json::json!({
                    "DefinitionString": r#"{"Comment":"${unclosed"}"#,
                    "DefinitionSubstitutions": { "unclosed": "never" },
                }),
                r#"{"Comment":"${unclosed"}"#,
            ),
        ];
        for (case, properties, expected) in cases {
            let built = definition(properties);
            assert_eq!(
                built.as_deref().ok(),
                Some(*expected),
                "case {case:?}: expected the definition {expected:?}, got {built:?}"
            );
        }
    }

    /// A declaration whose substitutions take the definition past the length a state machine
    /// definition may carry is refused, rather than built and sent.
    #[test]
    fn a_definition_past_the_modelled_length_is_refused() {
        let built = definition(&serde_json::json!({
            "DefinitionString": "${Big}",
            "DefinitionSubstitutions": { "Big": "x".repeat(MAX_DEFINITION_BYTES + 1) },
        }));
        assert!(
            built.is_err(),
            "case {:?}: expected a refusal naming the length, got {built:?}",
            "a substituted definition past the bound"
        );
        let inside = definition(&serde_json::json!({
            "DefinitionString": "${Big}",
            "DefinitionSubstitutions": { "Big": "x".repeat(MAX_DEFINITION_BYTES) },
        }));
        assert_eq!(
            inside.as_ref().map(String::len).ok(),
            Some(MAX_DEFINITION_BYTES),
            "case {:?}: expected the definition at the bound to be built, got {:?}",
            "a substituted definition at the bound",
            inside.as_ref().map(String::len)
        );
    }

    /// A declaration stating no workflow at all, and one stating no role, describe no state
    /// machine that could be created: each is refused rather than sent.
    #[test]
    fn a_declaration_missing_an_api_requirement_is_refused() {
        let no_definition = definition(&serde_json::json!({ "RoleArn": "arn:aws:iam::0:role/r" }));
        assert!(
            no_definition.is_err(),
            "case {:?}: expected a refusal naming the missing definition, got {no_definition:?}",
            "no definition"
        );
        let no_role = role_arn(&declared(
            serde_json::json!({ "DefinitionString": "{}" }),
        ));
        assert!(
            no_role.is_err(),
            "case {:?}: expected a refusal naming the missing role, got {no_role:?}",
            "no role"
        );
    }

    /// A configuration property is sent under this service's own spelling, members and all.
    #[test]
    fn a_configuration_is_sent_in_lower_camel_case() {
        let configuration = serde_json::json!({
            "Level": "ALL",
            "IncludeExecutionData": true,
            "Destinations": [{ "CloudWatchLogsLogGroup": { "LogGroupArn": "arn:aws:logs:::g" } }],
        });
        let converted = lower_camel_members(&configuration, 0).expect("the walk converts it");
        assert_eq!(
            converted,
            serde_json::json!({
                "level": "ALL",
                "includeExecutionData": true,
                "destinations": [{ "cloudWatchLogsLogGroup": { "logGroupArn": "arn:aws:logs:::g" } }],
            }),
            "expected every member in this service's own spelling, got {converted}"
        );
    }

    /// The machine is created under the declared name where there is one, and under the
    /// identifier CloudFormation computed where the declaration names none.
    #[test]
    fn the_declared_name_names_the_machine() {
        let named = state_machine_name(&declared(
            serde_json::json!({ "StateMachineName": "job-poller" }),
        ));
        assert_eq!(
            named, "job-poller",
            "case {:?}: expected the declared name, got {named:?}",
            "a declared name"
        );
        let generated = state_machine_name(&declared(serde_json::json!({})));
        assert_eq!(
            generated, "stack-Poller-ABC123",
            "case {:?}: expected the generated name, got {generated:?}",
            "no name at all"
        );
    }

    /// A property the type does not publish is refused rather than dropped.
    #[test]
    fn an_unpublished_property_is_refused() {
        let refused = refuse_unmapped_properties(&declared(serde_json::json!({
            "DefinitionString": "{}",
            "DefinitionS3Location": { "Bucket": "b", "Key": "k" },
        })));
        assert!(
            refused.is_err(),
            "case {:?}: expected a refusal naming the unapplied property, got {refused:?}",
            "DefinitionS3Location"
        );
    }
}
