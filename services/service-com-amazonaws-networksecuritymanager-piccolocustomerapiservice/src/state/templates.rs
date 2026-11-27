// SPDX-License-Identifier: CC0-1.0

//! The Template entity: the record `CreateTemplate` persists and `GetTemplate` and
//! `ListTemplates` read back.
//!
//! Kept beside [`super`]'s policy record rather than inside any one `src/ops/*.rs`, so
//! every template operation agrees on one copy of the record, the id minting and the
//! `templateIdentifier` resolution — the lookup a read's 200 and its modelled
//! `ResourceNotFoundException` are both decided by.

use std::collections::BTreeMap;
use std::time::SystemTime;

use super::{
    ComAmazonawsNetworksecuritymanagerPiccolocustomerapiserviceState as State, mint_entity_id,
    to_date_time,
};
use crate::ops::types::{AssociatedRule, RuleReference, TemplateSummary};

/// `ServiceResourceType` naming a template, for the `resourceId`/`resourceType` members
/// the modelled `ResourceNotFoundException` / `ServiceQuotaExceededException` shapes
/// carry. One of `ServiceResourceType`'s modelled values.
pub const RESOURCE_TYPE_TEMPLATE: &str = "Template";

/// The most templates this emulator stores, after which `CreateTemplate` answers with the
/// modelled `ServiceQuotaExceededException` (HTTP 402) instead of growing
/// [`State::templates`] for as long as a caller keeps asking. A CHOSEN emulator capacity,
/// not a quota this service documents: what matters is that the store has a ceiling and
/// that reaching it is refused through a modelled error rather than by exhausting the
/// host.
pub const MAX_TEMPLATES: usize = 100;

/// The largest page `ListTemplates` can answer with, and so the page size used when the
/// caller sends no `maxResults`: `TemplateSummaryList` is modelled `@length(max: 100)`,
/// so a larger page could not be serialized at all.
pub const MAX_TEMPLATE_PAGE: usize = 100;

/// `TemplateOwner` for a template this account holds — the model: "CUSTOMER – The template
/// is in your account". Every template this store holds was persisted by `CreateTemplate`
/// in the caller's own account, so it is the owner every stored template reports; the
/// other value, `AWS`, names an AWS managed template, which no operation of this service
/// creates and this store therefore never holds.
pub const TEMPLATE_OWNER_CUSTOMER: &str = "CUSTOMER";

/// One persisted template: every member `GetTemplate` answers with, plus the bookkeeping
/// the lifecycle needs (`tags`, `created_at`).
#[derive(Debug, Clone)]
pub struct Template {
    pub template_id: String,
    pub template_arn: String,
    pub template_name: String,
    pub template_description: Option<String>,
    /// `EntityStatus` — [`super::STATUS_ACTIVE`] or [`super::STATUS_DRAFT`], computed
    /// from `isPublished` by [`super::status_for`].
    pub status: String,
    /// `TemplateFirewallType`. Fixed at create: no modelled operation changes it.
    pub firewall_type: String,
    /// The rules this template is associated with, as the output member
    /// `associatedRuleList` returns them (`AssociatedRule` carries the rule's ARN).
    pub associated_rule_list: Vec<AssociatedRule>,
    /// Rendered as the modelled `EntityVersion` (`^[1-9][0-9]*$`) by
    /// [`Template::version_string`]; starts at 1 and is bumped by every update.
    pub version: u32,
    /// `UpdateToken` — a UUID rotated on every mutation, so a caller holding a stale one
    /// is refused with `ConflictException`.
    pub update_token: String,
    pub is_snapshot: bool,
    pub has_published_version: bool,
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl Template {
    /// The modelled `EntityVersion` rendering of [`Template::version`].
    pub fn version_string(&self) -> String {
        self.version.to_string()
    }

    /// `updatedAt` as the modelled `DateTimestamp`.
    pub fn updated_at_timestamp(&self) -> ::aws_smithy_types::DateTime {
        to_date_time(self.updated_at)
    }

    /// The `TemplateOwner` this template reports — see [`TEMPLATE_OWNER_CUSTOMER`].
    pub fn template_owner(&self) -> &'static str {
        TEMPLATE_OWNER_CUSTOMER
    }

    /// This template as the `TemplateSummary` `ListTemplates` returns.
    pub fn summary(&self) -> TemplateSummary {
        TemplateSummary {
            template_id: self.template_id.clone(),
            template_arn: self.template_arn.clone(),
            template_name: self.template_name.clone(),
            template_owner: Some(self.template_owner().to_string()),
            status: Some(self.status.clone()),
            version: Some(self.version_string()),
            has_published_version: Some(self.has_published_version),
            firewall_type: Some(self.firewall_type.clone()),
            updated_at: Some(self.updated_at_timestamp()),
        }
    }
}

impl State {
    /// A fresh `TemplateId`, minted exactly as a `PolicyId` is (see [`mint_entity_id`]):
    /// it satisfies the modelled `^[a-z0-9]{1,64}$` and cannot collide with an id this
    /// store already handed out.
    pub fn mint_template_id(&mut self) -> String {
        mint_entity_id(&mut self.next_template_seq)
    }

    /// The `templateId` a `templateIdentifier` names — the template's ARN, as the model
    /// documents it, or the bare id — or `None` when no stored template answers to it.
    pub fn resolve_template_id(&self, identifier: &str) -> Option<String> {
        if self.templates.contains_key(identifier) {
            return Some(identifier.to_string());
        }
        self.templates
            .values()
            .find(|template| template.template_arn == identifier)
            .map(|template| template.template_id.clone())
    }

    /// The template a `templateIdentifier` names.
    pub fn template_by_identifier(&self, identifier: &str) -> Option<&Template> {
        let id = self.resolve_template_id(identifier)?;
        self.templates.get(&id)
    }
}

/// The `associatedRuleList` entry a request's `RuleReference` becomes.
///
/// The model: "The identifier of the rule. This is the rule's Amazon Resource Name (ARN)"
/// — so the reference carries the ARN and the association echoes it under `ruleArn`.
pub fn associated_rule_of(reference: &RuleReference) -> AssociatedRule {
    AssociatedRule {
        rule_arn: reference.rule_identifier.clone(),
    }
}

/// Refuse a template's rule references the service could not answer with, returning the
/// offending member and the message naming what is wrong — `None` when the list is
/// usable. The check itself is [`super::reference_arn_violation`], shared with every
/// other entity that holds references.
pub fn template_reference_violation(rules: &[RuleReference]) -> Option<(&'static str, String)> {
    super::reference_arn_violation(rules.iter().map(|reference| {
        (
            "associatedRuleList",
            "ruleIdentifier",
            reference.rule_identifier.as_str(),
        )
    }))
}
