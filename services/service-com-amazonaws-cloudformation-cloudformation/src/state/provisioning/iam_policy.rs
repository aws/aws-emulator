// SPDX-License-Identifier: CC0-1.0

//! The INLINE policies a stack declares as resources of their own — `AWS::IAM::Policy` and
//! the per-entity `AWS::IAM::RolePolicy`, `AWS::IAM::UserPolicy` and `AWS::IAM::GroupPolicy`
//! — embedded in the roles, users and groups those declarations name, in AWS IAM itself.
//!
//! None of these types is an entity of IAM's own. "Adds or updates an inline policy document
//! that is embedded in the specified IAM role" (`PutRolePolicy`) — and the same call per user
//! (`PutUserPolicy`) and per group (`PutGroupPolicy`) — is what they ARE: one document, under
//! the `PolicyName` the declaration states, embedded in every entity the declaration names.
//! So a stack that declares one has to MAKE those calls, or the grant a template writes is a
//! grant nothing in IAM carries: a role whose access a stack declares this way would be
//! refused the very calls the template granted it, by every service that asks IAM what that
//! role may do.
//!
//! The four types differ only in HOW the declaration names the entities: `AWS::IAM::Policy`
//! lists them under `Roles`, `Users` and `Groups` — it can embed one document in several
//! entities at once — while each per-entity type names exactly ONE, under its own
//! `RoleName`, `UserName` or `GroupName`. Which it is travels with the owner
//! ([`Kind`]), so one set of arms serves all four.
//!
//! The entities are named by NAME, as those calls take them ("the name of the role to
//! associate the policy with"), which is what a `Ref` to an `AWS::IAM::Role`,
//! `AWS::IAM::User` or `AWS::IAM::Group` of the same template resolves to.
//!
//! The resource's physical id is CloudFormation's own generated id for the type (no
//! declaration carries a name property for one), so nothing of the id is adopted from IAM;
//! what addresses the calls is the `PolicyName` and the entity, both of which the record
//! carries as declared.
//!
//! It lives beside [`super::lambda_function`] and is reached through the same seam
//! ([`super::Owner`]), for the reason that module records.

use serde_json::Value;

use super::{call_query, Refusal};
use crate::state::StoredResource;

/// The SigV4 signing name of the service that owns these types.
const IAM_SIGNING_NAME: &str = "iam";

/// The API version IAM's awsQuery requests name beside the action, from that service's own
/// model.
const IAM_API_VERSION: &str = "2010-05-08";

/// The error code IAM answers for an entity — or an embedded policy — it does not hold
/// (`NoSuchEntity`), which is what says a removal has nothing left to remove.
const NO_SUCH_ENTITY: &str = "NoSuchEntity";

/// The property carrying the name the document is embedded under.
const POLICY_NAME_PROPERTY: &str = "PolicyName";

/// The property carrying the document itself.
const POLICY_DOCUMENT_PROPERTY: &str = "PolicyDocument";

/// One KIND of entity an inline policy is embedded in: the calls that embed and remove the
/// document, and the member those calls name the entity in.
///
/// Named at the same visibility as [`Kind`], which carries one: a private type inside a
/// visible one is a type a caller of that one could not name.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::state) struct Entity {
    /// The member the IAM calls name one of them in, which is also the property the
    /// per-entity type names the single entity under.
    member: &'static str,
    /// The property of an `AWS::IAM::Policy` declaration LISTING the entities of this kind.
    list_property: &'static str,
    /// `Put<entity>Policy`, which "adds or updates an inline policy document that is
    /// embedded in" one entity.
    put: &'static str,
    /// `Delete<entity>Policy`, which deletes it again.
    delete: &'static str,
}

/// The three kinds of entity an inline policy is embedded in, each with the pair of IAM calls
/// that embed and remove one.
const ROLE: Entity = Entity {
    member: "RoleName",
    list_property: "Roles",
    put: "PutRolePolicy",
    delete: "DeleteRolePolicy",
};
const USER: Entity = Entity {
    member: "UserName",
    list_property: "Users",
    put: "PutUserPolicy",
    delete: "DeleteUserPolicy",
};
const GROUP: Entity = Entity {
    member: "GroupName",
    list_property: "Groups",
    put: "PutGroupPolicy",
    delete: "DeleteGroupPolicy",
};

