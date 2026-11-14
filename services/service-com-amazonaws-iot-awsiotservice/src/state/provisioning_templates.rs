// SPDX-License-Identifier: CC0-1.0

//! The account's FLEET PROVISIONING TEMPLATES: the record one holds, its versions,
//! and which of them is the default.
//!
//! A provisioning template is a NAME plus a set of numbered template bodies and a
//! pointer to the one that is in effect, which is what the model describes:
//! `CreateProvisioningTemplate` answers a `defaultVersionId`,
//! `CreateProvisioningTemplateVersion` adds versions and may re-point that pointer,
//! and `DescribeProvisioningTemplate` reports the body of the default version
//! (`smithy_query(query="operation",operation="DescribeProvisioningTemplate")`) — the
//! same shape the IoT POLICY registry has ([`super::RegisteredPolicy`]).
//!
//! The name is the template's identity — the model binds it as the `@httpLabel` of
//! `/provisioning-templates/{templateName}` on every single-template operation — so
//! the store is a `BTreeMap` keyed by it, and the versions hang off the record in a
//! `BTreeMap` keyed by `versionId` so a lister reads them oldest-first without
//! sorting.
//!
//! The DEFAULT version cannot be deleted: a template always has a version in effect,
//! so `DeleteProvisioningTemplateVersion` of the default one is the modelled
//! `DeleteConflictException` until another version is made the default.

use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::ops::types::{
    DescribeProvisioningTemplateOutput, DescribeProvisioningTemplateVersionOutput,
    ProvisioningHook, ProvisioningTemplateVersionSummary,
};

/// How many provisioning templates this emulator keeps for the account.
///
/// AWS publishes a service quota on an account's fleet provisioning templates; the
/// exact figure is not in the model, so this is the emulator's own ceiling on a store
/// a caller can grow — the same reason [`super::MAX_POLICIES`] exists — and
/// `CreateProvisioningTemplate` past it refuses with the modelled
/// `LimitExceededException` rather than growing without bound.
pub const MAX_PROVISIONING_TEMPLATES: usize = 100;

/// How many versions one provisioning template holds.
///
/// `CreateProvisioningTemplateVersion` models `VersionsLimitExceededException` ("The
/// number of policy versions exceeds the limit", HTTP 409)
/// (`smithy_query(query="operation",operation="CreateProvisioningTemplateVersion")`),
/// so the service caps a template's versions; AWS publishes that cap as five
/// versions per provisioning template, the same figure
/// [`super::MAX_POLICY_VERSIONS`] records for an IoT policy.
pub const MAX_PROVISIONING_TEMPLATE_VERSIONS: usize = 5;

/// How many tags one provisioning template may carry: the same documented IoT tagging
/// limit [`super::MAX_POLICY_TAGS`] records, for the same reason.
pub const MAX_PROVISIONING_TEMPLATE_TAGS: usize = super::MAX_POLICY_TAGS;

/// The `versionId` a freshly created template's only version carries.
///
/// A template is created WITH a body, which is its first version and the one in
/// effect — `CreateProvisioningTemplate` answers a `defaultVersionId` for a template
/// that has had exactly one write — so the numbering starts at 1, the same rule
/// [`super::FIRST_POLICY_VERSION_ID`] states for a policy.
pub const FIRST_TEMPLATE_VERSION_ID: i32 = 1;

/// The `type` a template created without one carries.
///
/// `TemplateType` is `FLEET_PROVISIONING | JITP`
/// (`smithy_query(query="shape",shape="TemplateType")`), and a template created
/// through `CreateProvisioningTemplate` without naming a type is the fleet
/// provisioning kind the operation is for.
pub const DEFAULT_TEMPLATE_TYPE: &str = "FLEET_PROVISIONING";

/// Whether a template created without `enabled` is enabled.
///
/// `CreateProvisioningTemplate$enabled` is "True to enable the provisioning template,
/// otherwise false", with no modelled default, so a request that does not ask for it
/// gets a template that provisions nothing until an update enables it.
pub const DEFAULT_TEMPLATE_ENABLED: bool = false;

/// The ARN of a provisioning template:
/// `arn:aws:iot:<region>:<account>:provisioningtemplate/<templateName>`.
///
/// `provisioningtemplate/` is the resource type AWS IoT's provisioning-template ARNs
/// use — it is what `CreateProvisioningTemplate$templateArn` reports and what a
/// `CreateProvisioningClaim` policy statement names a template by; the namespace and
/// account segments are the ones [`super::thing_arn`] documents.
pub fn provisioning_template_arn(region: &str, template_name: &str) -> String {
    ::emulator_server::emulator_arn(
        "iot",
        region,
        &format!("provisioningtemplate/{template_name}"),
    )
}

