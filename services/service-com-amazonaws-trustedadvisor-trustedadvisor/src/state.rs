// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-trustedadvisor-trustedadvisor service.
//!
//! Trusted Advisor is a REPORTING API: it has no create operation, so nothing a
//! caller does brings a check, a recommendation or a recommendation resource
//! into existence. The store is therefore SEEDED deterministically the first
//! time the engine hands this state out (`state::<T>()` builds it from
//! `Default`), and every response is computed from it — never from the request.
//!
//! The catalog's human-readable text (a check's and a recommendation's `name`
//! and `description`) is held as a [`TextCatalog`]: one copy per
//! `RecommendationLanguage`, because `language` on the reporting operations
//! SELECTS which copy the response reports. A check and the recommendation it
//! produced share one catalog, so they cannot disagree.
//!
//! Four maps, each keyed by the identifier its API looks a resource up by, so
//! `BTreeMap` iteration order is also the stable pagination order:
//!
//! * [`checks`](ComAmazonawsTrustedadvisorTrustedadvisorState::checks) — keyed by check id.
//! * [`recommendations`](ComAmazonawsTrustedadvisorTrustedadvisorState::recommendations) —
//!   keyed by recommendation ARN, which is what `GetRecommendation`,
//!   `ListRecommendationResources` and `UpdateRecommendationLifecycle` take as
//!   their `@httpLabel`.
//! * [`recommendation_resources`](ComAmazonawsTrustedadvisorTrustedadvisorState::recommendation_resources) —
//!   keyed by recommendation-resource ARN, which is what
//!   `BatchUpdateRecommendationResourceExclusion` names its entries by.
//! * [`organization_recommendations`](ComAmazonawsTrustedadvisorTrustedadvisorState::organization_recommendations) —
//!   keyed by organization-recommendation ARN, which is what
//!   `GetOrganizationRecommendation` and
//!   `UpdateOrganizationRecommendationLifecycle` take as their `@httpLabel`,
//!   and what `ListOrganizationRecommendations` reports in that key order. It
//!   holds only PRIORITIZED recommendations ([`RECOMMENDATION_TYPE_PRIORITY`]),
//!   because every organization operation of this service is documented as
//!   supporting only prioritized recommendations.
//!
//! A recommendation does NOT keep a list of its resource ARNs: a resource
//! carries its owning `recommendation_arn`, so
//! [`resources_of`](ComAmazonawsTrustedadvisorTrustedadvisorState::resources_of)
//! derives the set from the one copy rather than keeping a second one that
//! could disagree. For the same reason `resourcesAggregates` is never stored:
//! [`aggregates_for`](ComAmazonawsTrustedadvisorTrustedadvisorState::aggregates_for)
//! counts the recommendation's own resources on every read, and
//! [`organization_aggregates_for`](ComAmazonawsTrustedadvisorTrustedadvisorState::organization_aggregates_for)
//! counts an organization recommendation's by summing those of the member
//! accounts' recommendations it aggregates — an organization recommendation
//! stores no resources of its own. The ARN of the AWS resource a recommendation
//! flagged — what `ListRecommendationsForResource` looks a resource up by — is
//! derived the same way, by
//! [`aws_resource_arn_of`](ComAmazonawsTrustedadvisorTrustedadvisorState::aws_resource_arn_of)
//! from the resource's own Region, id and owning account and the resource TYPE
//! of the check behind it, so no stored ARN can disagree with them.
//!
//! Beside the four maps the store holds the calling account's
//! [`SupportPlan`] — the whole input to the support-level authorization stage
//! [`support_level_refusal`], which every operation applies before it resolves
//! an identifier or writes anything. A plan arrives from OUTSIDE this API, which
//! models no operation that reads, buys or changes one, so the store starts on
//! the plan an account holds until it subscribes to another
//! ([`UNSUBSCRIBED_SUPPORT_PLAN`]) unless the operator names the one the account
//! it wants emulated holds ([`SUPPORT_PLAN_VAR`], resolved by
//! [`seeded_support_plan`]). An operation whose input names an AWS resource in
//! ANOTHER account answers for that resource first
//! ([`foreign_resource_refusal`]): no support plan grants an account access to
//! another account's resources.
//!
//! The one rule a lifecycle update applies to a stored recommendation lives
//! here as well — [`apply_lifecycle_update`], with the guard
//! [`lifecycle_update_conflict`] ahead of it — because the account-level
//! [`RecommendationEntry`] and the organization-level
//! [`OrganizationRecommendationEntry`] are different shapes carrying the SAME
//! lifecycle members ([`LifecycleWritable`]): the write that replaces them is
//! stated once rather than once per operation, so the two levels cannot drift
//! apart on it.
//!
//! The two levels' READ of one stored [`ResourceEntry`] lives here for the same
//! reason: [`resource_summary`] is the projection both resource lists report,
//! [`organization_resource_summary`] is that projection with the two members the
//! organization view is modelled to carry differently, and [`ResourceFilters`]
//! is the `status` / `exclusionStatus` / `regionCode` predicate both reads cut
//! their rows on — each stated once, so no account-level and organization-level
//! view of one resource can disagree.
//!
//! The two levels' LISTING of recommendations lives here for that same reason:
//! [`RecommendationFilters`] is the `type` / `status` / `pillar` / `awsService`
//! / `source` / `checkIdentifier` predicate and the two inclusive
//! `lastUpdatedAt` bounds both listings cut their rows on, plus the cursor scope
//! those filters contribute, read off either entry type through
//! [`RecommendationFilterable`] — so the account-level and the
//! organization-level listing cannot select differently on the members the two
//! entry types carry identically.
//!
//! The same ownership test ([`is_foreign_resource`]) SCOPES the account-level
//! views: [`recommendations`](ComAmazonawsTrustedadvisorTrustedadvisorState::recommendations)
//! and [`recommendation_resources`](ComAmazonawsTrustedadvisorTrustedadvisorState::recommendation_resources)
//! hold every account's rows — the organization surface exists to aggregate the
//! member accounts' recommendations — so an account-level listing reports only
//! the rows whose own ARN names the calling account, and the cross-account view
//! of those same rows is the organization listing.

use std::collections::BTreeMap;

use crate::ops::types::{
    AccessDeniedException, ConflictException, OrganizationRecommendationResourceSummary,
    RecommendationCostOptimizingAggregates, RecommendationPillarSpecificAggregates,
    RecommendationResourceSummary, RecommendationResourcesAggregates,
};

/// What this service answers a caller whose account's support plan does not
/// include the Trusted Advisor API: the support-level gate's refusal, carried by
/// the modelled `AccessDeniedException` — HTTP 403, documented "Exception that
/// access has been denied due to insufficient access"
/// (`smithy_query(query="doc",shape="AccessDeniedException")`).
///
/// The gate is an account-level authorization stage, not a per-request
/// computation, so its wording is ONE fixed sentence rather than something built
/// from the request: what a client branches on is the 403 and the
/// `AccessDeniedException` code, and this is the sentence the service states them
/// with.
pub const ACCESS_DENIED_SUPPORT_LEVEL: &str = "Access denied due to support level";

/// The AWS Support plan an account holds — what decides whether the Trusted
/// Advisor API answers that account at all.
///
/// The five plans AWS offers (AWS Support plans: Basic, Developer, Business,
/// Enterprise On-Ramp, Enterprise). `Basic` is what an account holds until it
/// subscribes to one of the others, and no modelled operation of this service
/// reads or changes the plan — it is an account property, not a resource of this
/// API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportPlan {
    Basic,
    Developer,
    Business,
    EnterpriseOnRamp,
    Enterprise,
}

impl SupportPlan {
    /// Every plan, in the order AWS publishes them (least to most support), so
    /// anything that spans "every plan" reads the set from here rather than
    /// writing it out again.
    pub const ALL: &'static [Self] = &[
        Self::Basic,
        Self::Developer,
        Self::Business,
        Self::EnterpriseOnRamp,
        Self::Enterprise,
    ];

    /// Whether this plan's entitlement covers the Trusted Advisor API.
    ///
    /// AWS states the requirement for the API as a whole — a Business,
    /// Enterprise On-Ramp or Enterprise Support plan is required to use the
    /// Trusted Advisor API — so an account on Basic or Developer is refused
    /// whatever it asks for, and no operation of this service is exempt.
    pub fn includes_trusted_advisor_api(self) -> bool {
        matches!(
            self,
            Self::Business | Self::EnterpriseOnRamp | Self::Enterprise
        )
    }

    /// The plan AWS publishes under `name`, or `None` for a name that is not one
    /// of the five plans.
    ///
    /// The names are the ones AWS prints for the plans (AWS Support plans:
    /// Basic, Developer, Business, Enterprise On-Ramp, Enterprise), compared
    /// without regard to case or to how the one two-word name is spelled —
    /// `enterprise-on-ramp`, `enterprise_on_ramp` and `EnterpriseOnRamp` are the
    /// same plan.
    pub fn parse(name: &str) -> Option<Self> {
        let folded: String = name
            .chars()
            .filter(|character| character.is_ascii_alphanumeric())
            .map(|character| character.to_ascii_lowercase())
            .collect();
        Self::ALL
            .iter()
            .copied()
            .find(|plan| plan.folded_name() == folded)
    }

    /// This plan's published name, lowercased with every separator removed —
    /// the one form [`Self::parse`] compares in.
    fn folded_name(self) -> &'static str {
        match self {
            Self::Basic => "basic",
            Self::Developer => "developer",
            Self::Business => "business",
            Self::EnterpriseOnRamp => "enterpriseonramp",
            Self::Enterprise => "enterprise",
        }
    }
}

/// The plan an AWS account holds until it subscribes to another one —
/// [`SupportPlan::Basic`], which every account includes and whose entitlement
/// does NOT cover the Trusted Advisor API.
///
/// This is what the store starts on, because a support subscription is a
/// property of the ACCOUNT rather than a resource of this API: no modelled
/// operation reads, buys or changes a plan, so nothing a caller does can bring a
/// subscription into existence, and an emulator that assumed one would answer
/// every unsubscribed account as though it had bought the API. The operator says
/// otherwise with [`SUPPORT_PLAN_VAR`].
pub const UNSUBSCRIBED_SUPPORT_PLAN: SupportPlan = SupportPlan::Basic;

/// The environment variable the operator names the emulated account's support
/// plan in, as AWS prints it (`basic`, `developer`, `business`,
/// `enterprise-on-ramp`, `enterprise` — see [`SupportPlan::parse`]).
///
/// A subscription arrives from outside this API, so the emulator takes it from
/// outside too: this is the one input that records the plan the account being
/// emulated holds, and it is what makes the entitled surface of the service
/// reachable by a black-box client, which no request could ask for.
pub const SUPPORT_PLAN_VAR: &str = "EMULATOR_TRUSTEDADVISOR_SUPPORT_PLAN";

/// The support plan the store starts on: the one [`SUPPORT_PLAN_VAR`] names, or
/// [`UNSUBSCRIBED_SUPPORT_PLAN`] when it names nothing this service recognizes.
///
/// Read once, when the engine builds this state. An in-process caller holding the
/// engine moves the plan afterwards with
/// [`set_support_plan`](ComAmazonawsTrustedadvisorTrustedadvisorState::set_support_plan),
/// since nothing on the wire changes a support subscription.
pub fn seeded_support_plan() -> SupportPlan {
    std::env::var(SUPPORT_PLAN_VAR)
        .ok()
        .and_then(|name| SupportPlan::parse(name.trim()))
        .unwrap_or(UNSUBSCRIBED_SUPPORT_PLAN)
}

/// The `ExclusionStatus` value a resource carries unless a caller excluded it.
/// `RecommendationResourceSummary$exclusionStatus` declares
/// `@default("included")`.
pub const EXCLUSION_INCLUDED: &str = "included";

/// The other `ExclusionStatus` value (see `EXCLUSION_STATUS_VALUES` in
/// `ops::types::shared_types`).
pub const EXCLUSION_EXCLUDED: &str = "excluded";