/// Every kind, in the order an `AWS::IAM::Policy`'s entities are walked.
const ENTITIES: &[&Entity] = &[&ROLE, &USER, &GROUP];

/// Which of the four types a resource is, which is all the arms below need to know about it.
///
/// Visible where the owner that carries it is ([`super::Owner`]), as the Amazon EC2
/// network types' own kind is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::state) enum Kind {
    /// `AWS::IAM::Policy`: one document, embedded in every entity its `Roles`, `Users` and
    /// `Groups` list.
    Shared,
    /// A per-entity type (`AWS::IAM::RolePolicy`, `AWS::IAM::UserPolicy`,
    /// `AWS::IAM::GroupPolicy`): one document, embedded in the ONE entity its own
    /// `RoleName`/`UserName`/`GroupName` names.
    One(&'static Entity),
}

/// Which of these types `resource_type` is, or `None` for a type this module does not own.
pub(super) fn kind_of(resource_type: &str) -> Option<Kind> {
    match resource_type {
        "AWS::IAM::Policy" => Some(Kind::Shared),
        "AWS::IAM::RolePolicy" => Some(Kind::One(&ROLE)),
        "AWS::IAM::UserPolicy" => Some(Kind::One(&USER)),
        "AWS::IAM::GroupPolicy" => Some(Kind::One(&GROUP)),
        _ => None,
    }
}

impl Kind {
    /// Every property the type publishes. A declared property this list does not carry is
    /// REFUSED rather than dropped, so a grant is never reported as made with less than its
    /// declaration asked for.
    ///
    /// Source: each type's own property set, which is the `Put<entity>Policy` calls' members
    /// (`PolicyName`, `PolicyDocument`) beside the properties naming the entities the
    /// document is embedded in.
    fn properties(self) -> &'static [&'static str] {
        match self {
            Self::Shared => &[
                "Groups",
                POLICY_DOCUMENT_PROPERTY,
                POLICY_NAME_PROPERTY,
                "Roles",
                "Users",
            ],
            Self::One(&ROLE) => &[POLICY_DOCUMENT_PROPERTY, POLICY_NAME_PROPERTY, "RoleName"],
            Self::One(&USER) => &[POLICY_DOCUMENT_PROPERTY, POLICY_NAME_PROPERTY, "UserName"],
            Self::One(_) => &[POLICY_DOCUMENT_PROPERTY, POLICY_NAME_PROPERTY, "GroupName"],
        }
    }

    /// The entities one declaration names, as `(the kind, the entity's name)` pairs: every
    /// entry of the three lists for [`Self::Shared`], and the one name the per-entity type's
    /// own property carries.
    fn entities(self, properties: &Value) -> Vec<(&'static Entity, String)> {
        match self {
            Self::Shared => ENTITIES
                .iter()
                .flat_map(|entity| {
                    named(properties, entity.list_property)
                        .into_iter()
                        .map(move |name| (*entity, name))
                })
                .collect(),
            Self::One(entity) => named(properties, entity.member)
                .into_iter()
                .map(|name| (entity, name))
                .collect(),
        }
    }
}

/// Embed the document the declaration states in every entity it names — so a role a stack
/// granted access this way really holds that grant, and the service that asks IAM what the
/// role may do reads the statements the template wrote.
///
/// `Err` is IAM's own refusal: an entity it does not hold, a document it will not accept.
pub(super) async fn create(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    refuse_unmapped_properties(kind, resource)?;
    // A declaration that names no role, user or group embeds its document NOWHERE, so
    // nothing is asked of IAM for it: the resource is the stack's record of a policy attached
    // to nothing, and there is no call whose arguments could be missing.
    let entities = kind.entities(&resource.properties);
    if entities.is_empty() {
        return Ok(());
    }
    let name = required_text(resource, POLICY_NAME_PROPERTY)?;
    let document = required_document(resource)?;
    for (entity, named) in entities {
        call_query(
            ctx,
            IAM_SIGNING_NAME,
            region,
            IAM_API_VERSION,
            entity.put,
            &[
                (entity.member.to_string(), named),
                (POLICY_NAME_PROPERTY.to_string(), name.clone()),
                (POLICY_DOCUMENT_PROPERTY.to_string(), document.clone()),
            ],
        )
        .await?;
    }
    Ok(())
}

