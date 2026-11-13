// SPDX-License-Identifier: CC0-1.0

//! `AWS::IAM::ManagedPolicy`, as it lives in the IAM service's own store.
//!
//! A customer managed policy is a resource of the IAM service: `iam:CreatePolicy` creates
//! it, `iam:GetPolicy` and `iam:ListPolicies` report it, `iam:CreatePolicyVersion` changes
//! its document and `iam:DeletePolicy` removes it, and the record of it lives in that
//! service's state in the same shared engine. So this provider holds a handle to THAT store
//! and nothing of its own: a policy this emulator holds is one Cloud Control API resolves
//! whichever API created it, one IAM no longer holds is one Cloud Control API reports as not
//! found, and one a `CreateResource` provisioned is one `iam:GetPolicy` and
//! `iam:AttachRolePolicy` accept — which is what makes the identifier Cloud Control hands
//! back a usable handle rather than a name.
//!
//! IAM is a GLOBAL service: a policy's ARN carries no Region ([`iam::policy_arn`]) and the
//! account holds its policies in one namespace, so a reference resolves to the same policy
//! whichever Region the call naming it was signed for.
//!
//! The resource model is the type's registry schema. Catalog source: the
//! `AWS::IAM::ManagedPolicy` resource provider schema's `properties`, whose
//! `primaryIdentifier` is `/properties/PolicyArn` — the ARN IAM assigns, which is therefore
//! also one of its `readOnlyProperties`. The properties this path APPLIES are the identity
//! and the policy's content:
//!
//! * `ManagedPolicyName`, the policy's friendly name, which the schema declares create-only
//!   and which the ARN is built from; a model naming none is a resource whose name the
//!   provider picks, the same way a template that declares none leaves the name to
//!   CloudFormation;
//! * `Path`, create-only, which the ARN also carries, defaulting to the account root;
//! * `Description`, create-only — IAM publishes no call that changes a policy's description;
//! * `PolicyDocument`, the IAM JSON policy the policy's default version holds, read against
//!   the shared reading of an identity policy every service taking one asks, so a document
//!   IAM refuses is refused here too.
//!
//! The read-only properties the owning record carries are reported beside them — `PolicyArn`,
//! `PolicyId`, `DefaultVersionId`, `AttachmentCount`, `PermissionsBoundaryUsageCount` and
//! `IsAttachable` — each read off that record, so what a Cloud Control API read reports and
//! what `iam:GetPolicy` reports are one fact.
//!
//! The schema's remaining properties are ones THIS path cannot apply
//! ([`UNAPPLIED_PROPERTIES`]), so a model that asks for one is refused rather than accepted
//! and dropped — a property a caller sets has to be one a later read reports:
//!
//! * `Groups`, `Roles` and `Users`, because an attachment is made by a call of its own
//!   (`iam:AttachGroupPolicy`, `iam:AttachRolePolicy`, `iam:AttachUserPolicy`) against an
//!   entity this handler does not resolve;
//! * `CreateDate` and `UpdateDate`, the instants the owning record stamps, which this path
//!   neither reports nor applies.

use ::std::time::SystemTime;

use ::serde_json::{Map, Value};
use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::{
    Policy, PolicyVersion,
};
use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;

use super::{
    epoch_millis, generated_name, require_held, require_value, text, unsupported, IamState,
    NativeRefusal, NativeResourceProvider,
};

/// The `TypeName` this provider stands behind.
pub const TYPE_NAME: &str = "AWS::IAM::ManagedPolicy";

/// The property that carries the type's primary identifier, which the schema's
/// `primaryIdentifier` points at (`/properties/PolicyArn`): the policy's ARN, which IAM
/// assigns rather than the caller naming it.
const IDENTIFIER_PROPERTY: &str = "PolicyArn";

/// The policy's friendly name (`ManagedPolicyName`), which the schema declares create-only
/// and which the ARN is built from.
const MANAGED_POLICY_NAME: &str = "ManagedPolicyName";