/// The message every operation that addresses ONE provisioning template refuses a
/// name the account holds none under with.
///
/// The fixed text is the family's — the template is either one this account created
/// or no template at all — and the variable part is the name the request asked about.
pub fn missing_provisioning_template_message(template_name: &str) -> String {
    format!("Unable to find template named {template_name}")
}

/// The message a reader of ONE VERSION of a template refuses a `versionId` the
/// template does not hold with.
///
/// The fixed text is the family's, and the variable parts are the version and the
/// template the request named.
pub fn missing_template_version_message(template_name: &str, version_id: i32) -> String {
    format!("Unable to find a template version with ID {version_id} for template {template_name}")
}

/// How this service refuses the delete of the version a template is PROVISIONING
/// with.
///
/// A template always has a version in effect, so the default version is only
/// deletable once another version is the default; the fixed text states that and the
/// variable part is the version the request named.
pub fn default_template_version_delete_message(version_id: i32) -> String {
    format!("Unable to delete template version {version_id}, it is the default version")
}

/// How this service refuses a template that already holds
/// [`MAX_PROVISIONING_TEMPLATE_VERSIONS`] versions.
pub fn template_versions_limit_message(template_name: &str) -> String {
    format!(
        "Provisioning template {template_name} already has the maximum number of versions \
         ({MAX_PROVISIONING_TEMPLATE_VERSIONS})"
    )
}

/// Why a string is not a provisioning template body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProvisioningTemplateBodyRejection {
    /// It is not JSON at all; carries the parser's own complaint.
    NotJson(String),
    /// It is JSON, but not a JSON object, so it cannot be a template body.
    NotAnObject,
    /// It carries no `Resources` at all.
    MissingResources,
}

impl ProvisioningTemplateBodyRejection {
    /// What to tell the caller: the rule that was violated.
    pub fn message(&self) -> String {
        match self {
            Self::NotJson(complaint) => {
                format!("templateBody is not a valid JSON document: {complaint}")
            }
            Self::NotAnObject => {
                "templateBody must be a JSON object carrying Resources".to_string()
            }
            Self::MissingResources => "templateBody must carry Resources".to_string(),
        }
    }
}

/// Check `body` against the provisioning template rules, naming the first rule it
/// breaks.
///
/// A provisioning template body is a JSON document whose `Resources` section declares
/// what provisioning creates — the thing, its certificate and its policy — beside an
/// optional `Parameters` section the device's CSR request fills in. What each resource
/// declares is the caller's to shape, so nothing here reads or restricts it.
///
/// Shared by the create and the version create, so the two cannot drift apart on what
/// they accept.
pub fn validate_provisioning_template_body(
    body: &str,
) -> Result<(), ProvisioningTemplateBodyRejection> {
    let document: ::serde_json::Value = ::serde_json::from_str(body)
        .map_err(|complaint| ProvisioningTemplateBodyRejection::NotJson(complaint.to_string()))?;
    let Some(members) = document.as_object() else {
        return Err(ProvisioningTemplateBodyRejection::NotAnObject);
    };
    if !members.contains_key("Resources") {
        return Err(ProvisioningTemplateBodyRejection::MissingResources);
    }
    Ok(())
}

/// One version of a provisioning template's body, as this service persists it.
#[derive(Debug, Clone)]
pub struct ProvisioningTemplateVersion {
    /// The body this version carries, verbatim as the writer supplied it: a reader
    /// hands back what was written, never a reformatted copy.
    pub template_body: String,
    /// When this version was created.
    pub creation_date: SystemTime,
}

/// One provisioning template in the store, as this service persists it.
#[derive(Debug, Clone)]
pub struct RegisteredProvisioningTemplate {
    /// The name the template was created under; also its key in the store.
    pub template_name: String,
    /// `arn:aws:iot:<region>:<account>:provisioningtemplate/<name>`, minted on create.
    pub template_arn: String,
    /// `CreateProvisioningTemplate$description`, when the request carried one.
    pub description: Option<String>,
    /// Every version that exists, keyed by its `versionId`. Ordered, so a lister
    /// reads them oldest-first without sorting.
    pub versions: BTreeMap<i32, ProvisioningTemplateVersion>,
    /// Which version is in effect — the `defaultVersionId` a reader reports and the
    /// version whose body `DescribeProvisioningTemplate` hands back.
    pub default_version_id: i32,
    /// Whether the template provisions at all — `CreateProvisioningTemplate$enabled`.
    pub enabled: bool,
    /// `CreateProvisioningTemplate$provisioningRoleArn` — the role provisioning
    /// assumes.
    pub provisioning_role_arn: String,
    /// `CreateProvisioningTemplate$preProvisioningHook`, when the request carried one.
    pub pre_provisioning_hook: Option<ProvisioningHook>,
    /// Which of [`crate::ops::types::TEMPLATE_TYPE_VALUES`] the template is.
    pub template_type: String,
    /// `Tag$Key` -> `Tag$Value` for the tags the create request carried.
    pub tags: BTreeMap<String, Option<String>>,
    /// When the template was created — `DescribeProvisioningTemplate$creationDate`.
    pub creation_date: SystemTime,
    /// When the template last changed —
    /// `DescribeProvisioningTemplate$lastModifiedDate`.
    pub last_modified_date: SystemTime,
}