/// The `RecommendationType` whose lifecycle `UpdateRecommendationLifecycle` can
/// move: its model doc says "This API only supports prioritized
/// recommendations". Every ORGANIZATION operation —
/// `ListOrganizationRecommendations`, `GetOrganizationRecommendation`,
/// `ListOrganizationRecommendationAccounts`,
/// `ListOrganizationRecommendationResources` and
/// `UpdateOrganizationRecommendationLifecycle` — carries that same sentence, so
/// they all share this value: an organization recommendation exists only for a
/// recommendation of this type.
pub const RECOMMENDATION_TYPE_PRIORITY: &str = "priority";

/// The one `UpdateRecommendationLifecycleStage` value that RESOLVES a
/// recommendation (see `UPDATE_RECOMMENDATION_LIFECYCLE_STAGE_VALUES` in
/// `ops::types::shared_types`): the stage on which an update stamps
/// `resolvedAt`, documented "When the Recommendation was resolved". Shared by
/// the account-level and the organization-level lifecycle update, which model
/// the same stage enum.
pub const LIFECYCLE_STAGE_RESOLVED: &str = "resolved";

/// The one `RecommendationPillar`
/// `RecommendationPillarSpecificAggregates` reports an aggregate for: that
/// structure models exactly one member, `costOptimizing`, and
/// `Recommendation$pillarSpecificAggregates` is documented "The pillar
/// aggregations for cost savings" (`RECOMMENDATION_PILLAR_VALUES` in
/// `ops::types::shared_types` holds every pillar a recommendation can carry).
pub const PILLAR_COST_OPTIMIZING: &str = "cost_optimizing";

/// The check-metadata column a Trusted Advisor cost check reports a resource's
/// estimated monthly saving in — the per-resource figure the cost aggregate
/// sums.
///
/// The savings are read out of the resource's own metadata because that is
/// where the check reports them (`CheckSummary$metadata` names the columns and
/// `RecommendationResourceSummary$metadata` carries this resource's values), so
/// the aggregate is derived from the ONE copy a caller can also read, rather
/// than from a second, shadow number that could disagree with the column.
pub const METADATA_ESTIMATED_MONTHLY_SAVINGS: &str = "Estimated Monthly Savings";

/// The three values `ResourceStatus` models, each with the
/// `RecommendationResourcesAggregates` counter it feeds (see
/// `RESOURCE_STATUS_VALUES` in `ops::types::shared_types`).
pub const RESOURCE_STATUS_OK: &str = "ok";
pub const RESOURCE_STATUS_WARNING: &str = "warning";
pub const RESOURCE_STATUS_ERROR: &str = "error";

/// A check ARN: `arn:aws:trustedadvisor:::check/<id>`.
///
/// Formatted here rather than through `engine::emulator_arn`, which always
/// writes the account segment: `CheckSummary$arn` is modelled
/// `^arn:[\w-]+:trustedadvisor:::check\/[\w-]+$`, i.e. with an EMPTY account.
pub fn check_arn(check_id: &str) -> String {
    format!("arn:aws:trustedadvisor:::check/{check_id}")
}

/// A recommendation ARN: `arn:aws:trustedadvisor::<account>:recommendation/<id>`,
/// matching `AccountRecommendationArn`'s
/// `^arn:[\w-]+:trustedadvisor::\d{12}:recommendation\/[\w-]+$`. The account
/// segment is the emulator's canonical account id.
pub fn recommendation_arn(recommendation_id: &str) -> String {
    ::engine::emulator_arn(
        "trustedadvisor",
        "",
        &format!("recommendation/{recommendation_id}"),
    )
}

/// An organization-recommendation ARN:
/// `arn:aws:trustedadvisor:::organization-recommendation/<id>`, matching
/// `OrganizationRecommendationArn`'s
/// `^arn:[\w-]+:trustedadvisor:::organization-recommendation\/[\w-]+$`.
///
/// Formatted here rather than through `engine::emulator_arn`, which always
/// writes the account segment: an organization recommendation spans the
/// organization's accounts, so its ARN carries an EMPTY account — the same
/// reason [`check_arn`] is formatted by hand.
pub fn organization_recommendation_arn(organization_recommendation_id: &str) -> String {
    format!("arn:aws:trustedadvisor:::organization-recommendation/{organization_recommendation_id}")
}

/// A recommendation-resource ARN:
/// `arn:aws:trustedadvisor::<account>:recommendation-resource/<rec id>/<resource id>`,
/// matching `RecommendationResourceArn`'s
/// `^arn:[\w-]+:trustedadvisor::\d{12}:recommendation-resource\/[\w-]+\/[\w-]+$`.
pub fn recommendation_resource_arn(recommendation_id: &str, resource_id: &str) -> String {
    ::engine::emulator_arn(
        "trustedadvisor",
        "",
        &format!("recommendation-resource/{recommendation_id}/{resource_id}"),
    )
}

/// The ARN of the AWS resource a check flagged — what
/// `ListRecommendationsForResource` looks a resource up by and what
/// `RecommendationForResourceSummary$awsResourceArn` reports.
///
/// The ARN form is a property of the RESOURCE TYPE, as AWS publishes it per
/// service (the Service Authorization Reference's "Resource types" table), so
/// the mapping is keyed by the CloudFormation type name a check carries in
/// `CheckSummary$awsResourceTypes`:
///
/// * `AWS::EC2::Instance` → `arn:aws:ec2:<region>:<account>:instance/<id>`
/// * `AWS::EC2::Volume` → `arn:aws:ec2:<region>:<account>:volume/<id>`
/// * `AWS::RDS::DBInstance` → `arn:aws:rds:<region>:<account>:db:<name>`
///
/// `None` for a type this table does not name: the emulator does not know that
/// type's ARN form, so it reports no ARN for it rather than guessing one that a
/// caller could not have formed. The account segment is `account_id`, the
/// account that OWNS the flagged resource, and the Region segment is the
/// resource's own `regionCode` — an AWS resource ARN is Regional.
///
/// Formatted here rather than through `engine::emulator_arn`, which always
/// writes the emulator's own account: the store spans the organization's member
/// accounts, so the resources of a member account are named by ARNs carrying
/// THAT account — the same reason [`check_arn`] is formatted by hand.
pub fn aws_resource_arn(
    resource_type: &str,
    account_id: &str,
    region_code: &str,
    aws_resource_id: &str,
) -> Option<String> {
    let (service, resource) = match resource_type {
        "AWS::EC2::Instance" => ("ec2", format!("instance/{aws_resource_id}")),
        "AWS::EC2::Volume" => ("ec2", format!("volume/{aws_resource_id}")),
        "AWS::RDS::DBInstance" => ("rds", format!("db:{aws_resource_id}")),
        _ => return None,
    };
    Some(format!(
        "arn:aws:{service}:{region_code}:{account_id}:{resource}"
    ))
}

/// The key prefix every resource of `recommendation_arn` shares: that ARN with
/// its `recommendation/` resource part rewritten to `recommendation-resource/`,
/// plus the `/` that separates the resource id. That trailing `/` is what keeps
/// the prefix of `.../foo/` from also matching `.../foo-bar/...`.
///
/// `None` for an ARN that is not a recommendation ARN, which therefore names no
/// stored resource.
fn resource_key_prefix(recommendation_arn: &str) -> Option<String> {
    let (scope, recommendation_id) = recommendation_arn.split_once(":recommendation/")?;
    Some(format!("{scope}:recommendation-resource/{recommendation_id}/"))
}

/// Wall-clock now as milliseconds since the Unix epoch — what an operation
/// stamps on a value it updates. Taken from the clock, never from the request.
pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
}

/// A modelled `date-time` as milliseconds since the Unix epoch, so a filter
/// bound and a stored `lastUpdatedAt` are compared on one scale. `to_millis` is
/// the exact conversion — going through seconds as an `f64` could move an
/// inclusive bound by a millisecond. The inverse of [`date_time`].
pub fn millis_of(value: &::aws_smithy_types::DateTime) -> i64 {
    value.to_millis().unwrap_or(i64::MAX)
}