/// The path the policy is filed under (`Path`), which the schema declares create-only and
/// which the ARN also carries.
const PATH: &str = "Path";

/// The policy's description (`Description`), which the schema declares create-only.
const DESCRIPTION: &str = "Description";

/// The IAM JSON policy the policy's default version holds (`PolicyDocument`).
const POLICY_DOCUMENT: &str = "PolicyDocument";

/// The policy's stable id (`PolicyId`), which the schema declares read-only.
const POLICY_ID: &str = "PolicyId";

/// The version in force on the policy (`DefaultVersionId`), which the schema declares
/// read-only.
const DEFAULT_VERSION_ID: &str = "DefaultVersionId";

/// How many entities the policy is attached to (`AttachmentCount`), which the schema
/// declares read-only.
const ATTACHMENT_COUNT: &str = "AttachmentCount";

/// For how many entities the policy sets the permissions boundary
/// (`PermissionsBoundaryUsageCount`), which the schema declares read-only.
const PERMISSIONS_BOUNDARY_USAGE_COUNT: &str = "PermissionsBoundaryUsageCount";

/// Whether the policy can be attached at all (`IsAttachable`), which the schema declares
/// read-only.
const IS_ATTACHABLE: &str = "IsAttachable";

/// The properties of the type's schema THIS path does not apply — see this module's header.
///
/// Named here so a model asking for one is told it is a property of the type this handler
/// does not apply, rather than one the type does not declare at all: the two are different
/// faults and a caller fixes them differently.
const UNAPPLIED_PROPERTIES: &[&str] = &["CreateDate", "Groups", "Roles", "UpdateDate", "Users"];

/// The customer managed policies of one engine, as Cloud Control API resources.
pub struct ManagedPolicyProvider {
    /// The owning service's store. A handle, so every answer is read from the live store.
    state: ::engine::ServiceState<IamState>,
}

impl ManagedPolicyProvider {
    /// The provider for the IAM store THIS engine carries, or `None` when it carries none.
    pub fn for_engine<Eng: Send + Sync + 'static>(engine: &Eng) -> Option<Self> {
        iam::service_state(engine).map(|state| Self { state })
    }
}

