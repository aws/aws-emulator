// SPDX-License-Identifier: CC0-1.0

//! The per-type provisioning arms of Amazon Bedrock Agent's build-time control plane: the
//! knowledge base an `AWS::Bedrock::KnowledgeBase` declares. One file of [`super`]'s
//! per-type arms, dispatched to by its `Provisioner`, beside the guardrail arms of Amazon
//! Bedrock's own control plane ([`super::bedrock_guardrails`]).
//!
//! ## Why the knowledge base is really created
//!
//! "Ref returns the knowledge base ID", and that id is the handle every knowledge-base API
//! takes — `GetKnowledgeBase`, `UpdateKnowledgeBase`, `DeleteKnowledgeBase`, the data-source
//! calls under it and the retrieval calls against it all address one by it. A template
//! publishes it through an output, an SSM parameter or another resource's property, and the
//! first thing a client does with the deployed stack is call that service with it. A stack
//! that reported the knowledge base created while nothing created it publishes a handle that
//! resolves to nothing, so that first call fails — the stack completed and the client is
//! broken. Creating the knowledge base where it belongs is what makes the id the stack
//! publishes one the owning service answers for.
//!
//! ## The identifier is the service's
//!
//! `CreateKnowledgeBase` takes the knowledge base's NAME and answers with the
//! `knowledgeBaseId` it minted — modelled `^[0-9a-zA-Z]{10}$`, exactly ten alphanumeric
//! characters — together with the `knowledge-base/<id>` ARN built around it. No name
//! CloudFormation decides, generated or declared, can satisfy that pattern, so the
//! identifier in force is the one that service assigned: it IS this resource's physical id
//! ([`super::super::template::identity_is_service_assigned`] names the type), and the
//! ten-character stand-in the naming rules computed is only what a reference was resolved
//! against until the create answered. The NAME the knowledge base is created under is still
//! CloudFormation's — the `Name` the declaration states, or the generated
//! `<stack>-<logical id>-<suffix>` name ([`knowledge_base_name`]).
//!
//! ## What the declaration is
//!
//! Every property of this type is the member of the same name with a lower-cased initial all
//! the way down (`RoleArn` is `roleArn`, `KnowledgeBaseConfiguration.Type` is
//! `knowledgeBaseConfiguration.type`), so the declared document IS the request once its
//! member names are re-cased ([`camel_cased`]) — the same relation the guardrail arms beside
//! this one rely on. The declared `Tags` are a MAP for this type, which is the shape
//! `CreateKnowledgeBase`'s own `tags` member takes, so they travel beside the ones
//! CloudFormation gives the resource ([`tags_of`]).
//!
//! No `clientToken` is sent. That member is an `@idempotencyToken`, so a token derived from
//! the resource would make the create of a REPLACEMENT knowledge base — the one a changed
//! embedding model asks for — replay the knowledge base the stack already had instead of
//! minting a new one.

use super::*;

/// The knowledge base COLLECTION `CreateKnowledgeBase` is bound to (`PUT /knowledgebases/`,
/// trailing slash included, as that operation's own `@http` binding spells it); one
/// knowledge base is addressed under it by its `knowledgeBaseId`
/// (`GET`/`PUT`/`DELETE /knowledgebases/{knowledgeBaseId}`).
const KNOWLEDGE_BASES_URI: &str = "/knowledgebases/";

/// The member the answer carries the knowledge base in: every one of these operations
/// answers a `knowledgeBase` structure.
const KNOWLEDGE_BASE_MEMBER: &str = "knowledgeBase";

/// The member `CreateKnowledgeBase` and `UpdateKnowledgeBase` take the knowledge base's name
/// in, which is the one member CloudFormation decides.
const NAME_MEMBER: &str = "name";

/// The member the declared `Tags` map reaches `CreateKnowledgeBase` as — its own `tags`,
/// which `UpdateKnowledgeBase` does not take.
const TAGS_MEMBER: &str = "tags";

/// The `KnowledgeBaseStatus` a knowledge base is ready in: "ACTIVE – The knowledge base is
/// ready to be queried".
const KNOWLEDGE_BASE_ACTIVE: &str = "ACTIVE";

/// The transient `KnowledgeBaseStatus` values a knowledge base is still coming up in:
/// "CREATING – The knowledge base is being created" and "UPDATING – The knowledge base is
/// being updated".
const KNOWLEDGE_BASE_COMING_UP: [&str; 2] = ["CREATING", "UPDATING"];

/// The `KnowledgeBaseStatus` a knowledge base being removed is reported in: "DELETING – The
/// knowledge base is being deleted".
const KNOWLEDGE_BASE_DELETING: &str = "DELETING";