/// Milliseconds since the Unix epoch as the modelled `date-time` timestamp.
///
/// `from_millis` is the exact conversion: going through seconds as an `f64`
/// would divide by 1000 in binary floating point, which is not representable
/// for most millisecond values and can shift the rendered sub-second part.
pub fn date_time(millis: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// The `createdAt` a stored recommendation of `recommendation_type` reports:
/// `Some(..)` only for a recommendation created by Trusted Advisor Priority.
///
/// Every recommendation shape of this service models the member as "When the
/// Recommendation was created, if created by AWS Trusted Advisor Priority", and
/// models `type` as exactly that distinction — "Whether the Recommendation was
/// automated or generated by AWS Trusted Advisor Priority". So a recommendation
/// whose type is not [`RECOMMENDATION_TYPE_PRIORITY`] was produced
/// automatically, has no Priority creation instant, and reports none — the same
/// scoping that leaves `lifecycleStage`, `createdBy` and the
/// `updatedOnBehalfOf…` members absent on such a recommendation.
///
/// The store keeps the instant a row entered it either way, so this is the ONE
/// place the scoping is applied: every surface that reports a recommendation or
/// a recommendation summary goes through this function, which is what keeps the
/// account-level and the organization-level views from drifting apart on it.
pub fn priority_created_at(
    recommendation_type: &str,
    created_at_millis: i64,
) -> Option<::aws_smithy_types::DateTime> {
    (recommendation_type == RECOMMENDATION_TYPE_PRIORITY).then(|| date_time(created_at_millis))
}

// ---------------------------------------------------------------------------
// Localized catalog text
// ---------------------------------------------------------------------------

/// The `RecommendationLanguage` the catalog's text is written in when a request
/// names no language: `en`, the first value `RecommendationLanguage` declares
/// (`RECOMMENDATION_LANGUAGE_VALUES` in `ops::types::shared_types`) and the
/// language AWS answers a request that asks for none in.
pub const DEFAULT_LANGUAGE: &str = "en";

/// The human-readable text of one catalog entry in ONE language: what a
/// localizable response member (`CheckSummary$name`/`$description`,
/// `Recommendation$name`/`$description`, `RecommendationSummary$name`) is copied
/// from.
#[derive(Debug, Clone)]
pub struct LocalizedText {
    pub name: String,
    pub description: String,
}

/// One catalog entry's text in every language the store holds a copy in.
///
/// `language` on `ListChecks`, `ListRecommendations`, `GetRecommendation` and
/// `ListRecommendationResources` is documented as "the ISO 639-1 code for the
/// language that you want your checks/recommendations to appear in", so it
/// SELECTS which stored copy of the text a response reports — it is not a
/// filter and never changes which entries are reported. A language the entry
/// has no copy in is answered in [`DEFAULT_LANGUAGE`], which every entry holds
/// by construction.
#[derive(Debug, Clone)]
pub struct TextCatalog {
    /// The text in [`DEFAULT_LANGUAGE`].
    default: LocalizedText,
    /// One copy per other language, keyed by its `RecommendationLanguage`
    /// value.
    translations: BTreeMap<String, LocalizedText>,
}

impl TextCatalog {
    /// A catalog whose [`DEFAULT_LANGUAGE`] copy is `default` and which holds
    /// `translations` (language, name, description) for the other languages.
    pub fn new<'a>(
        default: LocalizedText,
        translations: impl IntoIterator<Item = (&'a str, &'a str, &'a str)>,
    ) -> Self {
        Self {
            default,
            translations: translations
                .into_iter()
                .filter(|(language, _, _)| *language != DEFAULT_LANGUAGE)
                .map(|(language, name, description)| {
                    (
                        language.to_string(),
                        LocalizedText {
                            name: name.to_string(),
                            description: description.to_string(),
                        },
                    )
                })
                .collect(),
        }
    }

    /// The text to report for a request that asked for `language` (`None` when
    /// it named none): the stored copy for that language, or the
    /// [`DEFAULT_LANGUAGE`] copy when this entry holds none.
    pub fn text(&self, language: Option<&str>) -> &LocalizedText {
        language
            .filter(|language| *language != DEFAULT_LANGUAGE)
            .and_then(|language| self.translations.get(language))
            .unwrap_or(&self.default)
    }

    /// Every language this entry holds a copy in, `en` first then the rest in
    /// language order. Lets a caller see the catalog's coverage without
    /// reaching into its map.
    pub fn languages(&self) -> impl Iterator<Item = &str> {
        std::iter::once(DEFAULT_LANGUAGE).chain(self.translations.keys().map(String::as_str))
    }
}

/// A Trusted Advisor check: the catalog entry `ListChecks` reports.
///
/// Enum-typed members (`pillars`, `source`) are held as the wire strings the
/// model's enums accept — `RECOMMENDATION_PILLAR_VALUES` /
/// `RECOMMENDATION_SOURCE_VALUES` in `ops::types::shared_types` — which is also
/// how the generated shapes carry them.
#[derive(Debug, Clone)]
pub struct Check {
    pub id: String,
    pub arn: String,
    /// This check's `name`/`description`, one copy per language — read through
    /// [`TextCatalog::text`] with the request's `language`.
    pub text: TextCatalog,
    pub pillars: Vec<String>,
    pub aws_services: Vec<String>,
    pub source: String,
    /// Column headings for the metadata a resource of this check carries,
    /// keyed by the column index as a string (`{"0": "Region", ...}`).
    pub metadata: BTreeMap<String, String>,
    pub resource_arn_queryable: bool,
    pub aws_resource_types: Vec<String>,
    pub check_granularity: Option<String>,
    pub recommendation_id: Option<String>,
}

/// A recommendation: one per seeded check, the unit `ListRecommendations`,
/// `GetRecommendation` and `UpdateRecommendationLifecycle` work on.
#[derive(Debug, Clone)]
pub struct RecommendationEntry {
    pub id: String,
    pub arn: String,
    /// This recommendation's `name`/`description`, one copy per language — the
    /// text of the check that produced it, read through [`TextCatalog::text`]
    /// with the request's `language`.
    pub text: TextCatalog,
    pub check_arn: Option<String>,
    pub r#type: String,
    pub status: String,
    pub lifecycle_stage: Option<String>,
    pub pillars: Vec<String>,
    pub source: String,
    pub aws_services: Vec<String>,
    /// When this recommendation came into being. Reported as `createdAt` only
    /// for a recommendation created by Trusted Advisor Priority — read it
    /// through [`priority_created_at`], which applies that scoping.
    pub created_at_millis: i64,
    pub last_updated_at_millis: i64,
    pub resolved_at_millis: Option<i64>,
    pub update_reason: Option<String>,
    pub update_reason_code: Option<String>,
}

/// A recommendation for the accounts of an AWS Organizations organization: the
/// unit `ListOrganizationRecommendations`, `GetOrganizationRecommendation` and
/// `UpdateOrganizationRecommendationLifecycle` work on.
///
/// Its lifecycle stage, update reason and resolution instant are its OWN: an
/// organization recommendation is a separate resource from the member accounts'
/// recommendations, so moving one does not move the other.
///
/// Only a PRIORITIZED recommendation ([`RECOMMENDATION_TYPE_PRIORITY`]) has
/// one: every operation that reports, resolves or moves an organization
/// recommendation supports only prioritized recommendations, so `type` is that
/// value on every stored entry.
///
/// The organization-level view of one check's findings, so it carries the same
/// classification members as the account-level [`RecommendationEntry`] and
/// shares that check's [`TextCatalog`]. It stores NO resources of its own:
/// `member_recommendation_arns` names the accounts' recommendations it
/// aggregates, and
/// [`organization_aggregates_for`](ComAmazonawsTrustedadvisorTrustedadvisorState::organization_aggregates_for)
/// counts their resources on every read.
#[derive(Debug, Clone)]
pub struct OrganizationRecommendationEntry {
    pub id: String,
    pub arn: String,
    /// This recommendation's `name`/`description` — the text of the check that
    /// produced it. `GetOrganizationRecommendation` models no `language`, so
    /// the organization operations report the [`DEFAULT_LANGUAGE`] copy.
    pub text: TextCatalog,
    pub check_arn: Option<String>,
    pub r#type: String,
    pub status: String,
    pub lifecycle_stage: Option<String>,
    pub pillars: Vec<String>,
    pub source: String,
    pub aws_services: Vec<String>,
    /// The ARNs of the member accounts' recommendations this organization
    /// recommendation aggregates — keys of
    /// [`recommendations`](ComAmazonawsTrustedadvisorTrustedadvisorState::recommendations).
    /// The emulator presents exactly one account
    /// (`engine::EMULATOR_ACCOUNT_ID`), so the seed records that account's
    /// recommendation for the same check; the member set is a list so an
    /// organization with more accounts needs no second shape.
    ///
    /// A SET held in a `Vec`: every read of it goes through
    /// [`member_recommendations_of`](ComAmazonawsTrustedadvisorTrustedadvisorState::member_recommendations_of),
    /// which yields each member recommendation once, so no organization-level
    /// view reports or counts one twice.
    pub member_recommendation_arns: Vec<String>,
    /// When this organization recommendation came into being — reported as
    /// `createdAt` through [`priority_created_at`], as on the account-level
    /// entry.
    pub created_at_millis: i64,
    pub last_updated_at_millis: i64,
    pub resolved_at_millis: Option<i64>,
    pub update_reason: Option<String>,
    pub update_reason_code: Option<String>,
}

// ---------------------------------------------------------------------------
// The lifecycle write
// ---------------------------------------------------------------------------

/// The members of one stored recommendation that a lifecycle update writes,
/// borrowed from the entry they live on: the stage, the two reason members, the
/// `lastUpdatedAt` stamp and the resolution instant.
///
/// Handed out by [`LifecycleWritable::lifecycle_members`] so
/// [`apply_lifecycle_update`] writes them on EITHER kind of stored
/// recommendation without naming either one.
pub struct LifecycleMembers<'entry> {
    pub lifecycle_stage: &'entry mut Option<String>,
    pub update_reason: &'entry mut Option<String>,
    pub update_reason_code: &'entry mut Option<String>,
    pub last_updated_at_millis: &'entry mut i64,
    pub resolved_at_millis: &'entry mut Option<i64>,
}

/// A stored recommendation whose lifecycle an `Update…Lifecycle` operation
/// moves: the account-level [`RecommendationEntry`] and the organization-level
/// [`OrganizationRecommendationEntry`].
///
/// The two operations model the same stage and reason-code enums and carry the
/// same "only supports prioritized recommendations" sentence, so they apply ONE
/// rule to two kinds of entry. This is the seam that lets that rule be stated
/// once — by [`lifecycle_update_conflict`] and [`apply_lifecycle_update`] —
/// instead of once per operation, where the account-level and the
/// organization-level lifecycle could drift apart on the same members.
pub trait LifecycleWritable {
    /// What this service calls the resource whose lifecycle is being moved, for
    /// the refusal [`lifecycle_update_conflict`] builds.
    const RESOURCE_NOUN: &'static str;

    /// This entry's `RecommendationType`: the classification that decides
    /// whether a lifecycle update applies to it at all.
    fn recommendation_type(&self) -> &str;

    /// This entry's own ARN — what a refusal names the refused resource by.
    fn arn(&self) -> &str;

    /// The members a lifecycle update replaces, borrowed mutably.
    fn lifecycle_members(&mut self) -> LifecycleMembers<'_>;
}

impl LifecycleWritable for RecommendationEntry {
    const RESOURCE_NOUN: &'static str = "recommendation";

    fn recommendation_type(&self) -> &str {
        &self.r#type
    }

    fn arn(&self) -> &str {
        &self.arn
    }

    fn lifecycle_members(&mut self) -> LifecycleMembers<'_> {
        LifecycleMembers {
            lifecycle_stage: &mut self.lifecycle_stage,
            update_reason: &mut self.update_reason,
            update_reason_code: &mut self.update_reason_code,
            last_updated_at_millis: &mut self.last_updated_at_millis,
            resolved_at_millis: &mut self.resolved_at_millis,
        }
    }
}

impl LifecycleWritable for OrganizationRecommendationEntry {
    const RESOURCE_NOUN: &'static str = "organization recommendation";

    fn recommendation_type(&self) -> &str {
        &self.r#type
    }

    fn arn(&self) -> &str {
        &self.arn
    }

    fn lifecycle_members(&mut self) -> LifecycleMembers<'_> {
        LifecycleMembers {
            lifecycle_stage: &mut self.lifecycle_stage,
            update_reason: &mut self.update_reason,
            update_reason_code: &mut self.update_reason_code,
            last_updated_at_millis: &mut self.last_updated_at_millis,
            resolved_at_millis: &mut self.resolved_at_millis,
        }
    }
}

/// The refusal a lifecycle update answers for an entry it does not apply to:
/// `Some(..)` when the entry's type is not [`RECOMMENDATION_TYPE_PRIORITY`], so
/// the request is a `ConflictException` (HTTP 409, "Exception that the request
/// was denied due to conflictions in state") rather than a write — both
/// lifecycle operations are documented as supporting only prioritized
/// recommendations, which makes them inapplicable to an entry of any other
/// type.
///
/// The message is built from the entry: it names the type that makes the
/// operation inapplicable and the resource it was asked for.
pub fn lifecycle_update_conflict<Entry: LifecycleWritable>(
    entry: &Entry,
) -> Option<ConflictException> {
    (entry.recommendation_type() != RECOMMENDATION_TYPE_PRIORITY).then(|| ConflictException {
        message: format!(
            "the lifecycle of the {} {} {} cannot be updated: this operation only supports \
             prioritized recommendations",
            entry.recommendation_type(),
            Entry::RESOURCE_NOUN,
            entry.arn(),
        ),
    })
}

/// Apply a lifecycle update to `entry`: the write BOTH `Update…Lifecycle`
/// operations make, stated once.
///
/// The operations are `@idempotent` PUTs, so a request REPLACES the lifecycle
/// of the recommendation: the new stage and BOTH reason members are written as
/// the request states them, which means an omitted `updateReason` /
/// `updateReasonCode` clears the stored one rather than leaving the previous
/// update's wording attached to the new stage. `lastUpdatedAt` is stamped from
/// the clock, never from the request, and `resolvedAt` — "When the
/// Recommendation was resolved" — is that same instant exactly when the new
/// stage is [`LIFECYCLE_STAGE_RESOLVED`], so an update that moves the
/// recommendation back out of `resolved` clears it.
pub fn apply_lifecycle_update<Entry: LifecycleWritable>(
    entry: &mut Entry,
    lifecycle_stage: &str,
    update_reason: Option<&str>,
    update_reason_code: Option<&str>,
) {
    let updated_at = now_millis();
    let members = entry.lifecycle_members();
    *members.lifecycle_stage = Some(lifecycle_stage.to_string());
    *members.update_reason = update_reason.map(str::to_string);
    *members.update_reason_code = update_reason_code.map(str::to_string);
    *members.last_updated_at_millis = updated_at;
    *members.resolved_at_millis =
        (lifecycle_stage == LIFECYCLE_STAGE_RESOLVED).then_some(updated_at);
}

/// One AWS resource a recommendation flagged.
#[derive(Debug, Clone)]
pub struct ResourceEntry {
    pub id: String,
    pub arn: String,
    pub recommendation_arn: String,
    pub aws_resource_id: String,
    pub region_code: String,
    pub status: String,
    /// The values for this resource, positionally aligned with the owning
    /// check's `metadata` column headings.
    pub metadata: BTreeMap<String, String>,
    pub last_updated_at_millis: i64,
    pub exclusion_status: String,
    /// What this resource currently costs per month — the DENOMINATOR of
    /// `RecommendationCostOptimizingAggregates$estimatedPercentMonthlySavings`,
    /// which is the recommendation's estimated monthly saving as a percentage
    /// of what its resources are being spent on today.
    ///
    /// `None` for a resource of a check that does not look at cost: no modelled
    /// member reports this figure, so it exists only where a cost aggregate is
    /// computed from it, and a non-cost resource contributes nothing.
    pub estimated_monthly_spend: Option<f64>,
}

// ---------------------------------------------------------------------------
// The resource views
// ---------------------------------------------------------------------------

