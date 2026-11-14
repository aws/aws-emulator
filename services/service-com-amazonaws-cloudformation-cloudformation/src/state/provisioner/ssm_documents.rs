// SPDX-License-Identifier: CC0-1.0

//! The per-type provisioning arms of AWS Systems Manager's DOCUMENT store: the document an
//! `AWS::SSM::Document` declares. One file of [`super`]'s per-type arms, dispatched to by
//! its `Provisioner`.
//!
//! A document is a NAMED resource of that service: `CreateDocument` takes the `Name` the
//! document is created under — the value this type's `Ref` returns and the handle every
//! Systems Manager call on a document takes, `StartAutomationExecution`'s `DocumentName`
//! included — so the document is created under exactly the name CloudFormation decided for
//! the resource and the stand-in the naming rules computed is the real identifier
//! (`smithy_query(operation="CreateDocument,UpdateDocument,DeleteDocument,GetDocument",
//! model_path="services/service-com-amazonaws-ssm-amazonssm/smithy-model/\
//! com-amazonaws-ssm-amazonssm.json")`).
//!
//! `Content` is the one property the template states as a DOCUMENT and the API takes as
//! TEXT ("the content for the new SSM document in JSON or YAML format",
//! `CreateDocument$Content`), so it is rendered with the one JSON-to-text rule this
//! catalogue has ([`document_text`]); every other property is a member of the same name.

use super::*;

/// `AWS::SSM::Document`: a document of AWS Systems Manager. "Ref returns the document name" —
/// the handle every call of that service on a document takes (`GetDocument$Name`, and
/// `StartAutomationExecution`'s `DocumentName` for a runbook) — so the document a stack
/// declares is the one that service resolves under the name the stack publishes, which is this
/// resource's physical id. The name is the document's identity and CloudFormation decides it,
/// so the type needs no entry in `super::super::template::identity_is_service_assigned`:
/// nothing of that identifier is Systems Manager's to mint.
///
/// Visible to the rest of this service's state because the type's own NAME lives here, beside
/// the arms that provision it — the naming rules and the required-property catalogue key their
/// entries for the type by it.
pub(in crate::state) const SSM_DOCUMENT: &str = "AWS::SSM::Document";

/// The Systems Manager operations the `AWS::SSM::Document` provider asks for, by name: the
/// one call that creates a document, the one that adds a new version to one, the one that
/// makes a version the default, the one that reads a document's content back and the one
/// that removes it.
///
/// Named rather than spelled at each call because the provider REPORTS which of them was
/// refused ([`handler_refusal`]), so the name in the report and the name in the
/// `X-Amz-Target` cannot drift apart.
const CREATE_DOCUMENT: &str = "CreateDocument";
const UPDATE_DOCUMENT: &str = "UpdateDocument";
const UPDATE_DOCUMENT_DEFAULT_VERSION: &str = "UpdateDocumentDefaultVersion";
const DESCRIBE_DOCUMENT: &str = "DescribeDocument";
const GET_DOCUMENT: &str = "GetDocument";
const DELETE_DOCUMENT: &str = "DeleteDocument";

/// The `DocumentStatus` a document that is in place carries ("Active — The document is
/// ready for use"), and the one a document that will never be carries ("Failed — The
/// document couldn't be created"), from that enum's own values
/// (`smithy_query(query="shape", shape="DocumentStatus")`).
const DOCUMENT_STATUS_ACTIVE: &str = "Active";
const DOCUMENT_STATUS_FAILED: &str = "Failed";

/// The error code Systems Manager answers for a document it does not hold (`InvalidDocument`:
/// the modelled 404 of its read, its update and its delete).
const INVALID_DOCUMENT: &str = "InvalidDocument";

/// The error code it answers for an update whose content is the content the document already
/// carries (`DuplicateDocumentContent`), which is what a re-deploy of an UNCHANGED
/// declaration asks for: the document already holds exactly what the declaration states, so
/// the resource is already in the state the update asks for.
const DUPLICATE_DOCUMENT_CONTENT: &str = "DuplicateDocumentContent";

/// The error code it answers for an update that would add a version under a version NAME the
/// document already holds (`DuplicateDocumentVersionName`) — the same position
/// [`DUPLICATE_DOCUMENT_CONTENT`] is in for a declaration that states a `VersionName`.
const DUPLICATE_DOCUMENT_VERSION_NAME: &str = "DuplicateDocumentVersionName";

/// The member `CreateDocument` and `UpdateDocument` answer the document under, and the
/// member of it that carries the version the call produced.
const DOCUMENT_DESCRIPTION: &str = "DocumentDescription";
const DOCUMENT_VERSION: &str = "DocumentVersion";

