// SPDX-License-Identifier: CC0-1.0

//! The campaign records this service stores, and every projection a read reports one
//! through.
//!
//! A campaign's `State` and its `ActivationIssues` are DERIVED here on every read
//! rather than stored: `CampaignState` is "a summary of the assets, not a lifecycle:
//! nothing transitions it, it is never stored".

use std::collections::BTreeMap;
use std::time::SystemTime;

use super::*;
use crate::ops::types::{
    ActivationIssue, Campaign, CampaignSort, CampaignSummary, EmailTargetDetail, FileRef,
    MonetaryValue, Tag, TargetAudience,
};

/// `CampaignState` of a campaign whose UTC date has passed its `EndOn` — a member of
/// [`crate::ops::types::CAMPAIGN_STATE_VALUES`].
pub const CAMPAIGN_STATE_COMPLETED: &str = "COMPLETED";

/// `CampaignState` of a campaign that is not completed and has at least one live asset.
pub const CAMPAIGN_STATE_ACTIVE: &str = "ACTIVE";

/// `CampaignState` of a campaign that is not completed and has no live asset, including
/// one with no assets at all.
pub const CAMPAIGN_STATE_DRAFT: &str = "DRAFT";

/// `CampaignAssetState` of a live asset — a member of
/// [`crate::ops::types::CAMPAIGN_ASSET_STATE_VALUES`].
pub const CAMPAIGN_ASSET_STATE_ACTIVE: &str = "ACTIVE";

/// `SortOrder` reversing the attribute's own order — a member of
/// [`crate::ops::types::SORT_ORDER_VALUES`].
const SORT_ORDER_DESCENDING: &str = "DESCENDING";

/// The `CampaignSortName` members, as `ListCampaigns$Sort` names them.
const SORT_BY_NAME: &str = "NAME";
const SORT_BY_STATE: &str = "STATE";
const SORT_BY_START_ON: &str = "START_ON";

/// A stored campaign: every member a read of one reports, plus the assets its `State`
/// and its `ActivationIssues` are derived from.
#[derive(Debug, Clone)]
pub struct CampaignRecord {
    /// The minted `CampaignId`.
    pub id: String,
    /// `Arn`, built once at create time from that request's region and kept on the
    /// record, so every later read reports the ARN the create reported.
    pub arn: String,
    /// The catalog the campaign lives in; part of its ARN and of its store key.
    pub catalog: String,
    pub name: String,
    /// `CampaignTemplateId` — "Set at create, immutable", so no update writes it.
    pub template_id: String,
    /// `Language` — the template's `LanguageCode`, read once at create time, as
    /// `Campaign$Language` documents ("its template's `LanguageCode`, set at create").
    /// Absent when the template names no language.
    pub language: Option<String>,
    pub sender_email: Option<String>,
    pub privacy_policy_url: Option<String>,
    pub partner_address: Option<String>,
    /// The badge reference the caller staged. Persisted so the badge slice has one
    /// home; no read reports `PartnerBadge` yet, because `PartnerBadgeDetail$Url` is a
    /// location this slice serves no bytes from.
    pub partner_badge: Option<FileRef>,
    /// The recipient-list reference the caller staged, with the instant it was
    /// attached. A read reports the metadata this reference itself carries (the file's
    /// display name and that instant); the scan verdict and the address counts need the
    /// file's bytes, which this slice does not read, so they stay absent.
    pub email_target: Option<FileRef>,
    pub email_target_attached_at: Option<SystemTime>,
    pub email_target_engagement_group_ids: Vec<String>,
    /// `SocialConnections` — one connection list per platform, as set by
    /// `UpdateCampaign`. A `BTreeMap` so the map a read reports is in a stable order.
    pub social_connections: BTreeMap<String, Vec<String>>,
    pub estimated_budget: Option<MonetaryValue>,
    pub total_budget: Option<MonetaryValue>,
    pub target_audience: Option<TargetAudience>,
    /// `StartOn` / `EndOn` as the `UtcDate` the request carried: `YYYY-MM-DD`, which
    /// orders chronologically as a string, so the comparisons below need no calendar.
    pub start_on: Option<String>,
    pub end_on: Option<String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The tags applied at create. Removed with the campaign, as `DeleteCampaign`
    /// documents ("Its tags are removed with it").
    pub tags: Vec<Tag>,
    /// The campaign's assets. Empty in this slice — no operation creates one yet — and
    /// the field is what `State` and `ActivationIssues` are derived from, so both
    /// follow the assets rather than a stored status.
    pub assets: Vec<CampaignAssetRecord>,
}

