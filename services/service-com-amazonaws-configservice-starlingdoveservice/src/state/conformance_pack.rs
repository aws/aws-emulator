// SPDX-License-Identifier: CC0-1.0

//! One stored conformance pack: the pack as it was put and the projections the
//! conformance-pack read paths report it through.

use super::*;

use crate::ops::types::{
    ConformancePackScope, ConformancePackSource, DescribeConformancePacksFilters,
};

/// The `Type` of a conformance pack the account authored itself and deployed from a
/// template with `PutConformancePack` (`ConformancePackType`: "`CUSTOM` – A custom
/// conformance pack").
///
/// Every conformance pack read reports which KIND of pack it is describing, so a client
/// can tell a pack the account authored from one an Amazon Web Services service deployed
/// on its behalf — the latter being the pack that names that service in `CreatedBy`
/// (`ConformancePackDetail$CreatedBy`: "The Amazon Web Services service that created the
/// conformance pack"). A pack deployed from a TEMPLATE is the account's own, so that is
/// the type its record carries ([`ConformancePackEntry::pack_type`]).
pub const CONFORMANCE_PACK_TYPE_CUSTOM: &str = "CUSTOM";

/// The `Type` of a MANAGED conformance pack (`ConformancePackType`: "`MANAGED` – A
/// managed conformance pack"): one deployed from a managed `Source` rather than from a
/// template the account wrote, "predefined collections of Config rules that are created
/// and maintained by Amazon Web Services" (`PutConformancePackRequest$Source`).
///
/// A managed pack is the one kind that carries [`ConformancePackEntry::source`] and
/// [`ConformancePackEntry::scope`], which is why the type and those two members move
/// together, and the kind a pack is is fixed at creation: "You cannot convert an
/// existing conformance pack between a managed conformance pack and a custom
/// conformance pack".
pub const CONFORMANCE_PACK_TYPE_MANAGED: &str = "MANAGED";

/// How Config words a `Scope` named without the `Source` it belongs to.
///
/// `PutConformancePackRequest$Scope` is "supported only for managed conformance packs.
/// If you specify a scope, you must also specify `Source`": a scope defines which
/// resources trigger an evaluation for the rules of the managed pack a source names, so
/// it stands for nothing on its own. The text is fixed — it is built from no part of
/// the request — because the member it is about is the one the request named.
pub const CONFORMANCE_PACK_SCOPE_NEEDS_SOURCE_MESSAGE: &str =
    "When you specify Scope, you must also specify Source";

/// How Config words a deployment that would change WHICH KIND of conformance pack an
/// existing pack is.
///
/// "You cannot convert an existing conformance pack between a managed conformance pack
/// and a custom conformance pack" (`PutConformancePackRequest$Source`): the kind was
/// fixed when the pack was created ([`ConformancePackEntry::pack_type`]), so the pack
/// is left as it is and the refusal states the rule.
pub const CONFORMANCE_PACK_KIND_IS_FIXED_MESSAGE: &str =
    "You cannot convert an existing conformance pack between a managed conformance pack \
     and a custom conformance pack";