/// Apply a CHANGED declaration: the document is embedded in every entity the deployment now
/// names — `Put<entity>Policy` "adds or UPDATES" it, so re-putting is what applies an edited
/// document — and removed from every entity, or under every name, the declaration no longer
/// states, which is what the change asks for.
pub(super) async fn update(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
    declared: &Value,
) -> Result<(), Refusal> {
    let asked = StoredResource {
        properties: declared.clone(),
        ..resource.clone()
    };
    refuse_unmapped_properties(kind, &asked)?;
    create(ctx, region, kind, &asked).await?;
    let held = kind.entities(&resource.properties);
    if held.is_empty() {
        // Nothing was embedded anywhere, so the change has nothing to clean off an entity.
        return Ok(());
    }
    // The policy NAME the stack has: an entity dropped from the declaration is cleaned up
    // under the name the document was embedded under, which is the one the record carries.
    let held_name = required_text(resource, POLICY_NAME_PROPERTY)?;
    let name = declared
        .get(POLICY_NAME_PROPERTY)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let now = kind.entities(declared);
    for (entity, named) in held {
        // An entity the deployment still names keeps the document just re-put in it — under
        // its new name, so the old name is cleaned off it too where the name changed.
        if now.contains(&(entity, named.clone())) && name == held_name {
            continue;
        }
        remove(ctx, region, entity, &named, &held_name).await?;
    }
    Ok(())
}

/// Delete the document a stack embedded, from every entity the declaration names.
pub(super) async fn delete(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let entities = kind.entities(&resource.properties);
    if entities.is_empty() {
        return Ok(());
    }
    let name = required_text(resource, POLICY_NAME_PROPERTY)?;
    for (entity, named) in entities {
        remove(ctx, region, entity, &named, &name).await?;
    }
    Ok(())
}