/// The members an answered knowledge base reports, as (member, attribute): the values this
/// type publishes as return values, read off the answer rather than recomputed here.
///
/// `failureReasons` is not among them — it is a LIST, and the attribute map is
/// string-valued, so reporting one entry as the whole of it would be worse than reporting
/// none; the reasons are read for the diagnosis of a knowledge base that FAILED instead
/// ([`knowledge_base_readiness`]).
const KNOWLEDGE_BASE_ATTRIBUTES: &[(&str, &str)] = &[
    ("knowledgeBaseId", "KnowledgeBaseId"),
    ("knowledgeBaseArn", "KnowledgeBaseArn"),
    ("status", "Status"),
    ("createdAt", "CreatedAt"),
    ("updatedAt", "UpdatedAt"),
];

/// The properties a knowledge base's own read reports back under the same names, for drift
/// detection, as (property, member): the three scalars `GetKnowledgeBase` answers that a
/// declaration also states.
///
/// The configuration documents are left out: what the read reports are that service's own
/// member names, and a property the read does not report is not evidence of anything
/// ([`super::drift`]).
const KNOWLEDGE_BASE_REPORTED_PROPERTIES: &[(&str, &str)] = &[
    ("Name", "name"),
    ("Description", "description"),
    ("RoleArn", "roleArn"),
];

/// The error code Amazon Bedrock Agent answers when the name a create asks for is one
/// this account's knowledge bases already hold: `CreateKnowledgeBase`'s modelled 409
/// (`ConflictException`), whose one cause for this operation is the name — the only value
/// of the request that has to be unique in the account.
const BEDROCK_CONFLICT: &str = "ConflictException";

/// Create the knowledge base an `AWS::Bedrock::KnowledgeBase` declares, so a client that
/// deployed the stack and calls `GetKnowledgeBase` with the id the stack publishes reads the
/// knowledge base back — and so every hop off it (its data sources, a retrieval against it)
/// addresses a knowledge base that exists.
///
/// The identity is Amazon Bedrock Agent's: the `knowledgeBaseId` it minted becomes this
/// resource's physical id — the primary identifier of the type, and the handle every
/// knowledge-base API takes — and the id, the ARN, the status and the two instants it
/// answered are the resource's attributes.
///
/// A create that service refuses because the NAME is taken is reported as the collision it
/// is ([`name_taken`]): the class a client branches on for "the resource is already there"
/// is `AlreadyExists`, which is what this type's handler puts such a refusal in, and it is
/// what the create of a REPLACEMENT knowledge base runs into — the replacement is created
/// before the knowledge base it replaces is deleted, so a declaration that changes an
/// embedding model while keeping the `Name` asks for a second knowledge base under a name
/// the account still holds.
pub(super) async fn create_bedrock_knowledge_base(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = declared_knowledge_base_members(&resource.properties)?;
    let name = knowledge_base_name(resource);
    request.insert(NAME_MEMBER.to_string(), Value::String(name.clone()));
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert(TAGS_MEMBER.to_string(), Value::Object(tags_as_map(&tags)));
    }
    let answered = call_rest_json(
        ctx,
        BEDROCK_SIGNING_NAME,
        region,
        "PUT",
        KNOWLEDGE_BASES_URI,
        Some(&Value::Object(request)),
    )
    .await
    .map_err(|refusal| name_taken(resource, &name, refusal))?;
    let attributes = knowledge_base_attributes(&answered)?;
    Ok(Created {
        // Amazon Bedrock Agent assigned the knowledge base's identity, so the id it answered
        // is what `Ref` resolves to and what every call on the knowledge base is addressed
        // by.
        physical_resource_id: attributes.get("KnowledgeBaseId").cloned(),
        attributes,
    })
}

/// A create Amazon Bedrock Agent refused because the knowledge base's NAME is already
/// taken, as this type's handler reports it: the `AlreadyExists` class — the one a client
/// branches on to rename or stage the change — and the handler's own sentence naming the
/// type and the identifier it asked for. Every other refusal is passed on as it came.
fn name_taken(resource: &StoredResource, name: &str, refusal: Refusal) -> Refusal {
    if refusal.code != BEDROCK_CONFLICT {
        return refusal;
    }
    Refusal {
        code: ALREADY_EXISTS.to_string(),
        message: crate::state::early_validation::already_exists(&resource.resource_type, name),
    }
}