/// The stored resource, as the modelled `RecommendationResourceSummary`.
///
/// This is the projection of a [`ResourceEntry`] that BOTH resource lists
/// report — `ListRecommendationResources` over one recommendation's resources
/// and `ListOrganizationRecommendationResources` over the member accounts'
/// resources an organization recommendation aggregates — so it is stated here
/// ONCE, as [`apply_lifecycle_update`] states the one lifecycle write of the two
/// levels. Reporting the two views from one projection is what makes them unable
/// to disagree about a resource, instead of agreeing only while two copies of
/// the projection happen to be written identically.
pub fn resource_summary(resource: &ResourceEntry) -> RecommendationResourceSummary {
    RecommendationResourceSummary {
        id: resource.id.clone(),
        arn: resource.arn.clone(),
        aws_resource_id: resource.aws_resource_id.clone(),
        region_code: resource.region_code.clone(),
        status: resource.status.clone(),
        metadata: resource
            .metadata
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        last_updated_at: date_time(resource.last_updated_at_millis),
        exclusion_status: resource.exclusion_status.clone(),
        recommendation_arn: resource.recommendation_arn.clone(),
    }
}

/// The same stored resource, as the modelled
/// `OrganizationRecommendationResourceSummary`: [`resource_summary`]'s
/// projection, with the two members the organization view is modelled to carry
/// differently layered on top. Every other value is the one the account-level
/// view reports, taken from the same projection rather than written out again,
/// so the two views of one resource cannot disagree. The two:
///
/// * `accountId` is the account segment of the resource's own ARN — the account
///   that owns it — so no stored field can contradict the ARN a caller reads
///   beside it, taken through [`arn_account_id`], the one derivation every view
///   of a stored resource's owning account shares;
/// * `recommendationArn` is the ARN of the ORGANIZATION recommendation the
///   resource is reported under, which is what that member is modelled to carry
///   (`OrganizationRecommendationArn`, pattern
///   `^arn:[\w-]+:trustedadvisor:::organization-recommendation\/[\w-]+$`) — the
///   member account's own recommendation ARN does not satisfy it.
pub fn organization_resource_summary(
    resource: &ResourceEntry,
    organization_recommendation_arn: &str,
) -> OrganizationRecommendationResourceSummary {
    // Destructured by NAME, so every shared member reaches the organization
    // view as the projection computed it: the account-level
    // `recommendationArn` is the only one this view replaces outright.
    let RecommendationResourceSummary {
        id,
        arn,
        aws_resource_id,
        region_code,
        status,
        metadata,
        last_updated_at,
        exclusion_status,
        recommendation_arn: _,
    } = resource_summary(resource);
    OrganizationRecommendationResourceSummary {
        id,
        arn,
        aws_resource_id,
        region_code,
        status,
        metadata,
        last_updated_at,
        exclusion_status,
        account_id: arn_account_id(&resource.arn),
        recommendation_arn: organization_recommendation_arn.to_string(),
    }
}

/// The resource filters BOTH resource lists cut their rows on, as one read's
/// request states them: `status`, `exclusionStatus` and `regionCode`, each
/// `None` when the request named it not at all.
///
/// The three are modelled identically on the two operations and select over the
/// resource's own stored members, so the predicate is stated once — by
/// [`Self::keeps`] — and the cursor scope the filters contribute once, by
/// [`Self::scope`]. `ListOrganizationRecommendationResources` layers its extra
/// `affectedAccountId` test on top of this one; it has no account-level
/// counterpart, because an account-level list reports one account's resources.
#[derive(Debug, Clone, Copy)]
pub struct ResourceFilters<'request> {
    pub status: Option<&'request str>,
    pub exclusion_status: Option<&'request str>,
    pub region_code: Option<&'request str>,
}

impl<'request> ResourceFilters<'request> {
    /// Whether this read reports `resource`: a filter in force keeps a resource
    /// whose own member EQUALS the value asked for, and a filter the request
    /// omitted keeps every resource.
    pub fn keeps(&self, resource: &ResourceEntry) -> bool {
        self.status.is_none_or(|status| resource.status == status)
            && self
                .exclusion_status
                .is_none_or(|exclusion| resource.exclusion_status == exclusion)
            && self
                .region_code
                .is_none_or(|region| resource.region_code == region)
    }

    /// These filters as [`list_scope`] names them, under the member names the
    /// requests carry and in the order both reads list them — so a token issued
    /// under one filter set cannot resume a differently filtered sequence.
    pub fn scope(&self) -> Vec<(&'static str, Option<&'request str>)> {
        vec![
            ("status", self.status),
            ("exclusionStatus", self.exclusion_status),
            ("regionCode", self.region_code),
        ]
    }
}

// ---------------------------------------------------------------------------
// The recommendation listings' filters
// ---------------------------------------------------------------------------

/// The members of one stored recommendation a recommendation listing's filters
/// select on, borrowed from the entry they live on: its classification members
/// and its `lastUpdatedAt` stamp.
///
/// Handed out by [`RecommendationFilterable::classification`] so
/// [`RecommendationFilters::keeps`] selects EITHER kind of stored
/// recommendation without naming either one, exactly as [`LifecycleMembers`]
/// lets [`apply_lifecycle_update`] write both.
pub struct RecommendationClassification<'entry> {
    pub r#type: &'entry str,
    pub status: &'entry str,
    pub pillars: &'entry [String],
    pub aws_services: &'entry [String],
    pub source: &'entry str,
    pub check_arn: Option<&'entry str>,
    pub last_updated_at_millis: i64,
}

/// A stored recommendation a `List…Recommendations` operation reports: the
/// account-level [`RecommendationEntry`] and the organization-level
/// [`OrganizationRecommendationEntry`].
///
/// The two listings model the same eight filters over members the two entry
/// types carry identically, so they apply ONE rule to two kinds of entry. This
/// is the seam that lets that rule be stated once — by
/// [`RecommendationFilters`] — instead of once per operation, where the
/// account-level and the organization-level listing could drift apart on the
/// same members.
pub trait RecommendationFilterable {
    /// The members a listing's filters read, borrowed.
    fn classification(&self) -> RecommendationClassification<'_>;
}

impl RecommendationFilterable for RecommendationEntry {
    fn classification(&self) -> RecommendationClassification<'_> {
        RecommendationClassification {
            r#type: &self.r#type,
            status: &self.status,
            pillars: &self.pillars,
            aws_services: &self.aws_services,
            source: &self.source,
            check_arn: self.check_arn.as_deref(),
            last_updated_at_millis: self.last_updated_at_millis,
        }
    }
}

impl RecommendationFilterable for OrganizationRecommendationEntry {
    fn classification(&self) -> RecommendationClassification<'_> {
        RecommendationClassification {
            r#type: &self.r#type,
            status: &self.status,
            pillars: &self.pillars,
            aws_services: &self.aws_services,
            source: &self.source,
            check_arn: self.check_arn.as_deref(),
            last_updated_at_millis: self.last_updated_at_millis,
        }
    }
}

/// The filters BOTH recommendation listings cut their rows on, as one read's
/// request states them: `type`, `status`, `pillar`, `awsService`, `source`,
/// `checkIdentifier` and the two `lastUpdatedAt` bounds, each `None` when the
/// request named it not at all.
///
/// The eight are modelled identically on the two operations and select over
/// members both entry types carry, so the predicate is stated once — by
/// [`Self::keeps`] — and the cursor scope the filters contribute once, by
/// [`Self::scope`], as [`ResourceFilters`] already states the resource lists'.
/// `ListRecommendations` layers its extra ownership test
/// ([`is_foreign_resource`]) on top of this one; it has no organization-level
/// counterpart, because the organization listing is the cross-account view.
#[derive(Debug, Clone, Copy)]
pub struct RecommendationFilters<'request> {
    pub r#type: Option<&'request str>,
    pub status: Option<&'request str>,
    pub pillar: Option<&'request str>,
    pub aws_service: Option<&'request str>,
    pub source: Option<&'request str>,
    pub check_identifier: Option<&'request str>,
    /// `afterLastUpdatedAt`, as the request's timestamp.
    pub after_last_updated_at: Option<&'request ::aws_smithy_types::DateTime>,
    /// `beforeLastUpdatedAt`, as the request's timestamp.
    pub before_last_updated_at: Option<&'request ::aws_smithy_types::DateTime>,
}

impl RecommendationFilters<'_> {
    /// `afterLastUpdatedAt` and `beforeLastUpdatedAt` on the scale the stored
    /// `lastUpdatedAt` is held on — the ONE conversion both bounds go through
    /// ([`millis_of`]), so the predicate and the cursor scope read the same
    /// instant.
    fn bounds_millis(&self) -> (Option<i64>, Option<i64>) {
        (
            self.after_last_updated_at.map(millis_of),
            self.before_last_updated_at.map(millis_of),
        )
    }

    /// Whether this read reports `entry`: `afterLastUpdatedAt` and
    /// `beforeLastUpdatedAt` are INCLUSIVE bounds on the stored `lastUpdatedAt`,
    /// `pillar` and `awsService` are membership tests over the lists the entry
    /// holds, the remaining filters are equality tests on the entry's own
    /// member, and a filter the request omitted keeps every recommendation.
    pub fn keeps<Entry: RecommendationFilterable>(&self, entry: &Entry) -> bool {
        let row = entry.classification();
        let (after, before) = self.bounds_millis();
        self.r#type.is_none_or(|kind| row.r#type == kind)
            && self.status.is_none_or(|status| row.status == status)
            && self
                .pillar
                .is_none_or(|pillar| row.pillars.iter().any(|stored| stored == pillar))
            && self
                .aws_service
                .is_none_or(|service| row.aws_services.iter().any(|stored| stored == service))
            && self.source.is_none_or(|source| row.source == source)
            && self
                .check_identifier
                .is_none_or(|check| row.check_arn == Some(check))
            && after.is_none_or(|bound| row.last_updated_at_millis >= bound)
            && before.is_none_or(|bound| row.last_updated_at_millis <= bound)
    }

    /// The cursor scope of a read of `list` under these filters: [`list_scope`]
    /// over the filters in force, under the member names the requests carry and
    /// in the order both reads list them — so a token issued under one filter
    /// set cannot resume a differently filtered sequence. The two timestamp
    /// bounds enter it on the same millisecond scale the predicate compares
    /// them on.
    pub fn scope(&self, list: &str) -> String {
        let (after, before) = self.bounds_millis();
        let after = after.map(|millis| millis.to_string());
        let before = before.map(|millis| millis.to_string());
        list_scope(
            list,
            &[
                ("type", self.r#type),
                ("status", self.status),
                ("pillar", self.pillar),
                ("awsService", self.aws_service),
                ("source", self.source),
                ("checkIdentifier", self.check_identifier),
                ("afterLastUpdatedAt", after.as_deref()),
                ("beforeLastUpdatedAt", before.as_deref()),
            ],
        )
    }
}

/// Per-service state for com-amazonaws-trustedadvisor-trustedadvisor. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsTrustedadvisorTrustedadvisorState {
    /// Keyed by check id.
    pub checks: BTreeMap<String, Check>,
    /// Keyed by recommendation ARN, whose account segment names the account that
    /// owns the row: the map spans the organization's member accounts, so an
    /// account-level view of it is scoped by [`is_foreign_resource`].
    pub recommendations: BTreeMap<String, RecommendationEntry>,
    /// Keyed by recommendation-resource ARN, which likewise names the owning
    /// account.
    pub recommendation_resources: BTreeMap<String, ResourceEntry>,
    /// Keyed by organization-recommendation ARN.
    pub organization_recommendations: BTreeMap<String, OrganizationRecommendationEntry>,
    /// The AWS Support plan the calling account holds — the whole input to the
    /// support-level gate [`support_level_refusal`] applies ahead of every
    /// operation.
    ///
    /// [`seeded_support_plan`] at state init: a support subscription is a
    /// property of the account, not a resource of this API (which models no
    /// operation that reads, buys or changes a plan), so the store starts on the
    /// plan every account includes unless the operator named another one.
    /// Something outside the API — in this emulator the operator's
    /// [`SUPPORT_PLAN_VAR`], or an in-process caller holding the engine — records
    /// it, which is both how a subscribed account is modelled and how the gate is
    /// observed.
    support_plan: SupportPlan,
}

impl ComAmazonawsTrustedadvisorTrustedadvisorState {
    /// The support plan the calling account holds, which is what the gate admits
    /// a request on.
    pub fn support_plan(&self) -> SupportPlan {
        self.support_plan
    }