impl NativeResourceProvider for ManagedPolicyProvider {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn identifier_property(&self) -> &'static str {
        IDENTIFIER_PROPERTY
    }

    /// The ARN the policy a create describes will answer to.
    ///
    /// The type's primary identifier is read-only in its schema, so no desired state carries
    /// it: the ARN is the owning service's to assign, and it assigns the one built from the
    /// partition, this account, the policy's path and its name — IAM carries no Region in it.
    /// A model that names no name is a resource whose name the provider picks
    /// ([`generated_name`]), the same way a template that declares none leaves the name to
    /// CloudFormation.
    fn assigned_identifier(&self, region: &str, properties: &Value) -> Option<String> {
        // Global: a policy's ARN carries no Region (see this module's header).
        let _ = region;
        let name = properties
            .get(MANAGED_POLICY_NAME)
            .and_then(Value::as_str)
            .map(str::to_string)
            // A policy name carries no suffix any kind of it requires, so the generated one
            // is the bare minted token.
            .unwrap_or_else(|| generated_name(false, ""));
        let path = properties
            .get(PATH)
            .and_then(Value::as_str)
            .unwrap_or(iam::DEFAULT_PATH);
        Some(iam::policy_arn(path, &name))
    }

    fn read(&self, region: &str, identifier: &str) -> Option<Value> {
        let _ = region;
        let store = self.state.lock();
        let held = store.policy_by_arn(identifier)?;
        Some(model(
            held,
            store.policy_attachment_count(identifier),
            store.permissions_boundary_usage_count(identifier),
        ))
    }

    fn list(&self, region: &str, after: Option<&str>, limit: usize) -> Vec<(String, Value)> {
        let _ = region;
        let store = self.state.lock();
        // The owning store keys the account's policies by the case-folded NAME, so the ARN
        // order a listing pages over is built here — over the records that sort after the
        // position the page resumes at, which is bounded by that store's own policy quota.
        let mut found: Vec<(String, &iam::ManagedPolicy)> = store
            .policies
            .values()
            .filter_map(|held| Some((held.policy.arn.clone()?, held)))
            .filter(|(arn, _)| after.is_none_or(|after| arn.as_str() > after))
            .collect();
        found.sort_by(|(left, _), (right, _)| left.cmp(right));
        found
            .into_iter()
            .take(limit)
            .map(|(arn, held)| {
                let model = model(
                    held,
                    store.policy_attachment_count(&arn),
                    store.permissions_boundary_usage_count(&arn),
                );
                (arn, model)
            })
            .collect()
    }

    fn create(
        &self,
        region: &str,
        identifier: &str,
        properties: &Value,
        at: SystemTime,
    ) -> Result<Value, NativeRefusal> {
        let _ = region;
        // The account's own policies are the resources of this type: an ARN in AWS's managed
        // policy namespace names a policy AWS owns, which no account creates.
        if iam::is_aws_managed_policy_arn(identifier) {
            return Err(NativeRefusal::Invalid(format!(
                "{identifier} is an AWS managed policy, which cannot be created."
            )));
        }
        let identity = PolicyIdentity::of(identifier)?;
        let desired = DesiredModel::read(properties, &Held::fresh(&identity))?;
        let document = desired.document()?;
        let stamped = ::aws_smithy_types::DateTime::from_millis(epoch_millis(at));

        let mut store = self.state.lock();
        // IAM entity names are unique within the account and "are not distinguished by
        // case", so a second create under any casing of a held name is that service's own
        // already-exists verdict rather than a second policy.
        if store.policies.contains_key(&iam::name_key(&identity.name)) {
            return Err(NativeRefusal::AlreadyExists);
        }
        // An account holds a bounded number of customer managed policies, past which IAM
        // refuses the create.
        if store.policies.len() >= iam::MAX_POLICIES_PER_ACCOUNT {
            return Err(NativeRefusal::AtCapacity(format!(
                "Cannot exceed quota for PoliciesPerAccount: {}",
                iam::MAX_POLICIES_PER_ACCOUNT
            )));
        }
        let policy = Policy {
            arn: Some(identity.arn.clone()),
            policy_id: Some(store.next_policy_id()),
            policy_name: Some(identity.name.clone()),
            path: Some(identity.path.clone()),
            // "This operation creates a policy version with a version identifier of v1 and
            // sets v1 as the policy's default version."
            default_version_id: Some(iam::FIRST_POLICY_VERSION_ID.to_string()),
            // A policy is created attached to nothing and used as nobody's permissions
            // boundary; the counts are derived from the attachments themselves.
            attachment_count: Some(0),
            permissions_boundary_usage_count: Some(0),
            // A customer managed policy can be attached to a user, group or role.
            is_attachable: true,
            description: desired.description.clone(),
            create_date: Some(stamped),
            update_date: Some(stamped),
            // This type's schema declares no tag set (see this module's header).
            tags: None,
        };
        let version = PolicyVersion {
            document: Some(document),
            version_id: Some(iam::FIRST_POLICY_VERSION_ID.to_string()),
            is_default_version: true,
            create_date: Some(stamped),
        };
        let held = iam::ManagedPolicy {
            policy,
            versions: [(iam::FIRST_POLICY_VERSION_ID.to_string(), version)]
                .into_iter()
                .collect(),
        };
        let created = model(&held, 0, 0);
        store.policies.insert(iam::name_key(&identity.name), held);
        Ok(created)
    }

    fn update(
        &self,
        region: &str,
        identifier: &str,
        properties: &Value,
        at: SystemTime,
    ) -> Result<Value, NativeRefusal> {
        let _ = region;
        let mut store = self.state.lock();
        let attachments = store.policy_attachment_count(identifier);
        let boundary_uses = store.permissions_boundary_usage_count(identifier);
        let held = store
            .policy_by_arn(identifier)
            .ok_or(NativeRefusal::NotFound)?;
        let desired = DesiredModel::read(
            properties,
            &Held::of(held, attachments, boundary_uses),
        )?;
        let document = desired.document()?;
        let in_force = held
            .versions
            .get(held.default_version_id())
            .and_then(|version| version.document.clone())
            .unwrap_or_default();
        // A document the policy already holds asks for nothing: IAM changes a managed
        // policy's content by adding a VERSION, so a model that states the version in force
        // back unchanged leaves the policy exactly where it is rather than minting a second
        // copy of the same document.
        if document == in_force {
            return Ok(model(held, attachments, boundary_uses));
        }
        let stamped = ::aws_smithy_types::DateTime::from_millis(epoch_millis(at));
        let held = store
            .policy_by_arn_mut(identifier)
            .ok_or(NativeRefusal::NotFound)?;
        // "A managed policy can have up to five versions. If the policy has five versions,
        // you must delete an existing version using DeletePolicyVersion before you create a
        // new version": past the quota the owning service refuses the write.
        if held.versions.len() >= iam::MAX_POLICY_VERSIONS {
            return Err(NativeRefusal::AtCapacity(format!(
                "A managed policy can have up to {} versions. Before you create a new \
                 version, you must delete an existing version.",
                iam::MAX_POLICY_VERSIONS
            )));
        }
        // The new version becomes the operative one — "the version that is used for all
        // principals" — and exactly one version of a policy is, so the version that was
        // default stops being it.
        let version_id = held.next_version_id();
        for version in held.versions.values_mut() {
            version.is_default_version = false;
        }
        held.versions.insert(
            version_id.clone(),
            PolicyVersion {
                document: Some(document),
                version_id: Some(version_id.clone()),
                is_default_version: true,
                create_date: Some(stamped),
            },
        );
        held.policy.default_version_id = Some(version_id);
        // `Policy$UpdateDate`: "when a policy has more than one version, this field contains
        // the date and time when the most recent policy version was created".
        held.policy.update_date = Some(stamped);
        Ok(model(held, attachments, boundary_uses))
    }

    fn delete(&self, region: &str, identifier: &str, at: SystemTime) -> Result<(), NativeRefusal> {
        // A policy's record carries no instant a removal moves, and IAM is global.
        let _ = (region, at);
        let mut store = self.state.lock();
        let attachments = store.policy_attachment_count(identifier);
        let boundary_uses = store.permissions_boundary_usage_count(identifier);
        let held = store
            .policy_by_arn(identifier)
            .ok_or(NativeRefusal::NotFound)?;
        let name = held
            .policy
            .policy_name
            .clone()
            .ok_or(NativeRefusal::NotFound)?;
        // "Before you can delete a managed policy, you must first detach the policy from all
        // users, groups, and roles that it is attached to" — an entity still relating to the
        // policy is one the owning service refuses to leave pointing at a policy the account
        // no longer holds, whether it holds the policy as a permissions policy or uses it to
        // set its permissions boundary.
        if attachments > 0 || boundary_uses > 0 {
            return Err(NativeRefusal::Rejected(
                "Cannot delete a policy attached to entities.".to_string(),
            ));
        }
        // The policy and the versions it carries are one stored record, so removing it
        // releases all of them: the name is free for a later create, and every read
        // addressed by that ARN reports no entity afterwards.
        store.policies.remove(&iam::name_key(&name));
        Ok(())
    }
}