/// `Delete<entity>Policy` for one entity, with an entity — or an embedded policy — IAM no
/// longer holds read as the deletion this asked for, judged on the CODE and never on a
/// message.
async fn remove(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    entity: &Entity,
    named: &str,
    policy_name: &str,
) -> Result<(), Refusal> {
    match call_query(
        ctx,
        IAM_SIGNING_NAME,
        region,
        IAM_API_VERSION,
        entity.delete,
        &[
            (entity.member.to_string(), named.to_string()),
            (POLICY_NAME_PROPERTY.to_string(), policy_name.to_string()),
        ],
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == NO_SUCH_ENTITY => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The entity names one property of the declaration carries, each as the NAME the IAM calls
/// take: every entry of a LIST, or the one name a single value states. A property the
/// declaration leaves out names none, and an entry that is not text is not a name.
fn named(properties: &Value, property: &str) -> Vec<String> {
    let text = |value: &Value| {
        value
            .as_str()
            .filter(|name| !name.is_empty())
            .map(ToString::to_string)
    };
    match properties.get(property) {
        Some(Value::Array(entries)) => entries.iter().filter_map(text).collect(),
        Some(entry) => text(entry).into_iter().collect(),
        None => Vec::new(),
    }
}

/// One required text property, or the refusal naming the property the declaration left out.
fn required_text(resource: &StoredResource, property: &str) -> Result<String, Refusal> {
    match resource.properties.get(property).and_then(Value::as_str) {
        Some(text) if !text.is_empty() => Ok(text.to_string()),
        _ => Err(Refusal {
            code: "InvalidRequest".to_string(),
            message: format!(
                "{} requires the {property} property",
                resource.resource_type
            ),
        }),
    }
}

/// The declared document, as the text the IAM calls carry it as: a template states it as a
/// JSON object (the common form) or as a string, and both reach IAM as the document itself.
///
/// The same reading [`crate::state::provisioner`] gives the policy-document property of the
/// types it provisions, spelled here because that module is far past the length a change to
/// this repository may touch, so nothing in it can be made reachable from this one.
fn required_document(resource: &StoredResource) -> Result<String, Refusal> {
    let rendered = match resource.properties.get(POLICY_DOCUMENT_PROPERTY) {
        Some(Value::String(document)) => Some(document.clone()),
        Some(document @ (Value::Object(_) | Value::Array(_))) => {
            serde_json::to_string(document).ok()
        }
        _ => None,
    };
    match rendered.filter(|document| !document.is_empty()) {
        Some(document) => Ok(document),
        None => Err(Refusal {
            code: "InvalidRequest".to_string(),
            message: format!(
                "{} requires the {POLICY_DOCUMENT_PROPERTY} property",
                resource.resource_type
            ),
        }),
    }
}

/// The refusal for a declared property this module does not apply, naming which property it
/// was.
fn refuse_unmapped_properties(kind: Kind, resource: &StoredResource) -> Result<(), Refusal> {
    let Value::Object(properties) = &resource.properties else {
        return Ok(());
    };
    for property in properties.keys() {
        if kind.properties().contains(&property.as_str()) {
            continue;
        }
        return Err(Refusal {
            code: "NotImplemented".to_string(),
            message: format!(
                "this emulator does not apply the {property} property of {}",
                resource.resource_type
            ),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A declaration of one resource, as the record the arms read.
    fn declared(resource_type: &str, properties: Value) -> StoredResource {
        StoredResource {
            logical_id: "QueueAccess".to_string(),
            resource_type: resource_type.to_string(),
            physical_resource_id: "stack-QueueAccess-ABCDEFGHIJ".to_string(),
            attributes: ::std::collections::BTreeMap::new(),
            metadata: None,
            properties,
            materialised: false,
            deletion_policy: None,
            update_replace_policy: None,
        }
    }

    /// Every property each type publishes has a place: it addresses the calls, carries the
    /// document, or names the entities the document is embedded in.
    #[test]
    fn every_published_property_has_a_place() {
        for resource_type in [
            "AWS::IAM::Policy",
            "AWS::IAM::RolePolicy",
            "AWS::IAM::UserPolicy",
            "AWS::IAM::GroupPolicy",
        ] {
            let kind = kind_of(resource_type)
                .unwrap_or_else(|| panic!("case {resource_type:?}: the type is not owned here"));
            for property in kind.properties() {
                let placed = [POLICY_NAME_PROPERTY, POLICY_DOCUMENT_PROPERTY]
                    .contains(property)
                    || ENTITIES.iter().any(|entity| {
                        entity.member == *property || entity.list_property == *property
                    });
                assert!(
                    placed,
                    "case {resource_type:?}: expected {property:?} to address the calls, carry \
                     the document or name the entities, got none"
                );
            }
        }
        assert!(
            kind_of("AWS::IAM::Role").is_none(),
            "case \"a role\": expected the role itself not to be owned here, got an owner"
        );
    }

    /// One row of the table below: what to call it, the type it declares, the properties the
    /// declaration carries, and the `(member, name)` pairs those name.
    type EntityRow = (
        &'static str,
        &'static str,
        Value,
        &'static [(&'static str, &'static str)],
    );

    /// The entities a declaration names, over the four types and the forms each states them
    /// in: the shared type's three lists, and the one name a per-entity type carries.
    #[test]
    fn the_entities_a_declaration_names_are_the_ones_it_states() {
        let rows: &[EntityRow] = &[
            (
                "one role of a shared policy",
                "AWS::IAM::Policy",
                serde_json::json!({ "Roles": ["pump-exec"] }),
                &[("RoleName", "pump-exec")],
            ),
            (
                "two roles, a user and a group",
                "AWS::IAM::Policy",
                serde_json::json!({
                    "Roles": ["a", "b"],
                    "Users": ["u"],
                    "Groups": ["g"],
                }),
                &[
                    ("RoleName", "a"),
                    ("RoleName", "b"),
                    ("UserName", "u"),
                    ("GroupName", "g"),
                ],
            ),
            (
                "no entity at all",
                "AWS::IAM::Policy",
                serde_json::json!({}),
                &[],
            ),
            (
                "an entry that is not a name",
                "AWS::IAM::Policy",
                serde_json::json!({ "Roles": ["kept", "", 7] }),
                &[("RoleName", "kept")],
            ),
            (
                "the one role of a role policy",
                "AWS::IAM::RolePolicy",
                serde_json::json!({ "RoleName": "pump-exec" }),
                &[("RoleName", "pump-exec")],
            ),
            (
                "the one user of a user policy",
                "AWS::IAM::UserPolicy",
                serde_json::json!({ "UserName": "pump-user" }),
                &[("UserName", "pump-user")],
            ),
            (
                "the one group of a group policy",
                "AWS::IAM::GroupPolicy",
                serde_json::json!({ "GroupName": "pumps" }),
                &[("GroupName", "pumps")],
            ),
            (
                "a role policy naming no role",
                "AWS::IAM::RolePolicy",
                serde_json::json!({ "PolicyName": "queue-reads" }),
                &[],
            ),
        ];
        for (case, resource_type, properties, expected) in rows {
            let kind = kind_of(resource_type)
                .unwrap_or_else(|| panic!("case {case:?}: the type is not owned here"));
            let named: Vec<(String, String)> = kind
                .entities(properties)
                .into_iter()
                .map(|(entity, name)| (entity.member.to_string(), name))
                .collect();
            let expected: Vec<(String, String)> = expected
                .iter()
                .map(|(member, name)| ((*member).to_string(), (*name).to_string()))
                .collect();
            assert_eq!(
                named, expected,
                "case {case:?}: expected {expected:?}, got {named:?}"
            );
        }
    }

    /// The document reaches IAM as the document itself, whichever of the two forms a
    /// template states it in, and a declaration carrying none is refused rather than
    /// embedding an empty grant.
    ///
    /// What travels is the DOCUMENT, so each case is compared as the document it parses to
    /// rather than as one spelling of it: a serializer is free to order a map's members.
    #[test]
    fn the_document_travels_in_the_form_iam_takes_it() {
        let document = serde_json::json!({ "Version": "2012-10-17", "Statement": [] });
        let object = declared(
            "AWS::IAM::Policy",
            serde_json::json!({
                "PolicyName": "queue-reads",
                "PolicyDocument": document.clone(),
            }),
        );
        let carried = required_document(&object);
        let parsed: Option<Value> = carried
            .as_deref()
            .ok()
            .and_then(|text| serde_json::from_str(text).ok());
        assert_eq!(
            parsed,
            Some(document.clone()),
            "case \"a document stated as an object\": got {carried:?}"
        );

        let text = declared(
            "AWS::IAM::Policy",
            serde_json::json!({
                "PolicyName": "queue-reads",
                "PolicyDocument": r#"{"Version":"2012-10-17","Statement":[]}"#,
            }),
        );
        let carried = required_document(&text);
        assert_eq!(
            carried.as_deref().ok(),
            Some(r#"{"Version":"2012-10-17","Statement":[]}"#),
            "case \"a document stated as text\": got {carried:?}"
        );

        for (case, properties) in [
            ("no properties at all", Value::Null),
            (
                "no document",
                serde_json::json!({ "PolicyName": "queue-reads" }),
            ),
            (
                "an empty document",
                serde_json::json!({ "PolicyName": "queue-reads", "PolicyDocument": "" }),
            ),
        ] {
            let refused = required_document(&declared("AWS::IAM::Policy", properties));
            assert!(
                refused.is_err(),
                "case {case:?}: expected a refusal naming the required property, got {refused:?}"
            );
        }
    }
}