    /// Record the support plan of the calling account, so the gate admits — or
    /// goes on refusing — its requests.
    ///
    /// No modelled operation reaches this: the Trusted Advisor API has none that
    /// subscribes an account to a support plan, so a plan arrives from OUTSIDE
    /// the API — which in this emulator means the operator's [`SUPPORT_PLAN_VAR`]
    /// at state init, or an in-process caller holding the shared engine (a test
    /// standing in for a subscribed account). Nothing on the wire can grant
    /// itself the entitlement.
    pub fn set_support_plan(&mut self, plan: SupportPlan) {
        self.support_plan = plan;
    }

    /// The resources of one recommendation, in ARN order.
    ///
    /// A RANGE over the resource map, not a scan of it: every resource of a
    /// recommendation shares the key prefix
    /// [`resource_key_prefix`] derives from that recommendation's ARN, so this
    /// costs O(log n + k) even though `ListRecommendations` calls
    /// [`Self::aggregates_for`] once per row. The stored
    /// `recommendation_arn` is still compared, so the range can never widen the
    /// result.
    pub fn resources_of<'a>(
        &'a self,
        recommendation_arn: &'a str,
    ) -> impl Iterator<Item = (&'a String, &'a ResourceEntry)> + 'a {
        // A recommendation ARN outside the modelled form has no prefix — and no
        // resource either, since every stored resource ARN is built from one.
        let prefix = resource_key_prefix(recommendation_arn);
        let start = prefix.clone().unwrap_or_default();
        self.recommendation_resources
            .range(start..)
            .take_while(move |(key, _)| {
                prefix.as_ref().is_some_and(|prefix| key.starts_with(prefix))
            })
            .filter(move |(_, resource)| resource.recommendation_arn == recommendation_arn)
    }

    /// `resourcesAggregates` for a recommendation, COUNTED from its resources
    /// on every read: one counter per `ResourceStatus` value, plus the number
    /// of resources a caller excluded. Never a stored constant, so a status or
    /// exclusion change is visible on the next read without a second write.
    pub fn aggregates_for(&self, recommendation_arn: &str) -> RecommendationResourcesAggregates {
        let mut ok_count = 0i64;
        let mut warning_count = 0i64;
        let mut error_count = 0i64;
        let mut excluded_count = 0i64;
        for (_, resource) in self.resources_of(recommendation_arn) {
            match resource.status.as_str() {
                RESOURCE_STATUS_OK => ok_count += 1,
                RESOURCE_STATUS_WARNING => warning_count += 1,
                RESOURCE_STATUS_ERROR => error_count += 1,
                // `ResourceStatus` models exactly those three; a value outside
                // them cannot be stored, so there is nothing to count here.
                _ => {}
            }
            if resource.exclusion_status == EXCLUSION_EXCLUDED {
                excluded_count += 1;
            }
        }
        RecommendationResourcesAggregates {
            ok_count,
            warning_count,
            error_count,
            excluded_count: Some(excluded_count),
        }
    }

    /// The member accounts' recommendations one organization recommendation
    /// aggregates: each ONCE, in ARN order.
    ///
    /// [`OrganizationRecommendationEntry::member_recommendation_arns`] is a SET
    /// of member recommendations stored in a `Vec`, so a member named twice is
    /// still ONE member recommendation — one resource of this service, with one
    /// account, one lifecycle and one set of resources. Every organization-level
    /// view is derived through here (the accounts list, the resource union and
    /// both aggregates), so none of them can report or count a member a second
    /// time, and ARN order is the stable sequence the organization lists page
    /// in.
    ///
    /// An ARN naming no stored organization recommendation aggregates no
    /// member, so the set is empty.
    ///
    /// A named ARN the store does not actually hold a recommendation for names
    /// NOTHING — no account, no lifecycle, no resources — so it is not a member
    /// of any of those views either: the set is the named ARNs RESOLVED in
    /// [`recommendations`](Self::recommendations), and an unresolvable one is
    /// left out, exactly as every other read of a member recommendation in this
    /// crate treats it.
    pub fn member_recommendations_of(&self, organization_recommendation_arn: &str) -> Vec<&str> {
        let Some(entry) = self
            .organization_recommendations
            .get(organization_recommendation_arn)
        else {
            return Vec::new();
        };
        let mut members: Vec<&str> = entry
            .member_recommendation_arns
            .iter()
            .map(String::as_str)
            .filter(|member_arn| self.recommendations.contains_key(*member_arn))
            .collect();
        members.sort_unstable();
        members.dedup();
        members
    }

    /// `resourcesAggregates` for an organization recommendation, SUMMED over
    /// the member accounts' recommendations it aggregates: an organization
    /// recommendation owns no resources, so its counters are the counters of
    /// those recommendations' resources, counted on every read.
    ///
    /// An ARN naming no stored organization recommendation aggregates nothing,
    /// so every counter is zero — a caller never reaches this without
    /// resolving the ARN first, which answers `ResourceNotFoundException`.
    pub fn organization_aggregates_for(
        &self,
        organization_recommendation_arn: &str,
    ) -> RecommendationResourcesAggregates {
        let mut total = RecommendationResourcesAggregates {
            ok_count: 0,
            warning_count: 0,
            error_count: 0,
            excluded_count: Some(0),
        };
        for member in self.member_recommendations_of(organization_recommendation_arn) {
            let aggregates = self.aggregates_for(member);
            total.ok_count += aggregates.ok_count;
            total.warning_count += aggregates.warning_count;
            total.error_count += aggregates.error_count;
            total.excluded_count = Some(
                total.excluded_count.unwrap_or_default()
                    + aggregates.excluded_count.unwrap_or_default(),
            );
        }
        total
    }

    /// The check a recommendation was produced from, or `None` for a
    /// recommendation that names no check (`Recommendation$checkArn` is
    /// optional) or names one the store does not hold.
    ///
    /// The check map is keyed by check id, so the id is taken back out of the
    /// stored ARN rather than scanning the map.
    pub fn check_of(&self, recommendation: &RecommendationEntry) -> Option<&Check> {
        let arn = recommendation.check_arn.as_deref()?;
        let (_, check_id) = arn.split_once(":check/")?;
        self.checks.get(check_id)
    }

    /// The ARN of the AWS resource a stored recommendation resource flags —
    /// what `ListRecommendationsForResource` matches its `awsResourceArn`
    /// against.
    ///
    /// DERIVED on every read rather than stored, from the one copy of each of
    /// its parts: the resource's own `regionCode`, `awsResourceId` and owning
    /// account, and the resource TYPE of the check behind the recommendation
    /// that flagged it (see [`aws_resource_arn`] for the per-type ARN forms). So
    /// a resource can never carry an ARN that disagrees with the id, Region and
    /// owner a caller can also read.
    ///
    /// The owning account is the account segment of the stored resource's OWN
    /// ARN, read through the one derivation every view of a stored resource's
    /// owner goes through ([`arn_account_id`]) — the store spans the
    /// organization's member accounts, so an AWS resource ARN naming any other
    /// account would make one account's resources answer to another account's
    /// ARN.
    ///
    /// `None` when the check is not `resourceArnQueryable` — "Indicates whether
    /// this check is supported by the ListRecommendationsForResource API"
    /// (`smithy_query(query="doc",shape="CheckSummary",member="resourceArnQueryable")`)
    /// — when it names no resource type, when that type has no ARN form this
    /// emulator knows, or when the stored resource's own ARN names no account to
    /// own the flagged resource. Such a finding is not addressable by resource
    /// ARN, so no resource ARN can ever report it.
    pub fn aws_resource_arn_of(&self, resource: &ResourceEntry) -> Option<String> {
        let recommendation = self.recommendations.get(&resource.recommendation_arn)?;
        let check = self.check_of(recommendation)?;
        if !check.resource_arn_queryable {
            return None;
        }
        // A `resource`-granularity check reports one resource type, so the
        // first entry is the type of every resource it flags.
        let resource_type = check.aws_resource_types.first()?;
        let account_id = arn_account_id(&resource.arn)?;
        aws_resource_arn(
            resource_type,
            &account_id,
            &resource.region_code,
            &resource.aws_resource_id,
        )
    }

    /// `pillarSpecificAggregates` for a recommendation, COMPUTED from that
    /// recommendation's own resources on every read.
    ///
    /// `Some(..)` exactly when the recommendation optimizes the cost pillar
    /// ([`PILLAR_COST_OPTIMIZING`]): the structure models one member,
    /// `costOptimizing`, so a recommendation that optimizes no cost has no
    /// pillar aggregate to report and the optional member stays absent.
    ///
    /// Both members the cost aggregate declares `@required` are totals over
    /// that recommendation's resources:
    ///
    /// * `estimatedMonthlySavings` sums the per-resource saving the check
    ///   reports in its [`METADATA_ESTIMATED_MONTHLY_SAVINGS`] column;
    /// * `estimatedPercentMonthlySavings` is that sum as a percentage of the
    ///   same resources' [`ResourceEntry::estimated_monthly_spend`] — zero when
    ///   nothing is being spent on them, since no percentage of zero spend is
    ///   defined.
    ///
    /// Both totals span EVERY resource of the recommendation, exactly as
    /// [`Self::aggregates_for`]'s status counts do: an exclusion is reported on
    /// its own, as `resourcesAggregates.excludedCount`, rather than by taking
    /// the resource out of the other aggregates. Nothing is stored, so a
    /// resource's metadata or spend changing is visible on the very next read.
    pub fn pillar_aggregates_for(
        &self,
        recommendation_arn: &str,
    ) -> Option<RecommendationPillarSpecificAggregates> {
        let (savings, spend) = self.cost_totals(recommendation_arn)?;
        Some(cost_aggregate(savings, spend))
    }

    /// `pillarSpecificAggregates` for an ORGANIZATION recommendation, computed
    /// the same way over the member accounts' recommendations it aggregates:
    /// the cost totals of each member are summed and the percentage is taken
    /// from those sums, so the organization's figure is the organization's own
    /// ratio rather than an average of ratios.
    ///
    /// `Some(..)` exactly when at least one member recommendation optimizes the
    /// cost pillar, for the reason [`Self::pillar_aggregates_for`] gives: the
    /// structure models only `costOptimizing`.
    pub fn organization_pillar_aggregates_for(
        &self,
        organization_recommendation_arn: &str,
    ) -> Option<RecommendationPillarSpecificAggregates> {
        let mut totals: Option<(f64, f64)> = None;
        for member in self.member_recommendations_of(organization_recommendation_arn) {
            if let Some((savings, spend)) = self.cost_totals(member) {
                let (total_savings, total_spend) = totals.unwrap_or((0.0, 0.0));
                totals = Some((total_savings + savings, total_spend + spend));
            }
        }
        let (savings, spend) = totals?;
        Some(cost_aggregate(savings, spend))
    }

    /// The cost totals of one recommendation's resources — the sum of the
    /// per-resource saving the check reports in its
    /// [`METADATA_ESTIMATED_MONTHLY_SAVINGS`] column, and the sum of their
    /// [`ResourceEntry::estimated_monthly_spend`] — or `None` when the
    /// recommendation does not optimize the cost pillar and so has no cost
    /// aggregate at all.
    fn cost_totals(&self, recommendation_arn: &str) -> Option<(f64, f64)> {
        let entry = self.recommendations.get(recommendation_arn)?;
        if !entry
            .pillars
            .iter()
            .any(|pillar| pillar == PILLAR_COST_OPTIMIZING)
        {
            return None;
        }

        // The positional key of the savings column in this check's headings;
        // `None` for a cost check that reports no such column, whose resources
        // then contribute no saving.
        let savings_column = self
            .check_of(entry)
            .and_then(|check| metadata_column(check, METADATA_ESTIMATED_MONTHLY_SAVINGS));

        let mut savings = 0.0f64;
        let mut spend = 0.0f64;
        for (_, resource) in self.resources_of(recommendation_arn) {
            savings += savings_column
                .and_then(|column| resource.metadata.get(column))
                .and_then(|value| value.trim().parse::<f64>().ok())
                .unwrap_or(0.0);
            spend += resource.estimated_monthly_spend.unwrap_or(0.0);
        }
        Some((savings, spend))
    }
}