/// The identity a managed policy's ARN names: the parts the owning service builds that ARN
/// from.
struct PolicyIdentity {
    /// The ARN itself.
    arn: String,
    /// The policy's path, beginning and ending with `/`.
    path: String,
    /// The policy's friendly name.
    name: String,
}

impl PolicyIdentity {
    /// The identity `arn` names, or why it is not the ARN of a policy of this account.
    ///
    /// Read through the OWNING service's own reader of a managed-policy ARN
    /// ([`iam::parse_policy_arn`]) and then rebuilt with that service's own ARN builder, so
    /// an ARN this provider accepts is one that service resolves to the same policy and no
    /// other: a path or an account the builder would not produce names no resource of this
    /// type.
    fn of(arn: &str) -> Result<Self, NativeRefusal> {
        let parsed = iam::parse_policy_arn(arn).ok_or_else(arn_requirement)?;
        let identity = Self {
            arn: arn.to_string(),
            path: parsed.path,
            name: parsed.policy_name,
        };
        if iam::policy_arn(&identity.path, &identity.name) != identity.arn {
            return Err(arn_requirement());
        }
        // The name and the path are the OWNING service's to accept: a value outside what
        // `policyNameType` (length 1..128, pattern `^[\w+=,.@-]+$`) or an IAM entity path
        // admits is one that service's own API would refuse.
        if !name_is_well_formed(&identity.name) {
            return Err(NativeRefusal::Invalid(format!(
                "{MANAGED_POLICY_NAME} must be at most {MAX_NAME_CHARACTERS} of the \
                 characters a policy name admits"
            )));
        }
        if !::service_common::iam_entity_path::is_published_form(&identity.path) {
            return Err(NativeRefusal::Invalid(format!(
                "{PATH} must begin and end with a forward slash"
            )));
        }
        Ok(identity)
    }
}