/// A stored campaign asset, reduced to what a read of the CAMPAIGN derives from it.
#[derive(Debug, Clone)]
pub struct CampaignAssetRecord {
    /// The asset's own id, which an activation issue names in `AffectedAssetIds`.
    pub id: String,
    /// `CampaignAssetState`, one of [`crate::ops::types::CAMPAIGN_ASSET_STATE_VALUES`]:
    /// a campaign is `ACTIVE` while any of its assets is.
    pub state: String,
    /// The `CampaignSetting`s this asset's blueprint fields draw from the campaign —
    /// `CampaignAssetTemplateField$CampaignSetting` — so an unset one of them is an
    /// activation issue naming this asset.
    pub required_campaign_settings: Vec<String>,
}

impl CampaignRecord {
    /// Whether any of this campaign's assets is live — what makes the campaign `ACTIVE`
    /// and what `DeleteCampaign` refuses on.
    pub fn has_live_asset(&self) -> bool {
        self.assets
            .iter()
            .any(|asset| asset.state == CAMPAIGN_ASSET_STATE_ACTIVE)
    }
}

/// The campaign's `State`, derived from its `EndOn` and its assets as `CampaignState`
/// documents, with `today` the current UTC calendar date:
///
/// 1. `COMPLETED` once the UTC date is after `EndOn` (a campaign with no `EndOn` never
///    completes);
/// 2. otherwise `ACTIVE` when any of its assets is `ACTIVE`;
/// 3. otherwise `DRAFT`, including a campaign with no assets.
pub fn campaign_state(record: &CampaignRecord, today: &str) -> &'static str {
    if record
        .end_on
        .as_deref()
        .is_some_and(|end_on| today > end_on)
    {
        return CAMPAIGN_STATE_COMPLETED;
    }
    if record.has_live_asset() {
        return CAMPAIGN_STATE_ACTIVE;
    }
    CAMPAIGN_STATE_DRAFT
}

/// One campaign setting an activation issue can be reported for: the `CampaignSetting`
/// to set, the `ActivationIssue$Code` that names it missing, whether the campaign has
/// it, and the sentence the issue is displayed with.
struct SettingRule {
    setting: &'static str,
    code: &'static str,
    is_set: fn(&CampaignRecord) -> bool,
    message: &'static str,
}

/// The campaign settings an asset can be blocked on, as
/// `CampaignSetting` names them beside the `CampaignAssetStateReason` for each.
///
/// `PARTNER_BADGE` is deliberately not here: a campaign with no badge is not blocked —
/// "until one is set, every badge slot shows the Standalone AWS Partner badge".
const SETTING_RULES: &[SettingRule] = &[
    SettingRule {
        setting: "SENDER_EMAIL",
        code: "SENDER_EMAIL_MISSING",
        is_set: |record| record.sender_email.is_some(),
        message: "Set SenderEmail with UpdateCampaign before activating these assets.",
    },
    SettingRule {
        setting: "PRIVACY_POLICY_URL",
        code: "PRIVACY_POLICY_URL_MISSING",
        is_set: |record| record.privacy_policy_url.is_some(),
        message: "Set PrivacyPolicyUrl with UpdateCampaign before activating these assets.",
    },
    SettingRule {
        setting: "PARTNER_ADDRESS",
        code: "PARTNER_ADDRESS_MISSING",
        is_set: |record| record.partner_address.is_some(),
        message: "Set PartnerAddress with UpdateCampaign before activating these assets.",
    },
    SettingRule {
        setting: "END_ON",
        code: "END_ON_MISSING",
        is_set: |record| record.end_on.is_some(),
        message: "Set EndOn with UpdateCampaign before activating these assets.",
    },
    SettingRule {
        setting: "EMAIL_TARGET",
        code: "RECIPIENT_LIST_MISSING",
        is_set: |record| {
            record.email_target.is_some() || !record.email_target_engagement_group_ids.is_empty()
        },
        message: "Attach EmailTarget or name EmailTargetEngagementGroupIds with UpdateCampaign \
                  before activating these assets.",
    },
];