/// The cost pillar aggregate for `savings` out of `spend`: the savings as
/// reported, and as a percentage of the spend — zero when nothing is being
/// spent, since no percentage of zero spend is defined.
fn cost_aggregate(savings: f64, spend: f64) -> RecommendationPillarSpecificAggregates {
    let percent = if spend > 0.0 {
        savings / spend * 100.0
    } else {
        0.0
    };
    RecommendationPillarSpecificAggregates {
        cost_optimizing: Some(RecommendationCostOptimizingAggregates {
            estimated_monthly_savings: savings,
            estimated_percent_monthly_savings: percent,
        }),
    }
}

/// The key under which a check's resources carry the `heading` column, i.e. the
/// positional index of that heading in the check's `metadata` headings, as the
/// string both metadata maps are keyed by. `None` when the check declares no
/// such column.
fn metadata_column<'a>(check: &'a Check, heading: &str) -> Option<&'a str> {
    check
        .metadata
        .iter()
        .find(|(_, column)| column.as_str() == heading)
        .map(|(key, _)| key.as_str())
}

// ---------------------------------------------------------------------------
// Pagination: an opaque cursor over a map's sorted key order
// ---------------------------------------------------------------------------

/// The page size used when a request names no `maxResults`.
///
/// AWS documents no default for these operations, so this is a choice: 100,
/// which is inside every page-size range the service models (`maxResults` is
/// `1..200` on `ListChecks`/`ListRecommendations` and `1..5000` on
/// `ListRecommendationResources`).
pub const DEFAULT_PAGE_SIZE: usize = 100;

/// One page of a paginated read.
#[derive(Debug)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// `Some` only when entries remain after this page.
    pub next_token: Option<String>,
}

/// A `nextToken` this service did not issue, or issued for a different list.
///
/// Carries the value the request held so the handler that refuses it never has
/// to go back to the request for it: the clause the refusal states echoes the
/// member's value ([`crate::service::validation::page_token_refusal`]), and
/// taking it from the rejection is what keeps the echoed value and the rejected
/// one the same string.
#[derive(Debug)]
pub struct MalformedToken {
    token: String,
}

impl MalformedToken {
    /// The rejected token.
    fn new(token: &str) -> Self {
        Self {
            token: token.to_string(),
        }
    }

    /// The value the request's `nextToken` held, for the refusal to echo.
    pub fn token(&self) -> &str {
        &self.token
    }
}

/// The separator between a cursor's scope and its key. ASCII unit separator: a
/// key this service pages on is a stored id or ARN, which cannot contain it, and
/// a scope is JSON ([`list_scope`]), whose serializer escapes every control
/// character — so the split is unambiguous.
const TOKEN_SEPARATOR: char = '\u{1f}';

/// The cursor scope for one read of a filtered list: the list's name (and, for a
/// sub-resource list, the parent it was taken under) plus every filter in force,
/// in the order the handler lists them.
///
/// The filters belong in the scope because they decide the SEQUENCE the cursor
/// points into: resuming a token issued under one filter set against another
/// would skip to a key that sequence orders differently, so a caller following
/// tokens would not see every entry exactly once. Such a token is refused as
/// malformed instead. `maxResults` is deliberately NOT part of the scope: the
/// page size may change between pages without changing the sequence.
///
/// That refusal holds only while the scope identifies the read UNIQUELY, and a
/// filter value is request text: the model constrains some filters to an enum or
/// a digit string, but leaves others a plain string, or an ARN whose pattern ends
/// in a free-form tail, so a value may carry ANY character — including whatever
/// characters an encoding of its own might delimit on. The scope is therefore
/// built as a JSON document through this crate's serializer rather than by
/// concatenation: the list and the name/value pair of each filter in force are
/// its elements, so one document comes from exactly one list and filter set and
/// no value can spell out the encoding of another read.
pub fn list_scope(list: &str, filters: &[(&str, Option<&str>)]) -> String {
    let in_force: Vec<::serde_json::Value> = filters
        .iter()
        .filter_map(|(name, value)| {
            value.map(|value| {
                ::serde_json::Value::Array(vec![
                    ::serde_json::Value::String((*name).to_string()),
                    ::serde_json::Value::String(value.to_string()),
                ])
            })
        })
        .collect();
    // `Value`'s own rendering, which is infallible: no map key, float or byte
    // string this builds could fail to serialize.
    ::serde_json::Value::Array(vec![
        ::serde_json::Value::String(list.to_string()),
        ::serde_json::Value::Array(in_force),
    ])
    .to_string()
}

/// The cursor for a page of `scope` that ended on `last_key`: base64 of the
/// scope and that key, so the token is opaque to a caller and carries no state
/// of its own.
///
/// `scope` names the read the cursor belongs to (see [`list_scope`]), so a token
/// cannot be replayed on a different list or filter set, where the same key
/// would mean something else.
pub fn encode_token(scope: &str, last_key: &str) -> String {
    ::aws_smithy_types::base64::encode(format!("{scope}{TOKEN_SEPARATOR}{last_key}"))
}

/// The key a cursor points at, or [`MalformedToken`] when the token is not one
/// this service issued for `scope`.
pub fn decode_token(scope: &str, token: &str) -> Result<String, MalformedToken> {
    let bytes =
        ::aws_smithy_types::base64::decode(token).map_err(|_| MalformedToken::new(token))?;
    let decoded = String::from_utf8(bytes).map_err(|_| MalformedToken::new(token))?;
    let (token_scope, key) = decoded
        .split_once(TOKEN_SEPARATOR)
        .ok_or_else(|| MalformedToken::new(token))?;
    if token_scope != scope {
        return Err(MalformedToken::new(token));
    }
    Ok(key.to_string())
}

/// Cut one page out of `entries`, which must be in key order (a `BTreeMap`
/// walk, filtered).
///
/// `next_token` resumes strictly after the key it names, and the returned token
/// is present only when entries remain — so a caller that follows the token
/// until it is absent sees every entry exactly once. Both the token read and the
/// token written carry `scope`, so a cursor is only accepted by the list that
/// issued it.
pub fn paginate<T>(
    scope: &str,
    entries: Vec<(String, T)>,
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Result<Page<T>, MalformedToken> {
    let after = match next_token {
        Some(token) => Some(decode_token(scope, token)?),
        None => None,
    };
    // `maxResults` is already inside its modelled range when a handler runs;
    // the floor of 1 only keeps this helper total.
    let limit = max_results
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .max(1);

    let remaining: Vec<(String, T)> = entries
        .into_iter()
        .filter(|(key, _)| after.as_ref().is_none_or(|resume| key > resume))
        .collect();
    let truncated = remaining.len() > limit;
    let page: Vec<(String, T)> = remaining.into_iter().take(limit).collect();
    let next_token = match (truncated, page.last()) {
        (true, Some((last_key, _))) => Some(encode_token(scope, last_key)),
        _ => None,
    };
    Ok(Page {
        items: page.into_iter().map(|(_, value)| value).collect(),
        next_token,
    })
}

// ---------------------------------------------------------------------------
// The seeded catalog
// ---------------------------------------------------------------------------

/// `2024-01-01T00:00:00Z` — when every seeded recommendation was created.
const SEED_CREATED_AT_MILLIS: i64 = 1_704_067_200_000;

/// One day, so each seeded recommendation gets its own `lastUpdatedAt` and the
/// `afterLastUpdatedAt`/`beforeLastUpdatedAt` filters have something to cut on.
const DAY_MILLIS: i64 = 86_400_000;

/// A seeded resource of the row's recommendation.
struct SeedResource {
    id: &'static str,
    aws_resource_id: &'static str,
    region_code: &'static str,
    status: &'static str,
    /// Positionally aligned with the check's `metadata_headings`.
    metadata: &'static [&'static str],
    exclusion_status: &'static str,
    /// What the account is spending on this resource per month, for a resource
    /// of a cost-optimizing check: the denominator
    /// `estimatedPercentMonthlySavings` is taken against (see
    /// [`ComAmazonawsTrustedadvisorTrustedadvisorState::pillar_aggregates_for`]).
    /// `None` for a resource of a check that does not look at cost.
    ///
    /// Each figure is at least the resource's "Estimated Monthly Savings"
    /// column: a check's saving is what stopping or right-sizing the resource
    /// would recover out of that spend, so it can never exceed it.
    estimated_monthly_spend: Option<f64>,
}

/// One seeded check, the recommendation it produced, and that recommendation's
/// resources.
struct SeedRow {
    check_id: &'static str,
    /// The check's name in [`DEFAULT_LANGUAGE`].
    name: &'static str,
    /// The check's description in [`DEFAULT_LANGUAGE`].
    description: &'static str,
    /// The same text in every OTHER `RecommendationLanguage`, as
    /// `(language, name, description)`. The catalog's text is localized because
    /// `language` selects the copy a response reports; see [`TextCatalog`].
    translations: &'static [(&'static str, &'static str, &'static str)],
    pillars: &'static [&'static str],
    aws_services: &'static [&'static str],
    source: &'static str,
    metadata_headings: &'static [&'static str],
    resource_arn_queryable: bool,
    aws_resource_types: &'static [&'static str],
    check_granularity: &'static str,
    recommendation_type: &'static str,
    recommendation_status: &'static str,
    lifecycle_stage: Option<&'static str>,
    resources: &'static [SeedResource],
}