/// The most characters a policy's friendly name may carry (`policyNameType`, length 1..128).
const MAX_NAME_CHARACTERS: usize = 128;

/// Whether `name` is in the published form of IAM's `policyNameType`: 1..128 characters of
/// `^[\w+=,.@-]+$`, read as the ASCII word characters that pattern's class names.
fn name_is_well_formed(name: &str) -> bool {
    (1..=MAX_NAME_CHARACTERS).contains(&name.chars().count())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_+=,.@-".contains(c))
}

/// What the identifier of a resource of this type has to be, for a reference that is not one.
fn arn_requirement() -> NativeRefusal {
    NativeRefusal::Invalid(format!(
        "{IDENTIFIER_PROPERTY} must be the ARN of a customer managed policy of this account"
    ))
}

/// What the policy a model describes already answers to: the values its identity and its
/// read-only properties may be stated back with, and nothing else.
struct Held {
    /// Whether the policy EXISTS: `false` for the one a create is about to mint, which is
    /// what tells a model naming a create-only property that the create may choose it from
    /// an update that may only state back what the policy was created with.
    exists: bool,
    /// The policy's own ARN.
    arn: String,
    /// Its friendly name.
    name: String,
    /// Its path.
    path: String,
    /// Its description, or `None` for a policy that carries none.
    description: Option<String>,
    /// Its stable id, or `None` for a policy that does not exist yet.
    policy_id: Option<String>,
    /// The version in force, or `None` for a policy that does not exist yet.
    default_version_id: Option<String>,
    /// How many entities it is attached to.
    attachment_count: i32,
    /// For how many it sets the permissions boundary.
    permissions_boundary_usage_count: i32,
    /// Whether it can be attached.
    is_attachable: bool,
}

impl Held {
    /// What the policy a CREATE is about to mint will answer to: its identity, and the
    /// read-only values every freshly created policy carries — so a model that states them
    /// back is round-tripping the record this create produces rather than asking for one
    /// nothing would hold.
    fn fresh(identity: &PolicyIdentity) -> Self {
        Self {
            // Nothing holds this policy yet, so this write is the create that chooses what
            // its create-only properties are.
            exists: false,
            arn: identity.arn.clone(),
            name: identity.name.clone(),
            path: identity.path.clone(),
            // A create may name any description, so there is no held one to hold it to.
            description: None,
            // Minted by the owning store when the policy is created, so no model can state
            // either back.
            policy_id: None,
            default_version_id: None,
            attachment_count: 0,
            permissions_boundary_usage_count: 0,
            is_attachable: true,
        }
    }