impl RegisteredProvisioningTemplate {
    /// A newly created provisioning template: the minted identity, `template_body` as
    /// version [`FIRST_TEMPLATE_VERSION_ID`] and that version set as the default.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        template_name: String,
        template_arn: String,
        description: Option<String>,
        template_body: String,
        enabled: bool,
        provisioning_role_arn: String,
        pre_provisioning_hook: Option<ProvisioningHook>,
        template_type: String,
        tags: BTreeMap<String, Option<String>>,
        created_at: SystemTime,
    ) -> Self {
        Self {
            template_name,
            template_arn,
            description,
            versions: BTreeMap::from([(
                FIRST_TEMPLATE_VERSION_ID,
                ProvisioningTemplateVersion {
                    template_body,
                    creation_date: created_at,
                },
            )]),
            default_version_id: FIRST_TEMPLATE_VERSION_ID,
            enabled,
            provisioning_role_arn,
            pre_provisioning_hook,
            template_type,
            tags,
            creation_date: created_at,
            last_modified_date: created_at,
        }
    }

    /// The body of the version that is in effect — what
    /// `DescribeProvisioningTemplate` reports.
    pub fn default_body(&self) -> &str {
        self.versions
            .get(&self.default_version_id)
            .map_or("", |version| version.template_body.as_str())
    }

    /// Add `template_body` as a new version, making it the default when the request
    /// asked for that, and answer the id it was given.
    ///
    /// The id is one past the highest the template has ever held, so an id is never
    /// reused by a later version — a caller that deleted version 2 and adds another
    /// gets 3, not 2.
    pub fn add_version(
        &mut self,
        template_body: String,
        set_as_default: bool,
        now: SystemTime,
    ) -> i32 {
        let version_id = self
            .versions
            .keys()
            .next_back()
            .map_or(FIRST_TEMPLATE_VERSION_ID, |highest| highest + 1);
        self.versions.insert(
            version_id,
            ProvisioningTemplateVersion {
                template_body,
                creation_date: now,
            },
        );
        if set_as_default {
            self.default_version_id = version_id;
        }
        self.last_modified_date = now;
        version_id
    }

    /// The record as `DescribeProvisioningTemplate` reports it — every member of the
    /// stored record, so the read and the create cannot drift.
    pub fn to_description(&self) -> DescribeProvisioningTemplateOutput {
        DescribeProvisioningTemplateOutput {
            template_arn: Some(self.template_arn.clone()),
            template_name: Some(self.template_name.clone()),
            description: self.description.clone(),
            creation_date: Some(::aws_smithy_types::DateTime::from(self.creation_date)),
            last_modified_date: Some(::aws_smithy_types::DateTime::from(self.last_modified_date)),
            default_version_id: Some(self.default_version_id),
            template_body: Some(self.default_body().to_string()),
            enabled: Some(self.enabled),
            provisioning_role_arn: Some(self.provisioning_role_arn.clone()),
            pre_provisioning_hook: self.pre_provisioning_hook.clone(),
            r#type: Some(self.template_type.clone()),
        }
    }

    /// One version as `DescribeProvisioningTemplateVersion` reports it, or `None` for
    /// a `versionId` the template does not hold.
    pub fn version_description(
        &self,
        version_id: i32,
    ) -> Option<DescribeProvisioningTemplateVersionOutput> {
        let version = self.versions.get(&version_id)?;
        Some(DescribeProvisioningTemplateVersionOutput {
            version_id: Some(version_id),
            creation_date: Some(::aws_smithy_types::DateTime::from(version.creation_date)),
            template_body: Some(version.template_body.clone()),
            is_default_version: version_id == self.default_version_id,
        })
    }

    /// Every version as `ListProvisioningTemplateVersions` summarises them,
    /// oldest-first.
    pub fn version_summaries(&self) -> Vec<ProvisioningTemplateVersionSummary> {
        self.versions
            .iter()
            .map(|(version_id, version)| ProvisioningTemplateVersionSummary {
                version_id: Some(*version_id),
                creation_date: Some(::aws_smithy_types::DateTime::from(version.creation_date)),
                is_default_version: *version_id == self.default_version_id,
            })
            .collect()
    }
}