/// The `AWS::SSM::Document` properties `CreateDocument` takes under the SAME member name.
///
/// Source: AWS's CloudFormation resource reference for the type against
/// `smithy_query(operation="CreateDocument")` — every property of the type is a member of
/// that request, spelled identically, except the three that are not passed straight through:
/// `Name`, which CloudFormation decides and which is this resource's physical id; `Tags`,
/// which reaches the request as the `[{Key, Value}]` list that model's own `Tag` shape
/// spells; and `UpdateMethod`, which says how CLOUDFORMATION applies a changed declaration
/// and is a member of no Systems Manager call at all.
const DOCUMENT_MEMBERS: &[&str] = &[
    "Attachments",
    "Content",
    "DocumentFormat",
    "DocumentType",
    "Requires",
    "TargetType",
    "VersionName",
];

/// The same properties `UpdateDocument` takes, which is every one of the create's except the
/// two a document that EXISTS cannot be given a new value for: `DocumentType` is a member of
/// the create and of no update — no call of that service turns a runbook into a command
/// document — which is why `template::replacement_properties` names the property for this
/// type, and `Requires` is the same case.
const UPDATED_DOCUMENT_MEMBERS: &[&str] = &[
    "Attachments",
    "Content",
    "DocumentFormat",
    "TargetType",
    "VersionName",
];

/// The members `GetDocument` reports a document's own properties under, as
/// `(property, member)`: that read answers them FLAT, each under the very name the type
/// declares the property by. `Attachments` is not among them — the read reports the
/// attachments' CONTENT (`AttachmentsContent`), not the sources a create named — and neither
/// is `TargetType`, which that read does not report at all.
const REPORTED_DOCUMENT_MEMBERS: &[(&str, &str)] = &[
    ("Content", "Content"),
    ("DocumentFormat", "DocumentFormat"),
    ("DocumentType", "DocumentType"),
    ("Requires", "Requires"),
    ("VersionName", "VersionName"),
];

/// Create the document an `AWS::SSM::Document` declares, so a client that deployed the stack
/// and then calls Systems Manager with the name the stack published — `GetDocument`,
/// `DescribeDocument`, or `StartAutomationExecution` for a runbook — resolves the document
/// rather than being refused the name as one no document carries.
///
/// `Ref` of this type returns the document NAME, which is this resource's physical id, so the
/// document is created under exactly the name the stack reports for it. Nothing
/// `CreateDocument` answers is a published return value of the type, so no attribute is
/// re-reported off it.
///
/// The document carries the tags the declaration asks for plus the ones CloudFormation gives
/// the resource ([`tags_of`]), as the list Systems Manager's own `Tag` shape spells — so that
/// service's own tag read on the name the stack publishes answers with the stack's tags and
/// the `aws:cloudformation:*` ones.
pub(super) async fn create_ssm_document(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = document_request(
        &resource.properties,
        DOCUMENT_MEMBERS,
        &resource.physical_resource_id,
    );
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("Tags".to_string(), tags_as_list(&tags, UPPER_CASED_TAG));
    }
    let answered = call_ssm(ctx, region, CREATE_DOCUMENT, &Value::Object(request))
        .await
        .map_err(|refusal| handler_refusal(CREATE_DOCUMENT, refusal))?;
    // A create that answered no document left nothing for a client to resolve by name, so it
    // is a FAILED resource rather than a created one: the stack reporting CREATE_COMPLETE is
    // what a client takes as the guarantee that the declared document is usable.
    if reported_text(
        &reported_member(&answered, DOCUMENT_DESCRIPTION),
        "Name",
    )
    .unwrap_or_default()
    .is_empty()
    {
        return Err(Refusal {
            code: "InternalFailure".to_string(),
            message: "AWS Systems Manager answered a created document without a Name"
                .to_string(),
        });
    }
    Ok(Created::default())
}

/// Apply a CHANGED declaration to the document the stack created, through `UpdateDocument` —
/// "updates one or more values for an SSM document", which adds the new content to the
/// document that name belongs to as a VERSION of it — followed by the call that makes that
/// version the one every later execution runs (`UpdateDocumentDefaultVersion`, "set the
/// default version of a document").
///
/// A document is identified by its name, and the name is this resource's physical id, so a
/// changed declaration reaches that service as a new version of the same document rather than
/// replacing it. An update that states the content the document already carries, or the
/// version name it already holds, asks for a version that is already there
/// ([`DUPLICATE_DOCUMENT_CONTENT`], [`DUPLICATE_DOCUMENT_VERSION_NAME`]): the document is
/// already in the state the declaration asks for, so nothing more is applied.
pub(super) async fn update_ssm_document(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Created, Refusal> {
    let request = document_request(
        declared,
        UPDATED_DOCUMENT_MEMBERS,
        &resource.physical_resource_id,
    );
    let answered = match call_ssm(ctx, region, UPDATE_DOCUMENT, &Value::Object(request)).await {
        Ok(answered) => answered,
        Err(refusal)
            if refusal.code == DUPLICATE_DOCUMENT_CONTENT
                || refusal.code == DUPLICATE_DOCUMENT_VERSION_NAME =>
        {
            return Ok(Created::default())
        }
        Err(refusal) => return Err(handler_refusal(UPDATE_DOCUMENT, refusal)),
    };
    // "The document version for the current version of the document"
    // (`DocumentDescription$DocumentVersion`) is the version the update created, which is the
    // one a client running the document by name has to reach — so it is made the default.
    let created_version = reported_text(
        &reported_member(&answered, DOCUMENT_DESCRIPTION),
        DOCUMENT_VERSION,
    )
    .unwrap_or_default();
    if !created_version.is_empty() {
        let request = serde_json::json!({
            "Name": resource.physical_resource_id.clone(),
            DOCUMENT_VERSION: created_version,
        });
        call_ssm(ctx, region, UPDATE_DOCUMENT_DEFAULT_VERSION, &request)
            .await
            .map_err(|refusal| handler_refusal(UPDATE_DOCUMENT_DEFAULT_VERSION, refusal))?;
    }
    Ok(Created::default())
}

/// Delete the document a stack created, addressed by its name — this resource's physical id,
/// which is what `Ref` returns for the type. A document Systems Manager no longer holds
/// (`InvalidDocument`, judged on the CODE and never on a message that carries the request's
/// own values) is already in the state the delete asks for.
pub(super) async fn delete_ssm_document(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "Name": physical_resource_id });
    match call_ssm(ctx, region, DELETE_DOCUMENT, &request).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == INVALID_DOCUMENT => Ok(()),
        Err(refusal) => Err(handler_refusal(DELETE_DOCUMENT, refusal)),
    }
}