    /// What the policy `held` answers to now.
    fn of(held: &iam::ManagedPolicy, attachment_count: i32, boundary_uses: i32) -> Self {
        Self {
            exists: true,
            arn: held.policy.arn.clone().unwrap_or_default(),
            name: held.policy.policy_name.clone().unwrap_or_default(),
            path: held.policy.path.clone().unwrap_or_default(),
            description: held.policy.description.clone(),
            policy_id: held.policy.policy_id.clone(),
            default_version_id: Some(held.default_version_id().to_string()),
            attachment_count,
            permissions_boundary_usage_count: boundary_uses,
            is_attachable: held.policy.is_attachable,
        }
    }
}

/// What a resource model asks a managed policy to be: the properties this path APPLIES,
/// having been checked against the owning service's own rules.
struct DesiredModel {
    /// The IAM JSON policy the policy's default version is to hold, as the text the owning
    /// record carries.
    document: Option<String>,
    /// The description the policy is to carry.
    description: Option<String>,
}

impl DesiredModel {
    /// Read `properties` as the model of the policy `held` describes — or state why the
    /// owning service would not take it.
    ///
    /// The identity and the read-only properties may be CARRIED (a read reports them, so a
    /// model a client round-trips carries them) but only with the value the policy already
    /// answers to: a value accepted here and never applied would be one no later read
    /// reports. `ManagedPolicyName`, `Path` and `Description` are create-only in the schema,
    /// so an update holds each to the value the policy was created with.
    fn read(properties: &Value, held: &Held) -> Result<Self, NativeRefusal> {
        let Some(model) = properties.as_object() else {
            return Err(NativeRefusal::Invalid(format!(
                "{TYPE_NAME} requires a resource model of its properties"
            )));
        };
        let mut desired = Self {
            document: None,
            description: None,
        };
        for (property, value) in model {
            match property.as_str() {
                IDENTIFIER_PROPERTY => require_value(property, value, &held.arn)?,
                MANAGED_POLICY_NAME => require_value(property, value, &held.name)?,
                PATH => require_value(property, value, &held.path)?,
                DESCRIPTION => {
                    if held.exists {
                        // Create-only: the description an existing policy carries is the one
                        // it was created with, and IAM publishes no call that changes it, so
                        // a model may state that description back and nothing else — a
                        // policy carrying none is held to carrying none rather than taking a
                        // value this write would then drop.
                        require_held(TYPE_NAME, property, value, held.description.as_deref())?;
                    } else {
                        desired.description = Some(text(property, value)?);
                    }
                }
                POLICY_DOCUMENT => desired.document = Some(document_text(value)?),
                POLICY_ID => {
                    require_held(TYPE_NAME, property, value, held.policy_id.as_deref())?;
                }
                DEFAULT_VERSION_ID => {
                    require_held(
                        TYPE_NAME,
                        property,
                        value,
                        held.default_version_id.as_deref(),
                    )?;
                }
                ATTACHMENT_COUNT => require_number(property, value, held.attachment_count)?,
                PERMISSIONS_BOUNDARY_USAGE_COUNT => {
                    require_number(property, value, held.permissions_boundary_usage_count)?;
                }
                IS_ATTACHABLE => require_flag(property, value, held.is_attachable)?,
                other if UNAPPLIED_PROPERTIES.contains(&other) => {
                    return Err(unsupported(TYPE_NAME, other));
                }
                other => {
                    return Err(NativeRefusal::Invalid(format!(
                        "{other} is not a property of {TYPE_NAME}"
                    )));
                }
            }
        }
        // What an IAM JSON policy IS stays the OWNING service's own judgement, asked before
        // anything is written: the elements a managed policy's document must carry, the
        // values an identity-based policy's statements may declare, and the quota on its
        // size.
        if let Some(document) = &desired.document {
            if let Some(defect) = iam::managed_policy_document_defect(document) {
                return Err(NativeRefusal::Invalid(defect.message().to_string()));
            }
            if let Some(defect) =
                ::service_common::iam_policy::identity_policy_value_defect(document)
            {
                return Err(NativeRefusal::Invalid(defect.message()));
            }
            if iam::policy_document_characters(document) > iam::MAX_MANAGED_POLICY_CHARACTERS {
                return Err(NativeRefusal::AtCapacity(iam::policy_size_quota_exceeded(
                    iam::MAX_MANAGED_POLICY_CHARACTERS,
                )));
            }
        }
        Ok(desired)
    }