/// One conformance pack deployed in this account and Region.
///
/// A conformance pack is deployed as a CloudFormation stack, which is why the pack
/// carries both its own ARN and the stack's ([`ConformancePackStatusDetail$StackArn`]).
/// Its deployment is NOT finished when `PutConformancePack` answers: the pack is
/// `CREATE_IN_PROGRESS` until the deployment is due (see
/// [`CONFORMANCE_PACK_DEPLOY_SETTLE_AFTER`]) and `CREATE_COMPLETE` from then on. The
/// entry therefore stores the two INSTANTS and derives the state from the clock, so no
/// read has to move it and two reads at the same instant agree.
///
/// What the deployment CREATED is part of the pack: the Config rules its template
/// declares are written into
/// [`ComAmazonawsConfigserviceStarlingdoveserviceState::config_rules`] (and their
/// remediation configurations beside them) and named here in
/// [`Self::deployed_rule_names`], and the CloudFormation stack it created is named in
/// [`Self::stack_arn`] — so a pack reported complete is a pack whose resources exist; a
/// deployment that could not create them carries [`Self::failure_reason`], is reported
/// `CREATE_FAILED`, and has neither resources nor a stack.
///
/// [`ConformancePackStatusDetail$StackArn`]: ConformancePackStatusDetail
#[derive(Debug, Clone)]
pub struct ConformancePackEntry {
    /// `ConformancePackName`, which is unique per account per Region and is what every
    /// conformance pack operation names the pack by.
    pub name: String,
    /// The `ConformancePackId` Config assigned the pack when it was created; it is the
    /// last segment of [`Self::arn`] and never changes once minted.
    pub id: String,
    pub arn: String,
    /// The ARN of the CloudFormation stack the pack is deployed as, where the pack HAS
    /// one.
    ///
    /// The deployment is what CREATES the stack, and a pack keeps the stack it was
    /// deployed as for as long as it lives, so a redeploy reports the one it was created
    /// with. A deployment that could not be performed ([`Self::failure_reason`]) created
    /// none, so a pack no deployment of which has succeeded has no stack to report and
    /// carries no `StackArn` at all ([`Self::to_status_detail`]) rather than an ARN that
    /// resolves to nothing.
    pub stack_arn: Option<String>,
    /// WHICH KIND of conformance pack this is, as the pack's reads report it in `Type`
    /// ([`CONFORMANCE_PACK_TYPE_CUSTOM`]). Taken at creation from the provenance of the
    /// call that created the pack, and kept by a redeploy, so a pack's provenance cannot
    /// change under it.
    pub pack_type: String,
    /// The template the pack was last deployed from, byte for byte as it was sent, and
    /// the Amazon S3 location Config stores conformance pack templates at. Both are
    /// replaced by a later `PutConformancePack` for the same name, and
    /// `DescribeConformancePacks` is what reports the pack's stored configuration back.
    pub template_body: Option<String>,
    pub delivery_s3_bucket: Option<String>,
    pub delivery_s3_key_prefix: Option<String>,
    /// The `ConformancePackInputParameter` objects the pack was deployed with.
    pub input_parameters: Vec<ConformancePackInputParameter>,
    /// The Config rules this pack's deployment CREATED, by the names Config gave them
    /// ([`conformance_pack_rule_name`]).
    ///
    /// They are the pack's own resources, so a redeploy of this pack replaces exactly
    /// these and nothing else in
    /// [`ComAmazonawsConfigserviceStarlingdoveserviceState::config_rules`]. A pack whose
    /// deployment failed ([`Self::failure_reason`]) deployed none, so this is empty.
    pub deployed_rule_names: Vec<String>,
    /// Why the pack's deployment failed, when it did
    /// (`ConformancePackStatusDetail$ConformancePackStatusReason`: "The reason of
    /// conformance pack creation failure"), which is what puts the pack in
    /// [`CONFORMANCE_PACK_STATE_CREATE_FAILED`] rather than reporting a deployment that
    /// created nothing as complete.
    pub failure_reason: Option<String>,
    /// "Last time when conformation pack creation and update was requested" — the
    /// instant of the call that last deployed this pack.
    pub last_update_requested_at: SystemTime,
    /// When that deployment is due to be complete; see
    /// [`CONFORMANCE_PACK_DEPLOY_SETTLE_AFTER`].
    pub deployment_completes_at: SystemTime,
    /// Tags are taken at creation and not updated by `PutConformancePack` ("Tags are
    /// added at creation and cannot be updated with this operation"); they are this
    /// resource's one tag set, reached through
    /// [`ComAmazonawsConfigserviceStarlingdoveserviceState::tags_of`].
    pub tags: TagSet,
    /// The managed conformance pack this pack was deployed from, for a pack that was:
    /// "contains the identifier of the managed conformance pack that Config deploys in
    /// your account ... This field applies only to managed conformance packs"
    /// (`ConformancePackDetail$Source`). A pack deployed from a template
    /// ([`Self::template_body`]) has none, and the two are exclusive — a request naming
    /// a `Source` may name no template at all.
    pub source: Option<ConformancePackSource>,
    /// Which resources trigger an evaluation for the pack's Config rules, where the
    /// pack was deployed with a scope: "managed conformance packs with a scope populate
    /// this field. Custom conformance packs leave it empty"
    /// (`ConformancePackDetail$Scope`). It is carried only beside [`Self::source`], as
    /// `PutConformancePackRequest$Scope` requires ("if you specify a scope, you must
    /// also specify `Source`").
    pub scope: Option<ConformancePackScope>,
}