/// Whether Amazon Bedrock Agent has finished bringing up the knowledge base a stack created.
///
/// `CreateKnowledgeBase` answers `202 Accepted` with the knowledge base `CREATING` — in which
/// it cannot be queried — so `ACTIVE` is the state the resource's creation is finished in,
/// `CREATING` and `UPDATING` the states it is still coming up in, and every other status is
/// one no wait will bring it out of: a knowledge base that failed, or one being deleted out
/// of this stack's reach. A knowledge base that service no longer holds is in the same
/// position, nothing being left to create it. The reasons that service reports are the
/// diagnosis of a failure, so they travel with it.
pub(super) async fn knowledge_base_readiness(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Readiness, Refusal> {
    let Some(knowledge_base) = read_knowledge_base(ctx, region, physical_resource_id).await?
    else {
        return Ok(Readiness::Failed(format!(
            "Amazon Bedrock holds no knowledge base {physical_resource_id}"
        )));
    };
    let status = reported_text(&knowledge_base, "status").unwrap_or_default();
    if status == KNOWLEDGE_BASE_ACTIVE {
        return Ok(Readiness::Ready);
    }
    if KNOWLEDGE_BASE_COMING_UP.contains(&status.as_str()) {
        return Ok(Readiness::Pending(status));
    }
    Ok(Readiness::Failed(failed_reason(
        physical_resource_id,
        &status,
        &knowledge_base,
    )))
}

/// The attributes the owning service publishes for a knowledge base it now reports up: its
/// own read of the knowledge base, whose `status` and `updatedAt` are not the ones the create
/// answered with ([`KNOWLEDGE_BASE_ATTRIBUTES`]).
pub(super) async fn knowledge_base_ready_attributes(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<BTreeMap<String, String>, Refusal> {
    let Some(knowledge_base) = read_knowledge_base(ctx, region, physical_resource_id).await?
    else {
        return Ok(BTreeMap::new());
    };
    Ok(reported_attributes(&knowledge_base))
}

/// Whether Amazon Bedrock Agent has finished removing the knowledge base a stack deleted.
///
/// `DeleteKnowledgeBase` answers `202 Accepted` with the knowledge base `DELETING` — "the
/// knowledge base is being deleted" — so it is still there when the delete answers, and the
/// deletion is finished once that service no longer holds it.
pub(super) async fn knowledge_base_removal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Removal, Refusal> {
    match read_knowledge_base(ctx, region, physical_resource_id).await? {
        None => Ok(Removal::Gone),
        Some(knowledge_base) => Ok(Removal::Pending(
            reported_text(&knowledge_base, "status")
                .unwrap_or_else(|| KNOWLEDGE_BASE_DELETING.to_string()),
        )),
    }
}

/// Apply a changed declaration to the knowledge base the stack created, through
/// `UpdateKnowledgeBase` — the one call that changes a knowledge base that exists.
///
/// It takes the knowledge base's id together with the whole declared configuration, so a
/// member the declaration no longer states leaves the knowledge base without it rather than
/// keeping the one an earlier deployment gave it. The knowledge base keeps the id that
/// service minted, so the handle the stack publishes is unchanged.
///
/// `Ok(None)` is a change that call cannot make, so the deployment reports it as not applied
/// rather than as a success that dropped it: a changed `Tags` map, which `UpdateKnowledgeBase`
/// does not carry — a knowledge base's tags are that service's own tagging calls.
pub(super) async fn update_bedrock_knowledge_base(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    if declared.get("Tags") != resource.properties.get("Tags") {
        return Ok(None);
    }
    let mut request = declared_knowledge_base_members(declared)?;
    request.insert(
        NAME_MEMBER.to_string(),
        Value::String(declared_name_or(declared, resource)),
    );
    let answered = call_rest_json(
        ctx,
        BEDROCK_SIGNING_NAME,
        region,
        "PUT",
        &knowledge_base_uri(&resource.physical_resource_id),
        Some(&Value::Object(request)),
    )
    .await?;
    Ok(Some(Created::attributes(knowledge_base_attributes(
        &answered,
    )?)))
}

/// Delete the knowledge base a stack created, by the id it was created under — this
/// resource's physical id. One the service no longer holds (`ResourceNotFoundException`,
/// judged on the CODE and never on a message that carries the request's own values) is
/// already in the state the delete asks for.
pub(super) async fn delete_bedrock_knowledge_base(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    match call_rest_json(
        ctx,
        BEDROCK_SIGNING_NAME,
        region,
        "DELETE",
        &knowledge_base_uri(physical_resource_id),
        None,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == BEDROCK_RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The knowledge base's configuration as Amazon Bedrock Agent reports it, for drift
/// detection: `GetKnowledgeBase`'s own answer, read by the id that is this resource's
/// physical id ([`KNOWLEDGE_BASE_REPORTED_PROPERTIES`]). One it no longer holds was deleted.
pub(super) async fn describe_bedrock_knowledge_base(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    Ok(read_knowledge_base(ctx, region, physical_resource_id)
        .await?
        .map(|knowledge_base| {
            reported_as_properties(&knowledge_base, KNOWLEDGE_BASE_REPORTED_PROPERTIES)
        }))
}

/// The knowledge base Amazon Bedrock Agent holds under `knowledge_base_id`, or `None` when
/// it holds none (`ResourceNotFoundException`, judged on the CODE and never on a message).
async fn read_knowledge_base(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    knowledge_base_id: &str,
) -> Result<Option<Value>, Refusal> {
    match call_rest_json(
        ctx,
        BEDROCK_SIGNING_NAME,
        region,
        "GET",
        &knowledge_base_uri(knowledge_base_id),
        None,
    )
    .await
    {
        Ok(answer) => Ok(answer.get(KNOWLEDGE_BASE_MEMBER).cloned()),
        Err(refusal) if refusal.code == BEDROCK_RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The URI one knowledge base is addressed at: the collection plus the `knowledgeBaseId`
/// `@httpLabel`, escaped as a single path segment ([`percent_encoded`]) so a value carrying
/// URI structure addresses one knowledge base rather than another path.
fn knowledge_base_uri(knowledge_base_id: &str) -> String {
    format!(
        "{KNOWLEDGE_BASES_URI}{}",
        percent_encoded(knowledge_base_id)
    )
}

/// One declared property document as the request members Amazon Bedrock Agent takes: every
/// member name re-cased ([`camel_cased`]), with the tags left out — the create adds them as
/// the map that call takes, and the update does not take them at all — so one reader serves
/// both.
fn declared_knowledge_base_members(declared: &Value) -> Result<Map<String, Value>, Refusal> {
    let mut request = match camel_cased(declared, 0)? {
        Value::Object(members) => members,
        // A declaration that is not an object declares no member of the request.
        _ => Map::new(),
    };
    request.remove(TAGS_MEMBER);
    Ok(request)
}

/// The NAME the knowledge base is created under: the `Name` the template asked for, or the
/// `<stack>-<logical id>-<suffix>` name CloudFormation generated for a declaration that
/// states none — which is this resource's physical id until that service assigns one.
fn knowledge_base_name(resource: &StoredResource) -> String {
    declared_name_or(&resource.properties, resource)
}

/// The name one declared document states, or the one CloudFormation decided for the resource
/// ([`knowledge_base_name`]).
///
/// The fallback is the resource's own `Name` attribute where the record carries one — the
/// physical id being the id that service minted once the create has answered — and the
/// physical id itself before then.
fn declared_name_or(declared: &Value, resource: &StoredResource) -> String {
    if let Some(name) = declared
        .get("Name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
    {
        return name.to_string();
    }
    resource
        .attributes
        .get("Name")
        .cloned()
        .unwrap_or_else(|| resource.physical_resource_id.clone())
}

/// The attributes one answered knowledge base reports ([`KNOWLEDGE_BASE_ATTRIBUTES`]).
///
/// An answer carrying no id is reported as a refusal rather than recorded as a created
/// resource: there would be no handle a client could call Amazon Bedrock Agent with.
fn knowledge_base_attributes(answered: &Value) -> Result<BTreeMap<String, String>, Refusal> {
    let Some(knowledge_base) = answered.get(KNOWLEDGE_BASE_MEMBER) else {
        return Err(no_knowledge_base_answered());
    };
    let attributes = reported_attributes(knowledge_base);
    if !attributes.contains_key("KnowledgeBaseId") {
        return Err(no_knowledge_base_answered());
    }
    Ok(attributes)
}

/// The attributes one reported knowledge base structure carries
/// ([`KNOWLEDGE_BASE_ATTRIBUTES`]).
fn reported_attributes(knowledge_base: &Value) -> BTreeMap<String, String> {
    let mut attributes = BTreeMap::new();
    for (member, attribute) in KNOWLEDGE_BASE_ATTRIBUTES {
        if let Some(value) = reported_text(knowledge_base, member) {
            attributes.insert((*attribute).to_string(), value);
        }
    }
    attributes
}

/// The refusal an answer carrying no identified knowledge base is reported as.
fn no_knowledge_base_answered() -> Refusal {
    Refusal {
        code: "InternalFailure".to_string(),
        message: "Amazon Bedrock Agent answered a knowledge base without an id".to_string(),
    }
}

/// Why a knowledge base this stack created cannot come up: the status that service reports it
/// in, with the reasons it reports for it where there are any
/// (`KnowledgeBase$failureReasons`: "a list of reasons that the API operation on the
/// knowledge base failed").
fn failed_reason(knowledge_base_id: &str, status: &str, knowledge_base: &Value) -> String {
    let reasons: Vec<String> = knowledge_base
        .get("failureReasons")
        .and_then(Value::as_array)
        .map(|reasons| reasons.iter().filter_map(scalar_text).collect())
        .unwrap_or_default();
    let reported =
        format!("Amazon Bedrock reports knowledge base {knowledge_base_id} as {status}");
    if reasons.is_empty() {
        return reported;
    }
    format!("{reported}: {}", reasons.join("; "))
}