    /// The document the model names, which a managed policy cannot be without.
    fn document(&self) -> Result<String, NativeRefusal> {
        self.document
            .clone()
            .ok_or_else(|| NativeRefusal::Invalid(format!("{TYPE_NAME} requires {POLICY_DOCUMENT}")))
    }
}

/// The policy document text the `PolicyDocument` `value` carries.
///
/// The schema types it as the JSON object a template writes the policy inline as, and the
/// owning record holds that document as text; a document carried as TEXT is the same policy
/// written the other way the registry admits, so both are read here.
fn document_text(value: &Value) -> Result<String, NativeRefusal> {
    match value {
        Value::Object(_) => Ok(value.to_string()),
        Value::String(document) => Ok(document.clone()),
        _ => Err(NativeRefusal::Invalid(format!(
            "{POLICY_DOCUMENT} must be a JSON object carrying the policy"
        ))),
    }
}

/// Check that `property` carries exactly the whole number `expected` — the rule a read-only
/// COUNT of the type's schema is held to, beside the one its text properties are.
fn require_number(property: &str, value: &Value, expected: i32) -> Result<(), NativeRefusal> {
    (value.as_i64() == Some(i64::from(expected)))
        .then_some(())
        .ok_or_else(|| {
            NativeRefusal::Invalid(format!("{property} of this resource is {expected}"))
        })
}

/// Check that `property` carries exactly the flag `expected`; see [`require_number`].
fn require_flag(property: &str, value: &Value, expected: bool) -> Result<(), NativeRefusal> {
    (value.as_bool() == Some(expected))
        .then_some(())
        .ok_or_else(|| {
            NativeRefusal::Invalid(format!("{property} of this resource is {expected}"))
        })
}

/// The resource model one managed policy renders as — the schema's properties the owning
/// record carries, and only those it HAS: a policy with no description carries none, the
/// same way the owning service's own readers report none.
fn model(held: &iam::ManagedPolicy, attachment_count: i32, boundary_uses: i32) -> Value {
    let mut model = Map::new();
    if let Some(arn) = &held.policy.arn {
        model.insert(IDENTIFIER_PROPERTY.to_string(), Value::from(arn.clone()));
    }
    if let Some(name) = &held.policy.policy_name {
        model.insert(MANAGED_POLICY_NAME.to_string(), Value::from(name.clone()));
    }
    if let Some(path) = &held.policy.path {
        model.insert(PATH.to_string(), Value::from(path.clone()));
    }
    if let Some(description) = &held.policy.description {
        model.insert(DESCRIPTION.to_string(), Value::from(description.clone()));
    }
    if let Some(policy_id) = &held.policy.policy_id {
        model.insert(POLICY_ID.to_string(), Value::from(policy_id.clone()));
    }
    model.insert(
        DEFAULT_VERSION_ID.to_string(),
        Value::from(held.default_version_id().to_string()),
    );
    model.insert(
        ATTACHMENT_COUNT.to_string(),
        Value::from(attachment_count),
    );
    model.insert(
        PERMISSIONS_BOUNDARY_USAGE_COUNT.to_string(),
        Value::from(boundary_uses),
    );
    model.insert(
        IS_ATTACHABLE.to_string(),
        Value::from(held.policy.is_attachable),
    );
    // The document the policy's DEFAULT version holds, which is "the version that is used
    // for all principals" — reported as the JSON object the schema declares.
    if let Some(document) = held
        .versions
        .get(held.default_version_id())
        .and_then(|version| version.document.as_deref())
    {
        let parsed = ::serde_json::from_str::<Value>(document)
            .unwrap_or_else(|_| Value::from(document.to_string()));
        model.insert(POLICY_DOCUMENT.to_string(), parsed);
    }
    Value::Object(model)
}