impl ConformancePackEntry {
    /// The `ConformancePackState` the pack is in at `now`: a deployment that could not
    /// create the resources the template declares FAILED, and otherwise it is in
    /// progress until the instant it is due to complete, and complete from then on.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if self.failure_reason.is_some() {
            CONFORMANCE_PACK_STATE_CREATE_FAILED
        } else if now >= self.deployment_completes_at {
            CONFORMANCE_PACK_STATE_CREATE_COMPLETE
        } else {
            CONFORMANCE_PACK_STATE_CREATE_IN_PROGRESS
        }
    }

    /// The modelled `ConformancePackStatusDetail`, read back out of state at `now`.
    ///
    /// `ConformancePackStatusReason` is "the reason of conformance pack creation
    /// failure", which a deployment that is in progress or complete has none of;
    /// `StackArn` is "Amazon Resource Name (ARN) of CloudFormation stack", which a pack
    /// whose deployment created no stack has none of ([`Self::stack_arn`]); and
    /// `LastUpdateCompletedTime` is "last time when conformation pack creation and
    /// update was successful", which neither a deployment still in progress nor a failed
    /// one has reached.
    pub fn to_status_detail(&self, now: SystemTime) -> ConformancePackStatusDetail {
        let complete = self.state_at(now) == CONFORMANCE_PACK_STATE_CREATE_COMPLETE;
        ConformancePackStatusDetail {
            conformance_pack_name: self.name.clone(),
            r#type: Some(self.pack_type.clone()),
            conformance_pack_id: self.id.clone(),
            conformance_pack_arn: self.arn.clone(),
            conformance_pack_state: self.state_at(now).to_string(),
            stack_arn: self.stack_arn.clone(),
            conformance_pack_status_reason: self.failure_reason.clone(),
            last_update_requested_time: timestamp(self.last_update_requested_at),
            last_update_completed_time: complete
                .then(|| timestamp(self.deployment_completes_at)),
        }
    }

    /// The modelled `ConformancePackDetail` — the pack's stored CONFIGURATION, as
    /// `DescribeConformancePacks` reports it, which is what the deployment was last
    /// requested with rather than how far that deployment has got
    /// ([`Self::to_status_detail`] is what reports the latter).
    ///
    /// `CreatedBy` is "the Amazon Web Services service that created the conformance
    /// pack", which a pack the account deployed itself through `PutConformancePack` has
    /// none of, and `TemplateSSMDocumentDetails` is carried only by a pack deployed from
    /// a Systems Manager document, which this store holds none of: `PutConformancePack`
    /// refuses that leg rather than deploying from a template it never read.
    ///
    /// `Source` and `Scope` are the MANAGED pack's own members, so they are reported
    /// from what the pack was deployed with ([`Self::source`], [`Self::scope`]) and a
    /// pack deployed from a template carries neither.
    pub fn to_detail(&self) -> ConformancePackDetail {
        ConformancePackDetail {
            conformance_pack_name: self.name.clone(),
            r#type: Some(self.pack_type.clone()),
            conformance_pack_arn: self.arn.clone(),
            conformance_pack_id: self.id.clone(),
            delivery_s3_bucket: self.delivery_s3_bucket.clone(),
            delivery_s3_key_prefix: self.delivery_s3_key_prefix.clone(),
            conformance_pack_input_parameters: Some(self.input_parameters.clone()),
            last_update_requested_time: Some(timestamp(self.last_update_requested_at)),
            created_by: None,
            template_ssm_document_details: None,
            source: self.source.clone(),
            scope: self.scope.clone(),
        }
    }

    /// Whether a conformance pack read that filters by `Type` reports this pack.
    ///
    /// `DescribeConformancePacksFilters$Types` "filters results by conformance pack
    /// type ... if you specify both values, Config returns conformance packs of either
    /// type", and a request carrying no filter asks for every pack ("by default, if the
    /// filter is not defined, Config returns details for all your conformance packs").
    /// A value outside the published `ConformancePackType` names no type any pack is,
    /// so it selects none.
    pub fn selected_by_types(&self, filters: Option<&DescribeConformancePacksFilters>) -> bool {
        let Some(types) = filters.and_then(|filters| filters.types.as_deref()) else {
            return true;
        };
        types.contains(&self.pack_type)
    }
}

impl ComAmazonawsConfigserviceStarlingdoveserviceState {
    /// Drop the resources ONE conformance pack's deployment created, named by
    /// [`ConformancePackEntry::deployed_rule_names`].
    ///
    /// A pack's resources are the Config rules its template declared and the remediation
    /// actions declared for them, so this is the ONE statement of what goes when a pack's
    /// deployment is undone: `DeleteConformancePack` removes them with the pack ("all the
    /// Config rules, remediation actions, and all evaluation results within that
    /// conformance pack" — a rule's evaluation results live on the rule, so removing the
    /// rule removes them), and a REDEPLOY of the same pack removes exactly these before
    /// creating what the new template declares. Nothing else in the account's rules is
    /// touched.
    pub fn remove_conformance_pack_resources(&mut self, rule_names: &[String]) {
        for rule_name in rule_names {
            self.config_rules.remove(rule_name);
            self.remediation_configurations.remove(rule_name);
        }
    }
}