/// The campaign's `ActivationIssues`, derived on every read: one entry per campaign
/// setting that is unset AND that an asset of this campaign draws on, naming the assets
/// it blocks.
///
/// Empty when nothing is missing — and empty for a campaign with no assets, because
/// each issue reports "the assets this campaign-level issue blocks" and there are none.
pub fn activation_issues(record: &CampaignRecord) -> Vec<ActivationIssue> {
    SETTING_RULES
        .iter()
        .filter(|rule| !(rule.is_set)(record))
        .filter_map(|rule| {
            let affected: Vec<String> = record
                .assets
                .iter()
                .filter(|asset| {
                    asset
                        .required_campaign_settings
                        .iter()
                        .any(|setting| setting == rule.setting)
                })
                .map(|asset| asset.id.clone())
                .collect();
            (!affected.is_empty()).then(|| ActivationIssue {
                code: rule.code.to_string(),
                field_id: None,
                campaign_setting: Some(rule.setting.to_string()),
                partner_profile_field: None,
                affected_asset_ids: Some(affected),
                message: rule.message.to_string(),
            })
        })
        .collect()
}

/// The stored campaign as the wire `Campaign` shape, member for member, with `State`
/// and `ActivationIssues` derived for the reading at `today`.
pub fn campaign_shape(record: &CampaignRecord, today: &str) -> Campaign {
    Campaign {
        campaign_id: record.id.clone(),
        arn: record.arn.clone(),
        name: record.name.clone(),
        state: campaign_state(record, today).to_string(),
        campaign_template_id: Some(record.template_id.clone()),
        language: record.language.clone(),
        // `PartnerProfile` is the partner's branding as AWS Partner Central holds it —
        // another service's record, not this one's — so nothing is reported for it.
        partner_profile: None,
        sender_email: record.sender_email.clone(),
        privacy_policy_url: record.privacy_policy_url.clone(),
        partner_address: record.partner_address.clone(),
        partner_badge: None,
        email_target: email_target_shape(record),
        email_target_engagement_group_ids: (!record.email_target_engagement_group_ids.is_empty())
            .then(|| record.email_target_engagement_group_ids.clone()),
        social_connections: (!record.social_connections.is_empty()).then(|| {
            record
                .social_connections
                .iter()
                .map(|(platform, connections)| (platform.clone(), connections.clone()))
                .collect()
        }),
        estimated_budget: record.estimated_budget.clone(),
        total_budget: record.total_budget.clone(),
        target_audience: record.target_audience.clone(),
        start_on: record.start_on.clone(),
        end_on: record.end_on.clone(),
        created_at: Some(timestamp(record.created_at)),
        updated_at: Some(timestamp(record.updated_at)),
        activation_issues: activation_issues(record),
    }
}

/// The stored campaign as the wire `CampaignSummary` shape — the subset a list reports,
/// with `State` derived the same way a full read derives it.
pub fn campaign_summary_shape(record: &CampaignRecord, today: &str) -> CampaignSummary {
    CampaignSummary {
        campaign_id: record.id.clone(),
        arn: record.arn.clone(),
        name: record.name.clone(),
        state: campaign_state(record, today).to_string(),
        language: record.language.clone(),
        start_on: record.start_on.clone(),
        end_on: record.end_on.clone(),
        created_at: Some(timestamp(record.created_at)),
        updated_at: Some(timestamp(record.updated_at)),
    }
}