/// The document's configuration as Systems Manager reports it, for drift detection: what its
/// own read answers for the name that is this resource's physical id, under the property names
/// the type declares ([`REPORTED_DOCUMENT_MEMBERS`]). One it no longer holds
/// (`InvalidDocument`) was deleted, which is what `None` reports.
pub(super) async fn describe_ssm_document(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "Name": physical_resource_id });
    match call_ssm(ctx, region, GET_DOCUMENT, &request).await {
        Ok(answer) => Ok(Some(reported_as_properties(
            &answer,
            REPORTED_DOCUMENT_MEMBERS,
        ))),
        Err(refusal) if refusal.code == INVALID_DOCUMENT => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// Whether Systems Manager reports the created document USABLE.
///
/// "Creating — The document is being created" (`DocumentDescription$Status`) is the status a
/// created document is first reported in, and that service reports it `Active` once the
/// document is in place — so the resource's creation is finished when it is there, and a
/// client that reads the stack as complete finds a document every Systems Manager call on it
/// resolves. A document that service reports `Failed` ("The document couldn't be created")
/// fails the resource with its own `StatusInformation` rather than waiting the window out, and
/// one it holds nothing for is a document the create did not leave behind.
pub(super) async fn ssm_document_readiness(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Readiness, Refusal> {
    let request = serde_json::json!({ "Name": physical_resource_id });
    let described = match call_ssm(ctx, region, DESCRIBE_DOCUMENT, &request).await {
        Ok(answer) => reported_member(&answer, "Document"),
        Err(refusal) if refusal.code == INVALID_DOCUMENT => {
            return Ok(Readiness::Failed(format!(
                "AWS Systems Manager holds no document named {physical_resource_id}"
            )))
        }
        Err(refusal) => return Err(refusal),
    };
    let status = reported_text(&described, "Status").unwrap_or_default();
    match status.as_str() {
        DOCUMENT_STATUS_ACTIVE => Ok(Readiness::Ready),
        DOCUMENT_STATUS_FAILED => Ok(Readiness::Failed(
            reported_text(&described, "StatusInformation").unwrap_or_else(|| {
                format!("AWS Systems Manager reports document {physical_resource_id} Failed")
            }),
        )),
        // Every other status is one the document is on its way through, including a read that
        // reports none at all: the wait ends when that service says `Active`.
        _ => Ok(Readiness::Pending(status)),
    }
}

/// The request one declaration describes, under the name CloudFormation decided: the members
/// named identically on both sides (`members`), each exactly as the declaration resolved it,
/// with `Content` rendered as the TEXT its API member takes.
///
/// Every other value is passed through rather than re-rendered as text: `Attachments` and
/// `Requires` are lists of structures on the Systems Manager side as well as on the
/// template's, so a value re-spelled as a string would be a value that service refuses. A
/// property declared as `null` states no value at all, so it is left out rather than sent as
/// a null member.
fn document_request(properties: &Value, members: &[&str], name: &str) -> Map<String, Value> {
    let mut request = Map::new();
    if let Some(declared) = properties.as_object() {
        for member in members {
            let value = match declared.get(*member) {
                Some(Value::Null) | None => continue,
                Some(value) => value,
            };
            // "The content for the new SSM document in JSON or YAML format"
            // (`CreateDocument$Content`) is TEXT, and a template states it either as the
            // document itself or as the text of one.
            let sent = match *member {
                "Content" => document_text(value)
                    .map(Value::String)
                    .unwrap_or_else(|| value.clone()),
                _ => value.clone(),
            };
            request.insert((*member).to_string(), sent);
        }
    }
    request.insert("Name".to_string(), Value::String(name.to_string()));
    request
}