/// The catalog, spanning both `RecommendationType`s, several pillars, two
/// sources and all three `ResourceStatus` values so every modelled filter has
/// matching and non-matching rows. Deterministic: the same bytes on every boot.
///
/// Each `check_id` is kept short enough that its ARN fits the 64-character
/// `CheckIdentifier` bound `ListRecommendations$checkIdentifier` declares —
/// `arn:aws:trustedadvisor:::check/` is already 31 characters, so an id longer
/// than 33 would produce a check nobody could filter on.
const SEED: &[SeedRow] = &[
    SeedRow {
        check_id: "low-utilization-ec2-instances",
        name: "Low Utilization Amazon EC2 Instances",
        description: "Checks the Amazon EC2 instances that were running at any time during the \
                      last 14 days and alerts you if the daily CPU utilization was 10% or less \
                      and network I/O was 5 MB or less on 4 or more days.",
        translations: &[
            (
                "de",
                "Amazon EC2-Instances mit geringer Auslastung",
                "Prüft die Amazon EC2-Instances, die in den letzten 14 Tagen zu irgendeinem \
                 Zeitpunkt ausgeführt wurden, und warnt Sie, wenn die tägliche CPU-Auslastung an \
                 4 oder mehr Tagen höchstens 10 % und die Netzwerk-E/A höchstens 5 MB betrug.",
            ),
            (
                "es",
                "Instancias de Amazon EC2 con poca utilización",
                "Comprueba las instancias de Amazon EC2 que estuvieron en ejecución en cualquier \
                 momento durante los últimos 14 días y le avisa si la utilización diaria de CPU \
                 fue del 10 % o menos y la E/S de red de 5 MB o menos durante 4 días o más.",
            ),
            (
                "fr",
                "Instances Amazon EC2 sous-utilisées",
                "Vérifie les instances Amazon EC2 qui ont fonctionné à un moment quelconque au \
                 cours des 14 derniers jours et vous alerte si l'utilisation quotidienne du \
                 processeur a été inférieure ou égale à 10 % et les E/S réseau inférieures ou \
                 égales à 5 Mo pendant 4 jours ou plus.",
            ),
            (
                "id",
                "Instans Amazon EC2 dengan Pemanfaatan Rendah",
                "Memeriksa instans Amazon EC2 yang berjalan kapan saja selama 14 hari terakhir \
                 dan memberi tahu Anda jika pemanfaatan CPU harian 10% atau kurang dan I/O \
                 jaringan 5 MB atau kurang selama 4 hari atau lebih.",
            ),
            (
                "it",
                "Istanze Amazon EC2 con utilizzo ridotto",
                "Controlla le istanze Amazon EC2 in esecuzione in qualsiasi momento negli ultimi \
                 14 giorni e invia un avviso se l'utilizzo quotidiano della CPU è stato pari o \
                 inferiore al 10% e l'I/O di rete pari o inferiore a 5 MB per 4 o più giorni.",
            ),
            (
                "ja",
                "低使用率の Amazon EC2 インスタンス",
                "過去 14 日間に稼働していた Amazon EC2 インスタンスを確認し、1 日の CPU 使用率が \
                 10% 以下で、ネットワーク I/O が 5 MB 以下の日が 4 日以上あった場合に通知します。",
            ),
            (
                "ko",
                "사용률이 낮은 Amazon EC2 인스턴스",
                "지난 14일 동안 실행된 Amazon EC2 인스턴스를 확인하고, 일일 CPU 사용률이 10% \
                 이하이고 네트워크 I/O가 5MB 이하인 날이 4일 이상인 경우 알립니다.",
            ),
            (
                "pt_BR",
                "Instâncias do Amazon EC2 com baixa utilização",
                "Verifica as instâncias do Amazon EC2 que estiveram em execução em qualquer \
                 momento nos últimos 14 dias e emite um alerta se a utilização diária da CPU foi \
                 de 10% ou menos e a E/S de rede foi de 5 MB ou menos em 4 dias ou mais.",
            ),
            (
                "zh",
                "低利用率的 Amazon EC2 实例",
                "检查过去 14 天内任何时间运行过的 Amazon EC2 实例，如果其中有 4 天或更多天的每日 \
                 CPU 利用率不超过 10% 且网络 I/O 不超过 5 MB，则向您发出警报。",
            ),
            (
                "zh_TW",
                "使用率偏低的 Amazon EC2 執行個體",
                "檢查過去 14 天內曾經執行的 Amazon EC2 執行個體，若有 4 天以上的每日 CPU 使用率\
                 低於或等於 10% 且網路 I/O 低於或等於 5 MB，則發出警示。",
            ),
        ],
        pillars: &["cost_optimizing"],
        aws_services: &["ec2"],
        source: "ta_check",
        metadata_headings: &[
            "Region",
            "Instance ID",
            "Instance Type",
            "Estimated Monthly Savings",
        ],
        resource_arn_queryable: true,
        aws_resource_types: &["AWS::EC2::Instance"],
        check_granularity: "resource",
        recommendation_type: "priority",
        recommendation_status: "warning",
        lifecycle_stage: Some("pending_response"),
        resources: &[
            SeedResource {
                id: "res-ec2-0001",
                aws_resource_id: "i-0abc1de23f4567890",
                region_code: "us-east-1",
                status: RESOURCE_STATUS_WARNING,
                metadata: &["us-east-1", "i-0abc1de23f4567890", "m5.xlarge", "72.00"],
                exclusion_status: EXCLUSION_INCLUDED,
                estimated_monthly_spend: Some(144.00),
            },
            SeedResource {
                id: "res-ec2-0002",
                aws_resource_id: "i-0abc2de23f4567891",
                region_code: "us-west-2",
                status: RESOURCE_STATUS_OK,
                metadata: &["us-west-2", "i-0abc2de23f4567891", "t3.medium", "0.00"],
                exclusion_status: EXCLUSION_INCLUDED,
                estimated_monthly_spend: Some(36.00),
            },
            SeedResource {
                id: "res-ec2-0003",
                aws_resource_id: "i-0abc3de23f4567892",
                region_code: "eu-west-1",
                status: RESOURCE_STATUS_WARNING,
                metadata: &["eu-west-1", "i-0abc3de23f4567892", "c5.2xlarge", "148.00"],
                exclusion_status: EXCLUSION_INCLUDED,
                estimated_monthly_spend: Some(260.00),
            },
        ],
    },
    SeedRow {
        check_id: "amazon-ebs-snapshots",
        name: "Amazon EBS Snapshots",
        description: "Checks the age of the snapshots for your Amazon Elastic Block Store \
                      (Amazon EBS) volumes (available or in-use).",
        translations: &[
            (
                "de",
                "Amazon EBS-Snapshots",
                "Prüft das Alter der Snapshots Ihrer Amazon Elastic Block Store \
                 (Amazon EBS)-Volumes (verfügbar oder in Verwendung).",
            ),
            (
                "es",
                "Instantáneas de Amazon EBS",
                "Comprueba la antigüedad de las instantáneas de sus volúmenes de Amazon Elastic \
                 Block Store (Amazon EBS) (disponibles o en uso).",
            ),
            (
                "fr",
                "Instantanés Amazon EBS",
                "Vérifie l'ancienneté des instantanés de vos volumes Amazon Elastic Block Store \
                 (Amazon EBS) (disponibles ou en cours d'utilisation).",
            ),
            (
                "id",
                "Snapshot Amazon EBS",
                "Memeriksa usia snapshot volume Amazon Elastic Block Store (Amazon EBS) Anda \
                 (tersedia atau sedang digunakan).",
            ),
            (
                "it",
                "Snapshot Amazon EBS",
                "Controlla l'età degli snapshot dei volumi Amazon Elastic Block Store \
                 (Amazon EBS) (disponibili o in uso).",
            ),
            (
                "ja",
                "Amazon EBS スナップショット",
                "Amazon Elastic Block Store (Amazon EBS) ボリューム (available または in-use) の\
                 スナップショットの経過日数を確認します。",
            ),
            (
                "ko",
                "Amazon EBS 스냅샷",
                "Amazon Elastic Block Store(Amazon EBS) 볼륨(available 또는 in-use)의 스냅샷 \
                 생성 경과 기간을 확인합니다.",
            ),
            (
                "pt_BR",
                "Snapshots do Amazon EBS",
                "Verifica a idade dos snapshots dos volumes do Amazon Elastic Block Store \
                 (Amazon EBS) (disponíveis ou em uso).",
            ),
            (
                "zh",
                "Amazon EBS 快照",
                "检查您的 Amazon Elastic Block Store (Amazon EBS) 卷（available 或 in-use）的\
                 快照的创建时间。",
            ),
            (
                "zh_TW",
                "Amazon EBS 快照",
                "檢查您的 Amazon Elastic Block Store (Amazon EBS) 磁碟區 (available 或 in-use) \
                 快照的建立時間。",
            ),
        ],
        pillars: &["fault_tolerance"],
        aws_services: &["ebs"],
        source: "ta_check",
        metadata_headings: &["Region", "Volume ID", "Snapshot ID", "Snapshot Age"],
        resource_arn_queryable: true,
        aws_resource_types: &["AWS::EC2::Volume"],
        check_granularity: "resource",
        recommendation_type: "standard",
        recommendation_status: "error",
        lifecycle_stage: None,
        resources: &[
            SeedResource {
                id: "res-ebs-0001",
                aws_resource_id: "vol-0ea1b2c3d4e5f6789",
                region_code: "us-east-1",
                status: RESOURCE_STATUS_ERROR,
                metadata: &[
                    "us-east-1",
                    "vol-0ea1b2c3d4e5f6789",
                    "snap-0ea1b2c3d4e5f6789",
                    "412",
                ],
                exclusion_status: EXCLUSION_INCLUDED,
                // Fault tolerance, not cost: nothing to take a percentage of.
                estimated_monthly_spend: None,
            },
            SeedResource {
                id: "res-ebs-0002",
                aws_resource_id: "vol-0ea2b2c3d4e5f6789",
                region_code: "ap-southeast-1",
                status: RESOURCE_STATUS_OK,
                metadata: &[
                    "ap-southeast-1",
                    "vol-0ea2b2c3d4e5f6789",
                    "snap-0ea2b2c3d4e5f6789",
                    "11",
                ],
                // Seeded excluded, so the `exclusionStatus` filter and
                // `excludedCount` have a row from the first read.
                exclusion_status: EXCLUSION_EXCLUDED,
                estimated_monthly_spend: None,
            },
        ],
    },
    SeedRow {
        check_id: "iam-access-key-rotation",
        name: "IAM Access Key Rotation",
        description: "Checks for active IAM access keys that have not been rotated in the last \
                      90 days.",
        translations: &[
            (
                "de",
                "Rotation von IAM-Zugriffsschlüsseln",
                "Prüft, ob aktive IAM-Zugriffsschlüssel in den letzten 90 Tagen nicht rotiert \
                 wurden.",
            ),
            (
                "es",
                "Rotación de claves de acceso de IAM",
                "Busca claves de acceso de IAM activas que no se hayan rotado en los últimos 90 \
                 días.",
            ),
            (
                "fr",
                "Rotation des clés d'accès IAM",
                "Recherche les clés d'accès IAM actives qui n'ont pas fait l'objet d'une rotation \
                 au cours des 90 derniers jours.",
            ),
            (
                "id",
                "Rotasi Kunci Akses IAM",
                "Memeriksa kunci akses IAM aktif yang belum dirotasi dalam 90 hari terakhir.",
            ),
            (
                "it",
                "Rotazione delle chiavi di accesso IAM",
                "Verifica la presenza di chiavi di accesso IAM attive che non sono state ruotate \
                 negli ultimi 90 giorni.",
            ),
            (
                "ja",
                "IAM アクセスキーのローテーション",
                "過去 90 日間にローテーションされていないアクティブな IAM アクセスキーを\
                 確認します。",
            ),
            (
                "ko",
                "IAM 액세스 키 교체",
                "지난 90일 동안 교체되지 않은 활성 IAM 액세스 키를 확인합니다.",
            ),
            (
                "pt_BR",
                "Rotação de chaves de acesso do IAM",
                "Verifica se há chaves de acesso do IAM ativas que não foram alternadas nos \
                 últimos 90 dias.",
            ),
            (
                "zh",
                "IAM 访问密钥轮换",
                "检查在过去 90 天内未轮换的活动 IAM 访问密钥。",
            ),
            (
                "zh_TW",
                "IAM 存取金鑰輪換",
                "檢查在過去 90 天內未輪換的作用中 IAM 存取金鑰。",
            ),
        ],
        pillars: &["security"],
        aws_services: &["iam"],
        source: "ta_check",
        metadata_headings: &["Region", "IAM User", "Access Key ID", "Key Age In Days"],
        resource_arn_queryable: false,
        aws_resource_types: &[],
        check_granularity: "account",
        recommendation_type: "standard",
        recommendation_status: "ok",
        lifecycle_stage: None,
        resources: &[SeedResource {
            id: "res-iam-0001",
            aws_resource_id: "AKIAIOSFODNN7EXAMPLE",
            region_code: "us-east-1",
            status: RESOURCE_STATUS_OK,
            metadata: &["us-east-1", "build-agent", "AKIAIOSFODNN7EXAMPLE", "14"],
            exclusion_status: EXCLUSION_INCLUDED,
            estimated_monthly_spend: None,
        }],
    },
    SeedRow {
        check_id: "rds-idle-db-instances",
        name: "Amazon RDS Idle DB Instances",
        description: "Checks the configuration of your Amazon Relational Database Service \
                      (Amazon RDS) for any database (DB) instances that appear to be idle.",
        translations: &[
            (
                "de",
                "Ungenutzte Amazon RDS-DB-Instances",
                "Prüft die Konfiguration Ihres Amazon Relational Database Service (Amazon RDS) \
                 auf Datenbank-Instances (DB), die ungenutzt zu sein scheinen.",
            ),
            (
                "es",
                "Instancias de base de datos de Amazon RDS inactivas",
                "Comprueba la configuración de Amazon Relational Database Service (Amazon RDS) \
                 para detectar instancias de base de datos (BD) que parecen estar inactivas.",
            ),
            (
                "fr",
                "Instances de base de données Amazon RDS inactives",
                "Vérifie la configuration de votre Amazon Relational Database Service \
                 (Amazon RDS) à la recherche d'instances de base de données (BD) qui semblent \
                 inactives.",
            ),
            (
                "id",
                "Instans Basis Data Amazon RDS yang Menganggur",
                "Memeriksa konfigurasi Amazon Relational Database Service (Amazon RDS) Anda untuk \
                 menemukan instans basis data (DB) yang tampak menganggur.",
            ),
            (
                "it",
                "Istanze database Amazon RDS inattive",
                "Controlla la configurazione di Amazon Relational Database Service (Amazon RDS) \
                 per individuare istanze database (DB) che sembrano inattive.",
            ),
            (
                "ja",
                "アイドル状態の Amazon RDS DB インスタンス",
                "Amazon Relational Database Service (Amazon RDS) の設定を確認し、アイドル状態と\
                 思われるデータベース (DB) インスタンスを検出します。",
            ),
            (
                "ko",
                "유휴 Amazon RDS DB 인스턴스",
                "Amazon Relational Database Service(Amazon RDS) 구성에서 유휴 상태로 보이는 \
                 데이터베이스(DB) 인스턴스를 확인합니다.",
            ),
            (
                "pt_BR",
                "Instâncias de banco de dados do Amazon RDS inativas",
                "Verifica a configuração do Amazon Relational Database Service (Amazon RDS) para \
                 identificar instâncias de banco de dados (BD) que parecem estar inativas.",
            ),
            (
                "zh",
                "空闲的 Amazon RDS 数据库实例",
                "检查您的 Amazon Relational Database Service (Amazon RDS) 配置，找出似乎处于\
                 空闲状态的数据库 (DB) 实例。",
            ),
            (
                "zh_TW",
                "閒置的 Amazon RDS 資料庫執行個體",
                "檢查您的 Amazon Relational Database Service (Amazon RDS) 設定，找出似乎處於\
                 閒置狀態的資料庫 (DB) 執行個體。",
            ),
        ],
        pillars: &["cost_optimizing", "performance"],
        aws_services: &["rds"],
        source: "rds",
        metadata_headings: &[
            "Region",
            "DB Instance Name",
            "Instance Type",
            "Days Since Last Connection",
            "Estimated Monthly Savings",
        ],
        resource_arn_queryable: true,
        aws_resource_types: &["AWS::RDS::DBInstance"],
        check_granularity: "resource",
        recommendation_type: "priority",
        recommendation_status: "warning",
        lifecycle_stage: Some("in_progress"),
        resources: &[
            SeedResource {
                id: "res-rds-0001",
                aws_resource_id: "reporting-prod",
                region_code: "us-east-1",
                status: RESOURCE_STATUS_WARNING,
                metadata: &["us-east-1", "reporting-prod", "db.m5.large", "21", "131.40"],
                exclusion_status: EXCLUSION_INCLUDED,
                estimated_monthly_spend: Some(146.00),
            },
            SeedResource {
                id: "res-rds-0002",
                aws_resource_id: "analytics-staging",
                region_code: "us-west-2",
                status: RESOURCE_STATUS_OK,
                metadata: &["us-west-2", "analytics-staging", "db.t3.medium", "1", "0.00"],
                exclusion_status: EXCLUSION_INCLUDED,
                estimated_monthly_spend: Some(72.00),
            },
        ],
    },
];