/// The attached recipient list as `EmailTargetDetail` reports it, or `None` when none is
/// attached.
///
/// `FileName` is "the display name of the uploaded file, as the partner named it, with
/// no path", which is the last segment of the stored reference's key.
fn email_target_shape(record: &CampaignRecord) -> Option<EmailTargetDetail> {
    let file = record.email_target.as_ref()?;
    let file_name = ::service_common::s3_uri::object_address(&file.s3_uri)
        .and_then(|(_bucket, key)| key.rsplit('/').next().map(str::to_string))
        .filter(|name| !name.is_empty());
    Some(EmailTargetDetail {
        // Absent: the scan verdict and the address counts are read off the file's
        // bytes, which this slice does not read.
        scan_status: None,
        attached_at: record.email_target_attached_at.map(timestamp),
        valid_count: None,
        rejected_count: None,
        duplicate_count: None,
        file_name,
    })
}

/// A campaign as a list reports it: the stored record and the `State` derived for this
/// reading, which is both a filter and a sort key.
#[derive(Debug, Clone, Copy)]
pub struct ListedCampaign<'a> {
    pub record: &'a CampaignRecord,
    pub state: &'static str,
}

/// Whether this campaign passes every `ListCampaigns` filter the request named.
///
/// The filters combine with AND, as `ListCampaignsRequest` documents; each one is the
/// rule its own member documents — `Name` "contains this, compared case-insensitively",
/// `States` "in one of these states", and `CampaignIdentifiers` the campaigns those
/// identifiers name, where one naming nothing of this catalog is left out.
pub fn passes_filters(
    listed: &ListedCampaign<'_>,
    request: &crate::ops::types::ListCampaignsInput,
) -> bool {
    if let Some(identifiers) = request.campaign_identifiers.as_ref() {
        let named = identifiers
            .iter()
            .filter_map(|identifier| campaign_id_of(&listed.record.catalog, identifier))
            .any(|campaign_id| campaign_id == listed.record.id);
        if !named {
            return false;
        }
    }
    if let Some(name) = request.name.as_deref() {
        if !listed
            .record
            .name
            .to_lowercase()
            .contains(&name.to_lowercase())
        {
            return false;
        }
    }
    if let Some(states) = request.states.as_ref() {
        if !states.iter().any(|state| state == listed.state) {
            return false;
        }
    }
    true
}

/// Order `listed` as `ListCampaigns$Sort` asks, defaulting to `CREATED_AT`,
/// `DESCENDING` when the request names no sort.
///
/// Ties break by `CampaignId` ascending in either direction, as `CampaignSort`
/// documents, which is also what makes a page resumable: the order is total.
pub fn sort_campaigns(listed: &mut [ListedCampaign<'_>], sort: Option<&CampaignSort>) {
    let (by, order) = match sort {
        Some(sort) => (sort.sort_by.as_str(), sort.sort_order.as_str()),
        // The documented default.
        None => ("CREATED_AT", SORT_ORDER_DESCENDING),
    };
    let descending = order == SORT_ORDER_DESCENDING;
    listed.sort_by(|left, right| {
        let primary = match by {
            SORT_BY_NAME => left
                .record
                .name
                .to_lowercase()
                .cmp(&right.record.name.to_lowercase()),
            SORT_BY_STATE => state_rank(left.state).cmp(&state_rank(right.state)),
            SORT_BY_START_ON => {
                // "A campaign with none sorts last in either direction", so the
                // presence of a `StartOn` is compared before the direction is applied
                // and never reversed with it.
                let present = right
                    .record
                    .start_on
                    .is_some()
                    .cmp(&left.record.start_on.is_some());
                if present != std::cmp::Ordering::Equal {
                    return present.then_with(|| left.record.id.cmp(&right.record.id));
                }
                left.record.start_on.cmp(&right.record.start_on)
            }
            // `CREATED_AT`, and any value a later model adds: the documented default
            // attribute.
            _ => left.record.created_at.cmp(&right.record.created_at),
        };
        let directed = if descending { primary.reverse() } else { primary };
        directed.then_with(|| left.record.id.cmp(&right.record.id))
    });
}

/// Where a state falls in the lifecycle order `CampaignSortName$STATE` sorts by:
/// "`DRAFT`, `ACTIVE`, `COMPLETED`".
fn state_rank(state: &str) -> u8 {
    match state {
        CAMPAIGN_STATE_DRAFT => 0,
        CAMPAIGN_STATE_ACTIVE => 1,
        _ => 2,
    }
}