/// `{"0": first, "1": second, ...}` — the positional string map both
/// `CheckSummary$metadata` (column headings) and
/// `RecommendationResourceSummary$metadata` (that resource's values) use.
fn positional_map(values: &[&str]) -> BTreeMap<String, String> {
    values
        .iter()
        .enumerate()
        .map(|(index, value)| (index.to_string(), (*value).to_string()))
        .collect()
}

fn owned(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

impl Default for ComAmazonawsTrustedadvisorTrustedadvisorState {
    /// The seeded store. `engine.state::<T>()` builds this once, on the first
    /// request that reaches the service, and every later request sees the same
    /// value — including any lifecycle or exclusion change a caller made.
    fn default() -> Self {
        let mut state = Self {
            checks: BTreeMap::new(),
            recommendations: BTreeMap::new(),
            recommendation_resources: BTreeMap::new(),
            organization_recommendations: BTreeMap::new(),
            support_plan: seeded_support_plan(),
        };
        for (index, row) in SEED.iter().enumerate() {
            let recommendation_id = format!("{}-recommendation", row.check_id);
            let check_arn = check_arn(row.check_id);
            let recommendation_arn = recommendation_arn(&recommendation_id);
            // One day apart per row, so each recommendation has its own
            // `lastUpdatedAt`.
            let last_updated_at_millis =
                SEED_CREATED_AT_MILLIS + DAY_MILLIS * (index as i64 + 1);

            // ONE catalog of localized text per row, shared by the check and
            // the recommendation it produced, so the two can never report
            // different text for the same language.
            let text = TextCatalog::new(
                LocalizedText {
                    name: row.name.to_string(),
                    description: row.description.to_string(),
                },
                row.translations.iter().copied(),
            );

            state.checks.insert(
                row.check_id.to_string(),
                Check {
                    id: row.check_id.to_string(),
                    arn: check_arn.clone(),
                    text: text.clone(),
                    pillars: owned(row.pillars),
                    aws_services: owned(row.aws_services),
                    source: row.source.to_string(),
                    metadata: positional_map(row.metadata_headings),
                    resource_arn_queryable: row.resource_arn_queryable,
                    aws_resource_types: owned(row.aws_resource_types),
                    check_granularity: Some(row.check_granularity.to_string()),
                    recommendation_id: Some(recommendation_id.clone()),
                },
            );

            state.recommendations.insert(
                recommendation_arn.clone(),
                RecommendationEntry {
                    id: recommendation_id.clone(),
                    arn: recommendation_arn.clone(),
                    text: text.clone(),
                    check_arn: Some(check_arn.clone()),
                    r#type: row.recommendation_type.to_string(),
                    status: row.recommendation_status.to_string(),
                    lifecycle_stage: row.lifecycle_stage.map(str::to_string),
                    pillars: owned(row.pillars),
                    source: row.source.to_string(),
                    aws_services: owned(row.aws_services),
                    created_at_millis: SEED_CREATED_AT_MILLIS,
                    last_updated_at_millis,
                    resolved_at_millis: None,
                    update_reason: None,
                    update_reason_code: None,
                },
            );

            // The organization-level view of the same check, aggregating the
            // member accounts' recommendations for that check (here the
            // emulator's one account). Its classification members are the
            // check's, so the two views of one finding cannot disagree.
            //
            // Only a PRIORITIZED recommendation has one: every organization
            // operation of this service supports only prioritized
            // recommendations, so a check whose recommendation was produced
            // automatically contributes nothing the organization surface can
            // report, resolve or aggregate — and the organization recommendation
            // it would have had is not a resource of this API.
            if row.recommendation_type == RECOMMENDATION_TYPE_PRIORITY {
                let organization_recommendation_id =
                    format!("{}-organization-recommendation", row.check_id);
                let organization_arn =
                    organization_recommendation_arn(&organization_recommendation_id);
                state.organization_recommendations.insert(
                    organization_arn.clone(),
                    OrganizationRecommendationEntry {
                        id: organization_recommendation_id,
                        arn: organization_arn,
                        text,
                        check_arn: Some(check_arn),
                        r#type: row.recommendation_type.to_string(),
                        status: row.recommendation_status.to_string(),
                        lifecycle_stage: row.lifecycle_stage.map(str::to_string),
                        pillars: owned(row.pillars),
                        source: row.source.to_string(),
                        aws_services: owned(row.aws_services),
                        member_recommendation_arns: vec![recommendation_arn.clone()],
                        created_at_millis: SEED_CREATED_AT_MILLIS,
                        last_updated_at_millis,
                        resolved_at_millis: None,
                        update_reason: None,
                        update_reason_code: None,
                    },
                );
            }

            for resource in row.resources {
                let arn = recommendation_resource_arn(&recommendation_id, resource.id);
                state.recommendation_resources.insert(
                    arn.clone(),
                    ResourceEntry {
                        id: resource.id.to_string(),
                        arn,
                        recommendation_arn: recommendation_arn.clone(),
                        aws_resource_id: resource.aws_resource_id.to_string(),
                        region_code: resource.region_code.to_string(),
                        status: resource.status.to_string(),
                        metadata: positional_map(resource.metadata),
                        last_updated_at_millis,
                        exclusion_status: resource.exclusion_status.to_string(),
                        estimated_monthly_spend: resource.estimated_monthly_spend,
                    },
                );
            }
        }
        state
    }
}

/// The support-level gate: `Some(..)` when the calling account's support plan
/// does not include the Trusted Advisor API, so the request must be refused with
/// the modelled `AccessDeniedException` (HTTP 403) instead of being served.
///
/// Every operation applies this BEFORE it resolves an identifier and before it
/// writes anything, so an unentitled caller learns it lacks access rather than
/// whether a recommendation exists, and a mutation it may not make changes
/// nothing. Input validation is refused earlier still, so a malformed request is
/// a `ValidationException` whether the caller is entitled or not: the modelled
/// constraints by the centralized request walk, and a well-formedness condition
/// those traits cannot express by the handler that owns it, ahead of this gate.
///
/// The emulator presents exactly one account (`engine::EMULATOR_ACCOUNT_ID`), so
/// that is the account every request is made from and the only one the gate can
/// be asked about. A store that cannot be reached answers `None` — unknown is
/// not "denied", and the handler's own store check turns it into the modelled
/// server error.
pub fn support_level_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<AccessDeniedException> {
    let state = service_state(engine)?;
    let entitled = state.lock().support_plan().includes_trusted_advisor_api();
    (!entitled).then(|| AccessDeniedException {
        message: ACCESS_DENIED_SUPPORT_LEVEL.to_string(),
    })
}

/// The account segment of an AWS resource ARN
/// (`arn:<partition>:<service>:<region>:<account>:<resource>`), or `None` for a
/// string with fewer segments than that. The segment is EMPTY for a resource type
/// whose ARNs carry no account (an S3 bucket, a Trusted Advisor check), which is
/// not another account's resource.
pub fn arn_account(arn: &str) -> Option<&str> {
    arn.split(':').nth(4)
}

/// The owning account a modelled `accountId` member reports for a resource,
/// derived from that resource's own ARN: its account segment, owned, and `None`
/// when the ARN names no account at all — it carries fewer segments than an ARN,
/// or its account segment is EMPTY, which (see [`arn_account`]) is what the
/// segment is for a resource type whose ARNs carry no account.
///
/// Every view that reports a stored resource's owning account goes through this,
/// so the account-level and the organization-level view of one stored resource
/// cannot report its account differently.
pub fn arn_account_id(arn: &str) -> Option<String> {
    arn_account(arn)
        .filter(|account| !account.is_empty())
        .map(str::to_string)
}

/// Whether `arn` names a resource owned by an account OTHER than the calling
/// one: the account segment of the resource's OWN ARN ([`arn_account_id`])
/// differs from the account the request is made from. An ARN that names no
/// account at all (see [`arn_account`]) is not another account's resource.
///
/// The ONE test every account-level view of a stored resource makes, so a
/// listing and a refusal cannot disagree about who owns one stored resource:
///
/// * an account-level LISTING reports a row only when this answers `false` —
///   such a list reports the calling account's own resources, and the
///   cross-account view of the same resources is the organization surface;
/// * an operation whose INPUT names such a resource answers for it first, with
///   the refusal [`foreign_resource_refusal`] builds — an arbitrary AWS resource
///   ARN and one of this service's own account-level identifiers alike.
pub fn is_foreign_resource(arn: &str) -> bool {
    arn_account_id(arn).is_some_and(|account| account != ::engine::emulator_account_id())
}

/// The resource-authorization refusal: `Some(..)` when `arn` names a resource in
/// an account other than the caller's ([`is_foreign_resource`]), so the request is
/// refused for THAT resource — 403 `AccessDeniedException`, whatever the caller's
/// support plan, because no support plan grants an account access to another
/// account's resources.
///
/// `arn` is ANY ARN a request names a resource by: an arbitrary AWS resource
/// (`ListRecommendationsForResource`) as much as one of this service's own
/// account-level identifiers, whose account segment the caller writes
/// (`AccountRecommendationIdentifier`, `RecommendationResourceArn`). Every
/// operation that takes one applies this before it resolves the identifier and
/// before it writes anything, so a request for another account's resource learns
/// only that it was denied.
///
/// The message is built from the request: it names the resource that was refused,
/// which is the only thing the service knows about the refusal and the only way a
/// caller passing several ARNs learns which one was denied.
pub fn foreign_resource_refusal(arn: &str) -> Option<AccessDeniedException> {
    is_foreign_resource(arn).then(|| AccessDeniedException {
        message: format!("Access denied to resource: {arn}"),
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsTrustedadvisorTrustedadvisorState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsTrustedadvisorTrustedadvisorState>)
}
