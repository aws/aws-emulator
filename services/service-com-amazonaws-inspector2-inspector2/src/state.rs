// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-inspector2-inspector2 service.
//!
//! Six resources live here, and every implemented operation computes its answer from
//! them:
//!
//! * `accounts` — per-account Amazon Inspector enablement, written by `Enable` and read
//!   back by `BatchGetAccountStatus`, plus that account's EC2 deep inspection
//!   configuration, written by `UpdateEc2DeepInspectionConfiguration` and read back by
//!   `GetEc2DeepInspectionConfiguration`, the instant each of its scan types' free
//!   trial began, written where that scan type is switched on and read back by
//!   `BatchGetFreeTrialInfo`, and that account's Amazon Inspector scan configuration —
//!   the ECR automated re-scan setting and the Amazon EC2 scan mode — written by
//!   `UpdateConfiguration` and read back by `GetConfiguration` ([`scan_configuration`]).
//! * `filters` — the suppression/other findings filters, keyed by their own ARN so a
//!   record and its ARN can never diverge. `BTreeMap` iteration is stable, which is what
//!   makes a `ListFilters` page token meaningful across calls.
//! * `cis_scan_configurations` — the CIS scan configurations, keyed by their own ARN for
//!   the same reason, written by `CreateCisScanConfiguration`, listed by
//!   `ListCisScanConfigurations` and removed by `DeleteCisScanConfiguration`.
//! * `encryption_keys` — the customer managed KMS key in force per resource type and scan
//!   type, written by `UpdateEncryptionKey` and read back by `GetEncryptionKey`.
//! * `code_security_scan_configurations` — the code security scan configurations, keyed by
//!   their own ARN for the same reason, written by
//!   `CreateCodeSecurityScanConfiguration`, read back by
//!   `GetCodeSecurityScanConfiguration` and removed by
//!   `DeleteCodeSecurityScanConfiguration`.
//! * `org_package_paths` — the organization-wide deep inspection custom paths both deep
//!   inspection reads report alongside an account's own.
//! * `on_demand_scans` — the on-demand scans of a resource, keyed by their own scan id,
//!   started by `StartOnDemandScan`, reported one at a time by `GetOnDemandScanStatus` and
//!   paged through by `ListOnDemandScans` ([`on_demand_scan`]).
//! * `vulnerabilities` — the vulnerability database `SearchVulnerabilities` answers from,
//!   seeded from the published records in [`vulnerability_catalog`] because no operation
//!   of this API publishes a vulnerability.
//!
//! A resource's tags live on that resource's own record, never in a map beside it, so the
//! tagging operations and the reads that echo a record's tags can never report two
//! different tag sets for one resource:
//! [`ComAmazonawsInspector2Inspector2State::resource_tag_slot_mut`] is the one place a
//! `resourceArn` is resolved to the record whose tags a tagging call reads or writes.
//!
//! No resource lives here for the CIS-session RPCs: [`cis_session`] holds the single
//! identity gate all four of them answer their callers with, which is decided from the
//! calling identity alone and so reaches no store.

mod cis_session;
mod on_demand_scan;
mod scan_configuration;
mod vulnerability_catalog;

pub use cis_session::*;
pub use on_demand_scan::*;
pub use scan_configuration::*;
pub use vulnerability_catalog::*;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    CodeSecurityScanConfiguration, FilterCriteria, Schedule, ScopeSettings, Vulnerability,
};

/// The region a request is scoped to when it carries no SigV4 credential scope to read
/// one from: the region every generated emulator defaults to (see `emulator_arn` in
/// `vendor/server/src/lib.rs`).
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's SigV4 / ARN service namespace.
pub const SERVICE_NAMESPACE: &str = "inspector2";

/// Every `ResourceScanType` token, in model order.
///
/// Source: `smithy_query(query="shape", shape="ResourceScanType")` —
/// `EC2 | ECR | LAMBDA | LAMBDA_CODE | CODE_REPOSITORY`.
pub const RESOURCE_SCAN_TYPES: [&str; 5] = [
    "EC2",
    "ECR",
    "LAMBDA",
    "LAMBDA_CODE",
    "CODE_REPOSITORY",
];

/// How Amazon Machine Image scanning is named where this service reports a scan's status
/// per resource type: `ResourceStatus$ami` and `ResourceState$ami` report it, and
/// `OnDemandResourceType` names the same resource type with its one token.
///
/// It is NOT a `ResourceScanType` (`smithy_query(query="shape",
/// shape="ResourceScanType,ResourceStatus,OnDemandResourceType")`), so no `Enable` or
/// `Disable` request can name it and an account's AMI scanning is reported off until a
/// request that can switch it on exists.
pub const AMI_RESOURCE_TYPE: &str = "AMI";

/// The `Status` token for a scan that is on.
/// Source: `smithy_query(query="shape", shape="Status")`.
pub const STATUS_ENABLED: &str = "ENABLED";

/// The `Status` token for a scan that is off — the state of an account Amazon Inspector
/// has never been enabled for.
/// Source: `smithy_query(query="shape", shape="Status")`.
pub const STATUS_DISABLED: &str = "DISABLED";

/// The `ErrorCode` token a per-account entry carries when the calling account is not
/// allowed to act on, or read, the account that entry names.
/// Source: `smithy_query(query="shape", shape="ErrorCode,FreeTrialInfoErrorCode")` —
/// `ACCESS_DENIED` is a token of both, and `FailedAccount$errorCode` and
/// `FreeTrialInfoError$code` target them, so every per-account entry of this service names
/// that failure with one token.
pub const ERROR_CODE_ACCESS_DENIED: &str = "ACCESS_DENIED";

/// How this service words an [`ERROR_CODE_ACCESS_DENIED`] per-account entry: the calling
/// account has no access to the account the entry names. The entry's only variable part is
/// its `accountId`, so the message itself carries no value from the request.
pub const NO_ACCOUNT_ACCESS_MESSAGE: &str = "Invoking account does not have access to this account";

/// The `FreeTrialStatus` token of a scan type whose free trial window is still open.
/// Source: `smithy_query(query="shape", shape="FreeTrialStatus")` — `ACTIVE | INACTIVE`.
pub const FREE_TRIAL_ACTIVE: &str = "ACTIVE";

/// The `FreeTrialStatus` token of a scan type whose free trial window has closed.
/// Source: `smithy_query(query="shape", shape="FreeTrialStatus")`.
pub const FREE_TRIAL_INACTIVE: &str = "INACTIVE";

/// How long one scan type's Amazon Inspector free trial runs for an account.
///
/// Amazon Inspector documents the length of this trial — the Amazon Inspector User Guide's
/// free-trial section (`docs.aws.amazon.com/inspector/latest/user/free-trial.html`) states
/// that an account activating Amazon Inspector gets a 15-day free trial of each scan type —
/// so the window is 15 days wide rather than a span this emulator chose.
pub const FREE_TRIAL_DURATION: Duration = Duration::from_secs(15 * 24 * 60 * 60);

/// The `Ec2DeepInspectionStatus` token of an account deep inspection is running for.
/// Source: `smithy_query(query="shape", shape="Ec2DeepInspectionStatus")` —
/// `ACTIVATED | DEACTIVATED | PENDING | FAILED`.
pub const DEEP_INSPECTION_ACTIVATED: &str = "ACTIVATED";

/// The `Ec2DeepInspectionStatus` token of an account deep inspection is off for.
/// Source: `smithy_query(query="shape", shape="Ec2DeepInspectionStatus")`.
pub const DEEP_INSPECTION_DEACTIVATED: &str = "DEACTIVATED";

/// The `Ec2DeepInspectionStatus` token of an account whose requested activation change is
/// still being applied.
/// Source: `smithy_query(query="shape", shape="Ec2DeepInspectionStatus")`.
pub const DEEP_INSPECTION_PENDING: &str = "PENDING";

/// How long an EC2 deep inspection activation change stays [`DEEP_INSPECTION_PENDING`]
/// before the clock owes the account its terminal status. The real service applies the
/// change asynchronously, and no AWS document fixes how long it takes, so this span is
/// this emulator's own: non-zero, so a caller reading straight after the write still
/// observes the transient status, and short enough that polling settles quickly.
pub const DEEP_INSPECTION_TRANSITION: Duration = Duration::from_secs(3);

/// The most filters this emulator stores before refusing to mint another, so a caller
/// looping `CreateFilter` cannot grow the process heap without bound. `CreateFilter`
/// models `ServiceQuotaExceededException` precisely because the real service caps filters
/// per account too; this is the emulator's own bound, of the same order.
pub const MAX_STORED_FILTERS: usize = 1_000;

/// The most CIS scan configurations this emulator stores before refusing to mint another,
/// so a caller looping `CreateCisScanConfiguration` cannot grow the process heap without
/// bound. The real service caps CIS scan configurations per account too; this is the
/// emulator's own bound, of the same order as the filter bound above. The operation models
/// no quota error, so the refusal is its `ValidationException`
/// ([`too_many_cis_scan_configurations_message`]).
pub const MAX_STORED_CIS_SCAN_CONFIGURATIONS: usize = 1_000;

/// The most code security scan configurations this emulator stores before refusing to
/// mint another, so a caller looping `CreateCodeSecurityScanConfiguration` cannot grow the
/// process heap without bound. That operation models
/// `ServiceQuotaExceededException` precisely because the real service caps scan
/// configurations per account too; this is the emulator's own bound, of the same order as
/// the filter bound above.
pub const MAX_STORED_CODE_SECURITY_SCAN_CONFIGURATIONS: usize = 1_000;

/// The refusal text for a `CreateCisScanConfiguration` that would take the number of
/// stored CIS scan configurations past [`MAX_STORED_CIS_SCAN_CONFIGURATIONS`].
pub fn too_many_cis_scan_configurations_message() -> String {
    format!(
        "You have reached the limit of {MAX_STORED_CIS_SCAN_CONFIGURATIONS} CIS scan \
         configurations. Delete a CIS scan configuration and try again."
    )
}

/// The `TargetAccount` token standing for the account that owns the CIS scan
/// configuration, rather than for an account id of its own.
/// Source: `smithy_query(query="shape", shape="TargetAccount")` — a 12-digit account id,
/// `ALL_ACCOUNTS` or `SELF`.
pub const TARGET_ACCOUNT_SELF: &str = "SELF";

/// The `TargetAccount` token standing for every account of the owner's organization.
/// Source: `smithy_query(query="shape", shape="TargetAccount")`.
pub const TARGET_ACCOUNT_ALL: &str = "ALL_ACCOUNTS";

/// The most accounts this emulator tracks enablement for. Every operation that records
/// something for an account goes through
/// [`ComAmazonawsInspector2Inspector2State::account_mut`], which refuses to add one past
/// this, so a caller looping any of them with fresh account ids cannot grow the account
/// map without bound.
pub const MAX_ENABLED_ACCOUNTS: usize = 1_000;

/// The refusal text for a request that would take the number of accounts this emulator
/// tracks past [`MAX_ENABLED_ACCOUNTS`], shared by every operation that writes an account
/// record so they all refuse the same way.
pub fn too_many_accounts_message() -> String {
    format!("Amazon Inspector can be enabled for at most {MAX_ENABLED_ACCOUNTS} accounts.")
}

/// The most `Enable` idempotency tokens remembered per account. Tokens are caller-chosen
/// strings, so the record keeps only the most recent ones (oldest dropped first): a
/// retry, which is what the token exists for, follows its original call closely, and
/// nothing can accumulate tokens without bound.
pub const MAX_CLIENT_TOKENS_PER_ACCOUNT: usize = 64;

/// The most tags one resource carries. The model fixes how many tag keys a single
/// `UntagResource` call may name — `TagKeyList` has length 0..50 — which is the size of
/// the tag set one call has to be able to clear, so a resource's tag set is held to that
/// same 50 here: a caller looping `TagResource` with fresh keys cannot grow a stored
/// record without bound.
/// Source: `smithy_query(query="shape", shape="TagKeyList")`.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The tag-key prefix Amazon Web Services reserves for its own tags. The model's own
/// `TagKey` pattern excludes it with a negative lookahead, so a caller-supplied key
/// carrying it is not a valid tag key for this service.
/// Source: `smithy_query(query="shape", shape="TagKey")` — `^(?!aws:)[a-zA-Z+-=._:/]+$`.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// What a tagging write states when a tag it carries is not a valid tag: the sentence the
/// service documents for that failure class, which states the class and nothing about the
/// request, so every invalid tag is refused the same way.
/// Source: `smithy_query(query="shape", shape="BadRequestException")`.
pub const INVALID_TAGS_MESSAGE: &str =
    "One or more tags submitted as part of the request is not valid.";

/// What every tagging call states for a `resourceArn` naming no resource this service
/// stores: the sentence the service documents for that failure class.
/// Source: `smithy_query(query="shape", shape="ResourceNotFoundException")`.
pub const TAGGED_RESOURCE_NOT_FOUND_MESSAGE: &str =
    "The operation tried to access an invalid resource. Make sure the resource is \
     specified correctly.";

/// The refusal text for a tagging write that would leave the resource carrying
/// `requested` tags, past [`MAX_TAGS_PER_RESOURCE`].
pub fn too_many_tags_message(requested: usize) -> String {
    format!(
        "A resource can carry at most {MAX_TAGS_PER_RESOURCE} tags; this request would \
         leave it carrying {requested}."
    )
}

/// One stored findings filter: exactly what `CreateFilter`/`UpdateFilter` were given,
/// plus the ARN the service minted and the two timestamps it maintains.
#[derive(Debug, Clone)]
pub struct StoredFilter {
    /// The minted ARN; also this record's key in [`ComAmazonawsInspector2Inspector2State::filters`].
    pub arn: String,
    /// The 12-digit account that created the filter (`Filter$ownerId`).
    pub owner_id: String,
    pub name: String,
    pub criteria: FilterCriteria,
    /// A `FilterAction` token: `NONE` or `SUPPRESS`.
    pub action: String,
    pub description: Option<String>,
    pub reason: Option<String>,
    /// The tags as sent: `None` when the request carried no `tags` member, and an empty
    /// map when it carried an empty one. Either way a read reports the tag set as a map
    /// ([`tag_map`]). Sorted so the rendering order is stable.
    pub tags: Option<BTreeMap<String, String>>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

/// One stored CIS scan configuration: exactly what `CreateCisScanConfiguration` was
/// given, plus the ARN the service minted and the account that owns it.
#[derive(Debug, Clone)]
pub struct StoredCisScanConfiguration {
    /// The minted ARN; also this record's key in
    /// [`ComAmazonawsInspector2Inspector2State::cis_scan_configurations`].
    pub arn: String,
    /// The account that created the configuration (`CisScanConfiguration$ownerId`).
    pub owner_id: String,
    pub scan_name: String,
    /// A `CisSecurityLevel` token: `LEVEL_1` or `LEVEL_2`.
    pub security_level: String,
    pub schedule: Schedule,
    /// The `TargetAccount` values as SENT — an account id, `SELF` or `ALL_ACCOUNTS`.
    /// `ALL_ACCOUNTS` names a set that changes as accounts join, so the tokens are kept
    /// here and resolved on every read ([`ComAmazonawsInspector2Inspector2State::target_account_ids`])
    /// rather than frozen at create time.
    pub target_accounts: Vec<String>,
    /// The resource tags a scan selects its targets by, as sent. Sorted so the rendering
    /// order is stable; bounded by the modelled `TargetResourceTags` length (at most 5
    /// keys, each with at most 5 values), which the boundary validator enforces before any
    /// handler runs.
    pub target_resource_tags: BTreeMap<String, Vec<String>>,
    /// The tags as sent: `None` when the request carried no `tags` member, and an empty
    /// map when it carried an empty one. Either way a read reports the tag set as a map
    /// ([`tag_map`]). Sorted so the rendering order is stable.
    pub tags: Option<BTreeMap<String, String>>,
}

/// One stored code security scan configuration: exactly what
/// `CreateCodeSecurityScanConfiguration` was given, plus the ARN the service minted, the
/// owner it was created under and the two timestamps the service maintains for it.
#[derive(Debug, Clone)]
pub struct StoredCodeSecurityScanConfiguration {
    /// The minted ARN; also this record's key in
    /// [`ComAmazonawsInspector2Inspector2State::code_security_scan_configurations`].
    pub arn: String,
    /// The owner the configuration was created under, as the ARN's `owner/` segment and
    /// `CodeSecurityScanConfigurationSummary$ownerAccountId` report it.
    pub owner_id: String,
    pub name: String,
    /// A `ConfigurationLevel` token: `ACCOUNT` or `ORGANIZATION`.
    pub level: String,
    /// The scan settings as the service stores them — the rule set categories and the
    /// periodic and continuous-integration settings as sent, with the periodic schedule
    /// the service picks itself filled in by
    /// [`stored_code_security_scan_configuration`] — so a read renders back exactly the
    /// configuration the scans run under, unchanged for as long as the record lives.
    pub configuration: CodeSecurityScanConfiguration,
    /// The scope settings as sent: `None` for a general scan configuration, and present
    /// for a default one, which is the distinction the create's `scopeSettings` member
    /// draws.
    pub scope_settings: Option<ScopeSettings>,
    /// The tags as sent: `None` when the request carried no `tags` member, and an empty
    /// map when it carried an empty one. Either way a read reports the tag set as a map
    /// ([`tag_map`]). Sorted so the rendering order is stable.
    pub tags: Option<BTreeMap<String, String>>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

/// One account's Amazon Inspector deep inspection configuration for Amazon EC2.
#[derive(Debug, Clone, Default)]
pub struct Ec2DeepInspection {
    /// The activation this account last asked for, or `None` while the account has never
    /// configured deep inspection — in which case the activation follows the account's
    /// EC2 scanning enablement (see
    /// [`AccountEnablement::deep_inspection_activated`]), so the absence is kept out of
    /// the boolean rather than spelled as one of its values.
    pub activated: Option<bool>,
    /// The instant the activation change this account asked for stops being
    /// [`DEEP_INSPECTION_PENDING`] — the transition the CLOCK owes, stored rather than
    /// slept for, so the status is derived on every read without writing anything.
    pub pending_until: Option<SystemTime>,
    /// The account's deep inspection custom paths, exactly as the last update wrote them;
    /// empty for an account with no custom paths. Bounded by the modelled `PathList`
    /// length (at most 5), which the boundary validator enforces before any handler runs.
    pub package_paths: Vec<String>,
}

impl Ec2DeepInspection {
    /// Whether the activation change this account asked for is still being applied at
    /// `now`.
    pub fn is_pending(&self, now: SystemTime) -> bool {
        self.pending_until.is_some_and(|due| now < due)
    }
}

/// One scan type's Amazon Inspector free trial window for one account: when it began and
/// when it ends, [`FREE_TRIAL_DURATION`] later.
#[derive(Debug, Clone, Copy)]
pub struct FreeTrialWindow {
    /// The instant the trial began — the first time the scan type's scanning was switched
    /// on for the account.
    pub start: SystemTime,
    /// The instant the trial ends, [`FREE_TRIAL_DURATION`] after it began.
    pub end: SystemTime,
}

impl FreeTrialWindow {
    /// The `FreeTrialStatus` this window reports at `now`: [`FREE_TRIAL_ACTIVE`] while the
    /// window is still open, and [`FREE_TRIAL_INACTIVE`] from the instant it closes.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now < self.end {
            FREE_TRIAL_ACTIVE
        } else {
            FREE_TRIAL_INACTIVE
        }
    }
}

/// Amazon Inspector enablement for one account.
#[derive(Debug, Clone, Default)]
pub struct AccountEnablement {
    /// The account-level `Status` token, or `None` for an account Amazon Inspector has
    /// never been enabled for: `Status` has no token for "no record", so the absence is
    /// kept out of the token space instead of being spelled with an illegal value.
    pub status: Option<String>,
    /// The per-`ResourceScanType` `Status` map both `Enable`'s `Account$resourceStatus`
    /// and `BatchGetAccountStatus`'s `AccountState$resourceState` are rendered from.
    pub resource_status: BTreeMap<String, String>,
    /// When the account was first enabled.
    pub enabled_at: Option<SystemTime>,
    /// The instant each `ResourceScanType`'s Amazon Inspector free trial began for this
    /// account — the first time that scan type's scanning was switched on — keyed by the
    /// scan type. A scan type absent here has never been switched on, so its trial has not
    /// begun and there is no window to report for it, which is a different answer from a
    /// window that has closed.
    ///
    /// BOUNDED without a cap of its own: a key is only ever one of the five modelled
    /// `ResourceScanType` tokens, and a token outside that enum is refused at the request
    /// boundary, so this map holds at most five entries however many requests arrive.
    ///
    /// An entry is written once and never rewritten or removed: the trial is the account's
    /// one 15-day window for that scan type, so turning the scan type off and on again
    /// neither restarts it nor hands out a second one.
    pub trial_starts: BTreeMap<String, SystemTime>,
    /// `Enable` idempotency tokens already honoured for this account, so a retry of the
    /// same request is a replay rather than a second write. A BOUNDED queue, oldest
    /// first: at most [`MAX_CLIENT_TOKENS_PER_ACCOUNT`] are kept, so caller-chosen
    /// strings cannot accumulate here. Add through [`AccountEnablement::remember_token`].
    pub client_tokens: VecDeque<String>,
    /// The account's Amazon Inspector deep inspection configuration for Amazon EC2,
    /// written by `UpdateEc2DeepInspectionConfiguration` and read back by
    /// `GetEc2DeepInspectionConfiguration`.
    pub deep_inspection: Ec2DeepInspection,
    /// The account's OWN Amazon Inspector scan configuration, written by
    /// `UpdateConfiguration` and read back by `GetConfiguration`. A scan type with no
    /// record here was never configured individually for this account and follows the
    /// delegated administrator's — see [`scan_configuration`].
    pub scan_configuration: ScanConfiguration,
}

impl AccountEnablement {
    /// The account-level `Status`. An account with no enablement record reports
    /// [`STATUS_DISABLED`], which is what the real service reports for an account Amazon
    /// Inspector is not enabled for.
    pub fn account_status(&self) -> &str {
        self.status.as_deref().unwrap_or(STATUS_DISABLED)
    }

    /// The `Status` of one resource scan type. An account that was never enabled for a
    /// type reports [`STATUS_DISABLED`], which is what the real service reports for a
    /// scan that is off.
    pub fn status_of(&self, resource_type: &str) -> &str {
        self.resource_status
            .get(resource_type)
            .map_or(STATUS_DISABLED, String::as_str)
    }

    /// Whether Amazon Inspector deep inspection is in force for this account — the
    /// activation the account last asked for, or, while it has never configured deep
    /// inspection, the one that follows from its EC2 scanning enablement: Amazon Inspector
    /// runs deep inspection for an account whose EC2 scanning is on unless that account
    /// turns it off. While a change is still pending this is the activation being applied,
    /// which is what another change is measured against.
    pub fn deep_inspection_activated(&self) -> bool {
        self.deep_inspection
            .activated
            .unwrap_or_else(|| self.status_of("EC2") == STATUS_ENABLED)
    }

    /// The `Ec2DeepInspectionStatus` this account reports at `now`: the activation change
    /// it asked for while that change is still being applied, and the activation in force
    /// once the clock has closed the span.
    pub fn deep_inspection_status(&self, now: SystemTime) -> &'static str {
        if self.deep_inspection.is_pending(now) {
            DEEP_INSPECTION_PENDING
        } else if self.deep_inspection_activated() {
            DEEP_INSPECTION_ACTIVATED
        } else {
            DEEP_INSPECTION_DEACTIVATED
        }
    }

    /// Turn every scan type `resource_types` names off for this account.
    ///
    /// A scan type the call does not name keeps whatever it had, and the account-level
    /// `Status` follows the scan types: it stays [`STATUS_ENABLED`] while any scan type is
    /// still on, and becomes [`STATUS_DISABLED`] once none is, because turning off every
    /// scan type in an account turns Amazon Inspector itself off for it.
    ///
    /// A free trial window already begun ([`Self::trial_starts`]) is left exactly as it
    /// was: the trial is a span of the account's calendar rather than a span of scanning,
    /// so it keeps running while the scan type is off and does not start over when the
    /// scan type comes back on.
    pub fn disable(&mut self, resource_types: &[String]) {
        for resource_type in resource_types {
            self.resource_status
                .insert(resource_type.clone(), STATUS_DISABLED.to_string());
        }
        let any_on = RESOURCE_SCAN_TYPES
            .iter()
            .any(|resource_type| self.status_of(resource_type) == STATUS_ENABLED);
        if !any_on {
            self.status = Some(STATUS_DISABLED.to_string());
            // Amazon Inspector is no longer enabled for the account, so neither the
            // instant it was enabled at nor the tokens that acknowledged that enablement
            // describes anything any more: a later `Enable` carrying one of those tokens
            // asks for a fresh enablement rather than replaying the disabled one.
            self.enabled_at = None;
            self.client_tokens.clear();
        }
    }

    /// Begin this account's free trial of `resource_type` at `now`, if it has not begun
    /// already. Called where a scan type's scanning is switched on; a scan type whose
    /// trial already began keeps its original window.
    pub fn begin_free_trial(&mut self, resource_type: &str, now: SystemTime) {
        self.trial_starts
            .entry(resource_type.to_string())
            .or_insert(now);
    }

    /// This account's free trial window for `resource_type`, or `None` while that scan
    /// type's trial has not begun.
    pub fn free_trial(&self, resource_type: &str) -> Option<FreeTrialWindow> {
        self.trial_starts
            .get(resource_type)
            .map(|start| FreeTrialWindow {
                start: *start,
                end: *start + FREE_TRIAL_DURATION,
            })
    }

    /// Whether this account was already enabled under `token`.
    pub fn has_token(&self, token: &str) -> bool {
        self.client_tokens.iter().any(|seen| seen == token)
    }

    /// Remember `token`, dropping the oldest once
    /// [`MAX_CLIENT_TOKENS_PER_ACCOUNT`] are held.
    pub fn remember_token(&mut self, token: String) {
        if self.has_token(&token) {
            return;
        }
        self.client_tokens.push_back(token);
        while self.client_tokens.len() > MAX_CLIENT_TOKENS_PER_ACCOUNT {
            self.client_tokens.pop_front();
        }
    }
}

/// Per-service state for com-amazonaws-inspector2-inspector2. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsInspector2Inspector2State {
    /// Stored filters, keyed by ARN. Bounded by [`MAX_STORED_FILTERS`]: `CreateFilter`
    /// refuses once the map holds that many.
    pub filters: BTreeMap<String, StoredFilter>,
    /// Account enablement, keyed by 12-digit account id. Bounded by
    /// [`MAX_ENABLED_ACCOUNTS`], which [`Self::account_mut`] — the only place an entry is
    /// added — enforces for every operation that writes one.
    pub accounts: BTreeMap<String, AccountEnablement>,
    /// Monotonic source for the id segment of a minted filter ARN.
    pub next_filter_id: u64,
    /// Stored CIS scan configurations, keyed by ARN. Bounded by
    /// [`MAX_STORED_CIS_SCAN_CONFIGURATIONS`]: `CreateCisScanConfiguration` refuses once
    /// the map holds that many, and only `DeleteCisScanConfiguration` removes one.
    pub cis_scan_configurations: BTreeMap<String, StoredCisScanConfiguration>,
    /// Monotonic source for the id segment of a minted CIS scan configuration ARN.
    pub next_cis_scan_configuration_id: u64,
    /// Stored code security scan configurations, keyed by ARN. Bounded by
    /// [`MAX_STORED_CODE_SECURITY_SCAN_CONFIGURATIONS`]:
    /// `CreateCodeSecurityScanConfiguration` refuses once the map holds that many, and
    /// only `DeleteCodeSecurityScanConfiguration` removes one.
    pub code_security_scan_configurations: BTreeMap<String, StoredCodeSecurityScanConfiguration>,
    /// Monotonic source for the id segment of a minted code security scan configuration
    /// ARN.
    pub next_code_security_scan_configuration_id: u64,
    /// The customer managed KMS key in force for one `ResourceType` / `ScanType` pair —
    /// the scope Amazon Inspector configures an encryption key at, for the single account
    /// this emulator presents (neither encryption-key operation carries an account
    /// member). An absent entry means that pair is encrypted with an Amazon Web Services
    /// owned key, which is a different answer from a configured key rather than a missing
    /// value, so the absence is kept out of the `KmsKeyArn` value space.
    ///
    /// BOUNDED without a cap of its own: both halves of the key are modelled enum tokens,
    /// and a token outside those enums is refused at the boundary before any handler runs,
    /// so the map holds at most `ResourceType` × `ScanType` entries however many requests
    /// arrive. No request-supplied string ever becomes a key here.
    pub encryption_keys: BTreeMap<EncryptionKeyScope, String>,
    /// The ORGANIZATION's deep inspection custom paths, which apply to every member
    /// account on top of the account's own: one list for the organization this emulator
    /// presents, since the operation that sets it carries no organization member. Empty
    /// until the organization's delegated administrator sets paths through
    /// `UpdateOrgEc2DeepInspectionConfiguration`, which both deep inspection reads report
    /// from here. Bounded by the modelled `PathList` length (at most 5).
    pub org_package_paths: Vec<String>,
    /// The vulnerability database, keyed by the CVE id a record is addressed by.
    ///
    /// Amazon Inspector republishes vulnerability records rather than minting them, and
    /// no operation of this API writes one, so this map is SEEDED with the published
    /// records this emulator ships ([`seed_vulnerability_catalog`]) and only read from.
    /// BOUNDED without a cap of its own: nothing adds an entry after the seed.
    pub vulnerabilities: BTreeMap<String, Vulnerability>,
    /// The recorded on-demand scans, keyed by their own scan id — which is minted in
    /// starting order, so iteration walks the scans in the order they were started.
    /// Bounded by [`MAX_ON_DEMAND_SCANS`]: `StartOnDemandScan`, the only operation that
    /// adds one, refuses once the map holds that many.
    pub on_demand_scans: BTreeMap<String, StoredOnDemandScan>,
    /// Monotonic source for a minted `ScanId`.
    pub next_on_demand_scan_id: u64,
}

impl Default for ComAmazonawsInspector2Inspector2State {
    fn default() -> Self {
        Self {
            filters: BTreeMap::new(),
            accounts: BTreeMap::new(),
            next_filter_id: 0,
            cis_scan_configurations: BTreeMap::new(),
            next_cis_scan_configuration_id: 0,
            code_security_scan_configurations: BTreeMap::new(),
            next_code_security_scan_configuration_id: 0,
            encryption_keys: BTreeMap::new(),
            org_package_paths: Vec::new(),
            vulnerabilities: seed_vulnerability_catalog(),
            on_demand_scans: BTreeMap::new(),
            next_on_demand_scan_id: 0,
        }
    }
}

/// What both EC2 deep inspection reads report for one account: its activation status at
/// the instant it was asked, its own custom paths and the organization's.
#[derive(Debug, Clone)]
pub struct Ec2DeepInspectionReport {
    /// An `Ec2DeepInspectionStatus` token.
    pub status: String,
    /// The account's custom paths, absent when it has none.
    pub package_paths: Option<Vec<String>>,
    /// The organization's custom paths, absent when it has none.
    pub org_package_paths: Option<Vec<String>>,
}

/// One account's enablement as the wire `Account` both `Enable` and `Disable` report: the
/// account-level `Status` and the per-`ResourceScanType` status map, every modelled scan
/// type reported, so an account with no record reports every scan off rather than leaving
/// a type out.
///
/// Source: `smithy_query(query="shape", shape="Account,ResourceStatus")` — `ec2` and `ecr`
/// are required members, the other four optional.
pub fn account_view(
    account_id: &str,
    enablement: &AccountEnablement,
) -> crate::ops::types::Account {
    crate::ops::types::Account {
        account_id: account_id.to_string(),
        status: enablement.account_status().to_string(),
        resource_status: crate::ops::types::ResourceStatus {
            ec2: enablement.status_of("EC2").to_string(),
            ecr: enablement.status_of("ECR").to_string(),
            lambda: Some(enablement.status_of("LAMBDA").to_string()),
            lambda_code: Some(enablement.status_of("LAMBDA_CODE").to_string()),
            code_repository: Some(enablement.status_of("CODE_REPOSITORY").to_string()),
            ami: Some(enablement.status_of(AMI_RESOURCE_TYPE).to_string()),
        },
    }
}

/// Render a stored `PathList` for the wire: the paths when any are configured, and the
/// member left ABSENT when none are, so an account that never set a custom path and one
/// that cleared its paths report the same thing — having no custom paths — rather than one
/// of them reporting an empty list.
pub fn path_list(paths: &[String]) -> Option<Vec<String>> {
    (!paths.is_empty()).then(|| paths.to_vec())
}

/// Render a stored resource's tag map for the wire: always PRESENT, and EMPTY when the
/// resource holds no tags at all.
///
/// A resource's tag set is part of the resource, so it is reported unconditionally and the
/// shape of a response never depends on whether tags were ever set: a resource created
/// without a `tags` member and one created with an empty map both report an empty map, and
/// a client that indexes or iterates the map never meets an absent member it has to branch
/// on.
///
/// Source: `smithy_query(query="shape", shape="Filter,CisScanConfiguration,TagMap")` and
/// `smithy_query(query="operation", operation="GetCodeSecurityScanConfiguration")` — every
/// one of them reports `tags` as a `TagMap`.
pub fn tag_map(
    tags: Option<&BTreeMap<String, String>>,
) -> ::std::collections::HashMap<String, String> {
    tags.map(|tags| {
        tags.iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    })
    .unwrap_or_default()
}

/// The days of the week a scan schedule can name, in the order this service's own day
/// vocabulary lists them, so index 0 is the first day of the week.
///
/// Source: `smithy_query(query="shape", shape="Day")` — this service names a day of the
/// week by abbreviation (`SUN`, `MON`, `TUE`, `WED`, `THU`, `FRI`, `SAT`), which is also
/// the day-of-week vocabulary an AWS cron expression accepts.
pub const SCAN_DAYS_OF_WEEK: [&str; 7] = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];

/// The highest day of the month a monthly schedule may name: the last day EVERY month has,
/// so a monthly scan recurs in every month including February.
pub const LAST_DAY_EVERY_MONTH_HAS: usize = 28;

/// The time of day — minute, then hour, as an AWS cron expression orders them — that a
/// schedule Amazon Inspector chose itself runs at: the start of the day, UTC.
///
/// Fixed, not derived from anything: the only part of a chosen schedule the model licenses
/// the service to pick is the DAY the scan runs on, so the time of day a chosen schedule
/// names is the same for every configuration.
///
/// Source: `smithy_query(query="shape", shape="PeriodicScanConfiguration")` — the
/// `frequency` member documents that Amazon Inspector chooses the day for the scan to run
/// when `frequencyExpression` is not provided.
const CHOSEN_SCHEDULE_TIME_OF_DAY: (u32, u32) = (0, 0);

/// What the day of a chosen schedule is mixed under, so the index is this one choice's and
/// no other derived value of this crate can collide with it.
const SCHEDULE_DAY_SEED: &str = "code-security-periodic-scan-day";

/// How many hex digits of the mixed id the day is read from: eight, the widest run that
/// still parses into a `u32`.
const SCHEDULE_DAY_HEX_DIGITS: usize = 8;

/// Which day a schedule Amazon Inspector chose names, as an index into a `count`-day cycle
/// — a non-zero cycle length: the two call sites pass the size of a day vocabulary —
/// decided from the configuration's own identifier `configuration_id`.
///
/// The day is a property of the ONE configuration it was chosen for: it is the same on
/// every read of that configuration for as long as the configuration exists, it does not
/// move when the clock does, and two configurations need not run on the same day — which is
/// what spreads scheduled scans across the cycle. The identifier is the only thing the
/// choice reads, so nothing about the request or the moment it landed can change it.
///
/// The mixing is the workspace's one derivation of a value a service computes from what it
/// identifies and then puts on the wire (`::service_common::stable_hex_id`), so the spread
/// over the cycle is that function's and nothing is hand-rolled here.
fn chosen_schedule_day_index(configuration_id: &str, count: usize) -> usize {
    debug_assert!(count > 0, "a cycle of days has at least one day in it");
    let mixed = ::service_common::stable_hex_id(&[SCHEDULE_DAY_SEED, configuration_id]);
    let bucket = mixed
        .get(..SCHEDULE_DAY_HEX_DIGITS)
        .and_then(|digits| u32::from_str_radix(digits, 16).ok())
        .unwrap_or_default();
    usize::try_from(bucket).unwrap_or_default() % count
}

/// The cron schedule expression Amazon Inspector itself picks for a periodic code security
/// scan that was asked for by `frequency` alone, or `None` for a frequency that names no
/// schedule at all.
///
/// A caller may name the frequency without naming a schedule, in which case the service
/// chooses the day the scan runs on and the configuration carries a schedule expression
/// matching that frequency; a caller that names the expression keeps its own. The day is
/// the only field the service picks ([`chosen_schedule_day_index`], from the
/// configuration's own id): the time of day is the fixed
/// [`CHOSEN_SCHEDULE_TIME_OF_DAY`], and the remaining fields are the wildcards that make
/// the expression recur with the frequency asked for. So the schedule a configuration
/// reports is the same on every read and the same on every run of the same program, rather
/// than a value that moves with the clock.
///
/// Source: `smithy_query(query="shape", shape="PeriodicScanConfiguration")` — the
/// `frequency` member documents that Amazon Inspector chooses the day for the scan to run
/// when `frequencyExpression` is not provided, and that a provided expression must match
/// the frequency; `PeriodicScanFrequency` admits `WEEKLY`, `MONTHLY` and `NEVER`.
pub fn chosen_frequency_expression(frequency: &str, configuration_id: &str) -> Option<String> {
    let (minute, hour) = CHOSEN_SCHEDULE_TIME_OF_DAY;
    match frequency {
        // A weekly scan recurs on one day of the week, named the way this service names a
        // day, with the day-of-month field left as the `?` the AWS cron format requires
        // when the day-of-week field names the day.
        "WEEKLY" => Some(format!(
            "cron({minute} {hour} ? * {day_of_week} *)",
            day_of_week = SCAN_DAYS_OF_WEEK
                [chosen_schedule_day_index(configuration_id, SCAN_DAYS_OF_WEEK.len())]
        )),
        // A monthly scan recurs on one day of the month, with day-of-week left as `?`. The
        // day is one every month has, so the scan does not skip a short month.
        "MONTHLY" => Some(format!(
            "cron({minute} {hour} {day_of_month} * ? *)",
            day_of_month =
                1 + chosen_schedule_day_index(configuration_id, LAST_DAY_EVERY_MONTH_HAS)
        )),
        // `NEVER` turns periodic scanning off, so there is no schedule to express.
        "NEVER" => None,
        // Periodic settings that name no frequency at all run on no schedule either. The
        // boundary validator refuses a token outside `PeriodicScanFrequency` before any
        // handler runs, so nothing else reaches here.
        _ => None,
    }
}

/// One `CodeSecurityScanConfiguration` as the service STORES it: what the request carried,
/// with the periodic schedule the service chooses itself filled in
/// ([`chosen_frequency_expression`], for the configuration `arn` names) when the periodic
/// settings name a frequency but no expression. Stored this way rather than derived on each
/// read, so every read of the configuration — and an update that round-trips one — sees the
/// one schedule the configuration runs on.
pub fn stored_code_security_scan_configuration(
    mut configuration: crate::ops::types::CodeSecurityScanConfiguration,
    arn: &str,
) -> crate::ops::types::CodeSecurityScanConfiguration {
    // The configuration's own id — the last segment of the ARN
    // [`ComAmazonawsInspector2Inspector2State::mint_code_security_scan_configuration_arn`]
    // minted for it, which identifies this one configuration and nothing else. `split`
    // always yields at least one part, so a value carrying no `/` at all is its own id.
    let configuration_id = arn.split('/').next_back().unwrap_or(arn);
    if let Some(periodic) = configuration.periodic_scan_configuration.as_mut() {
        periodic.frequency_expression = periodic.frequency_expression.take().or_else(|| {
            periodic
                .frequency
                .as_deref()
                .and_then(|frequency| chosen_frequency_expression(frequency, configuration_id))
        });
    }
    configuration
}

/// Whether `value` is a well-formed filter ARN of this service — the SHAPE check every
/// operation that takes a `FilterArn` naming a filter applies before it looks the filter
/// up, so a value that could not name any filter is refused as invalid input instead of
/// being reported as a missing resource.
///
/// Well-formed means a resource ARN of this service's namespace
/// (`::service_common::arn`, the one ARN parse) that carries a region and an account and
/// whose resource is `owner/<owner>/filter/<id>` — the form
/// [`ComAmazonawsInspector2Inspector2State::mint_filter_arn`] mints. An ARN may leave its
/// region or account empty in general (a partition-global resource), but a filter is
/// regional and account-owned, so neither is empty in one of its ARNs.
///
/// The resource form is the one this service's OTHER resource ARNs state as a modelled
/// `@pattern` — `owner/(o-<org>|<account>)/<resource type>/<id>`; `FilterArn` is the shape
/// that carries no pattern, which is why the check lives here instead of at the request
/// boundary.
/// Source: `smithy_query(query="shape",
/// shape="FilterArn,CisScanConfigurationArn,ScanConfigurationArn")`.
///
/// The identity of the partition, region, account, owner and id is deliberately NOT
/// checked: whether a well-formed ARN names a stored filter is the store's answer, not the
/// shape's.
pub fn is_filter_arn(value: &str) -> bool {
    let Some(arn) = ::service_common::arn::parse_of(value, SERVICE_NAMESPACE) else {
        return false;
    };
    if arn.region.is_empty() || arn.account.is_empty() {
        return false;
    }
    // Everything the resource field carries after its `owner` type, which is the owner the
    // filter belongs to and then the filter itself.
    let Some(owned) = ::service_common::arn::resource_id_of_type(value, "owner") else {
        return false;
    };
    let mut segments = owned.splitn(3, '/');
    matches!(
        (segments.next(), segments.next(), segments.next()),
        (Some(owner), Some("filter"), Some(id))
            if !owner.is_empty() && !id.is_empty() && !id.contains(['/', ':'])
    )
}

/// How this service words its refusal of a `filterArn`/`arn` member that is not a
/// well-formed filter ARN ([`is_filter_arn`]), so every operation that takes one refuses
/// it the same way. The value is echoed exactly as the request sent it.
pub fn invalid_filter_arn_message(value: &str) -> String {
    format!("Invalid Filter Arn {value}")
}

/// The findings backend that serves this service's filter plane, as the SDK layer relaying
/// the call to it names that call's service in the envelope around a refusal — neither this
/// service's endpoint prefix nor its `sdkId`.
///
/// No model states a message's text, so this is recorded behaviour: a filter refusal reaches
/// a client wrapped in the envelope of the relayed call that produced it.
pub const FILTER_BACKEND_SERVICE: &str = "MandoFindings";

/// The HTTP status a filter lookup that finds nothing is refused with, which is also the
/// status the relayed call to [`FILTER_BACKEND_SERVICE`] answered with.
/// Source: `smithy_query(query="shape", shape="ResourceNotFoundException")` — `httpError`
/// 404.
pub const FILTER_NOT_FOUND_STATUS: u16 = 404;

/// How this service words its refusal of a WELL-FORMED filter ARN ([`is_filter_arn`]) that
/// names no stored filter, so every operation that looks a filter up refuses the same way.
///
/// The ARN comes FIRST, echoed exactly as the request sent it, with no noun phrase before
/// it; then the bare sentence " does not exist", with no closing period; then the envelope
/// the relayed call to the findings backend is reported inside — that backend's name, the
/// status it answered with, that call's own request id and how many attempts it took. The
/// envelope itself is `service_common::relayed_refusal`, the workspace's one rendering of
/// it, so the request id is freshly minted per response and two refusals never report one
/// id.
pub fn filter_not_found_message(value: &str) -> String {
    ::service_common::relayed_refusal::relayed_refusal_message(
        &format!("{value} does not exist"),
        ::service_common::relayed_refusal::RelayedCall::once(
            FILTER_BACKEND_SERVICE,
            FILTER_NOT_FOUND_STATUS,
        ),
    )
}

/// The `ValidationExceptionReason` token for a request whose member carried a value the
/// service rejects: the field itself failed validation.
/// Source: `smithy_query(query="shape", shape="ValidationExceptionReason")` —
/// `CANNOT_PARSE | FIELD_VALIDATION_FAILED | OTHER`.
pub const VALIDATION_FIELD_FAILED: &str = "FIELD_VALIDATION_FAILED";

/// The scope one encryption-key setting belongs to: a `ResourceType` token and a
/// `ScanType` token, in that order, so iteration groups a resource type's settings
/// together.
pub type EncryptionKeyScope = (String, String);

impl ComAmazonawsInspector2Inspector2State {
    /// The `tags` member of the record `resource_arn` names, as the SLOT a tagging write
    /// replaces: `None` when this service stores no resource under that ARN, and
    /// otherwise that record's own tags — themselves absent while the create that minted
    /// it carried no `tags` member at all.
    ///
    /// Every taggable resource of this service answers here, so one ARN resolves to one
    /// tag set however it is reached.
    pub fn resource_tag_slot_mut(
        &mut self,
        resource_arn: &str,
    ) -> Option<&mut Option<BTreeMap<String, String>>> {
        if let Some(filter) = self.filters.get_mut(resource_arn) {
            return Some(&mut filter.tags);
        }
        if let Some(configuration) = self.cis_scan_configurations.get_mut(resource_arn) {
            return Some(&mut configuration.tags);
        }
        self.code_security_scan_configurations
            .get_mut(resource_arn)
            .map(|configuration| &mut configuration.tags)
    }

    /// The tag set of the resource `resource_arn` names, EMPTY for a stored resource
    /// carrying no tags, and `None` when this service stores no resource under that ARN —
    /// which is what tells "this resource carries no tags" from "there is no such
    /// resource".
    pub fn resource_tags(&self, resource_arn: &str) -> Option<BTreeMap<String, String>> {
        let stored = if let Some(filter) = self.filters.get(resource_arn) {
            &filter.tags
        } else if let Some(configuration) = self.cis_scan_configurations.get(resource_arn) {
            &configuration.tags
        } else {
            &self
                .code_security_scan_configurations
                .get(resource_arn)?
                .tags
        };
        Some(stored.clone().unwrap_or_default())
    }

    /// The customer managed KMS key ARN configured for the `resource_type` /
    /// `scan_type` pair, or `None` when that pair is encrypted with an Amazon Web
    /// Services owned key.
    pub fn encryption_key(&self, resource_type: &str, scan_type: &str) -> Option<&str> {
        self.encryption_keys
            .get(&(resource_type.to_string(), scan_type.to_string()))
            .map(String::as_str)
    }

    /// Put `kms_key_id` in force for the `resource_type` / `scan_type` pair, replacing
    /// whatever key that pair used before. Writes at most one entry per modelled pair, so
    /// repeating the call cannot grow the map.
    pub fn set_encryption_key(
        &mut self,
        resource_type: &str,
        scan_type: &str,
        kms_key_id: String,
    ) {
        self.encryption_keys.insert(
            (resource_type.to_string(), scan_type.to_string()),
            kms_key_id,
        );
    }

    /// Mint the ARN for a new filter:
    /// `arn:aws:inspector2:<region>:<account>:owner/<owner>/filter/<id>`.
    ///
    /// The id segment is a zero-padded 16-hex-digit counter, so the whole ARN is well
    /// inside `FilterArn`'s modelled 1..128 length and no two filters of one emulator
    /// run ever collide.
    pub fn mint_filter_arn(&mut self, region: &str, owner_id: &str) -> String {
        self.next_filter_id += 1;
        let id = format!("{:016x}", self.next_filter_id);
        ::engine::emulator_arn(
            SERVICE_NAMESPACE,
            region,
            &format!("owner/{owner_id}/filter/{id}"),
        )
    }

    /// Mint the ARN for a new CIS scan configuration:
    /// `arn:aws:inspector2:<region>:<account>:owner/<owner>/cis-configuration/<id>`.
    ///
    /// `CisScanConfigurationArn` ends `/cis-configuration/[0-9a-fA-F-]+`, which is the
    /// hyphenated UUID form the real service mints that segment in. The 16 bytes behind it
    /// are this emulator's monotonic counter, so no two configurations of one run collide;
    /// the UUID LAYOUT — the group boundaries and the version and variant bits — is
    /// `service_common`'s one rendering of 16 opaque bytes, so it is not restated here.
    pub fn mint_cis_scan_configuration_arn(&mut self, region: &str, owner_id: &str) -> String {
        self.next_cis_scan_configuration_id += 1;
        let counter = self.next_cis_scan_configuration_id.to_be_bytes();
        let mut bytes = [0u8; 16];
        // The counter's low 56 bits, placed BELOW the byte the variant bits land in, so
        // the layout never overwrites a digit of the counter.
        bytes[9..].copy_from_slice(&counter[1..]);
        let id = ::service_common::uuid_from_random_bytes(bytes);
        ::engine::emulator_arn(
            SERVICE_NAMESPACE,
            region,
            &format!("owner/{owner_id}/cis-configuration/{id}"),
        )
    }

    /// Mint the ARN for a new code security scan configuration:
    /// `arn:aws:inspector2:<region>:<account>:owner/<owner>/codesecurity-configuration/<id>`.
    ///
    /// `ScanConfigurationArn` ends `/codesecurity-configuration/[a-f0-9-]{36}`, which is
    /// the hyphenated UUID form the real service mints that segment in. The 16 bytes
    /// behind it are this emulator's monotonic counter, so no two configurations of one run
    /// collide; the UUID LAYOUT is `service_common`'s one rendering of 16 opaque bytes, so
    /// it is not restated here.
    pub fn mint_code_security_scan_configuration_arn(
        &mut self,
        region: &str,
        owner_id: &str,
    ) -> String {
        self.next_code_security_scan_configuration_id += 1;
        let counter = self.next_code_security_scan_configuration_id.to_be_bytes();
        let mut bytes = [0u8; 16];
        // The counter's low 56 bits, placed BELOW the byte the variant bits land in, so
        // the layout never overwrites a digit of the counter.
        bytes[9..].copy_from_slice(&counter[1..]);
        let id = ::service_common::uuid_from_random_bytes(bytes);
        ::engine::emulator_arn(
            SERVICE_NAMESPACE,
            region,
            &format!("owner/{owner_id}/codesecurity-configuration/{id}"),
        )
    }

    /// The accounts the organization `owner_id` administers holds, as this store knows
    /// them: the owner itself, plus every account Amazon Inspector has been enabled for
    /// through it. That set is what a `TARGET_ACCOUNT_ALL` target names, and it is read on
    /// every call rather than recorded, because an account that joins later is inside a
    /// configuration that already targets every account.
    ///
    /// This is the set form of [`Self::administers_account`] — the same membership, enumerated
    /// rather than asked about one id — so the two answer the same question and a caller needing
    /// the ids and a caller needing the decision cannot disagree.
    fn organization_account_ids(&self, owner_id: &str) -> BTreeSet<String> {
        let mut accounts: BTreeSet<String> = self.accounts.keys().cloned().collect();
        accounts.insert(owner_id.to_string());
        accounts
    }

    /// Whether `account_id` is an account the calling account `caller_id` administers —
    /// membership of the set [`Self::organization_account_ids`] enumerates, answered without
    /// building it so a batch of ids costs one lookup each.
    ///
    /// This is the one place an operation asks whether the calling account may act on
    /// another account: its own account always, and otherwise an account Amazon Inspector
    /// holds a record for, which in this emulator is how an account enters the
    /// administrator's organization. An id no write ever recorded is outside it.
    pub fn administers_account(&self, caller_id: &str, account_id: &str) -> bool {
        account_id == caller_id || self.accounts.contains_key(account_id)
    }

    /// The account ids `stored` targets, as the wire `CisTargets$accountIds` reports them.
    ///
    /// A create names its targets as `TargetAccount` values, which admit two tokens
    /// besides an account id, while a read reports `AccountId` values (12 digits) — so the
    /// tokens are RESOLVED here: `SELF` is the account that owns the configuration, and
    /// `ALL_ACCOUNTS` is every account of its organization
    /// ([`Self::organization_account_ids`]). The ids are de-duplicated and ordered, which
    /// is what the modelled `uniqueItems` asks for and makes two reads of one
    /// configuration agree.
    ///
    /// Source: `smithy_query(query="shape", shape="TargetAccount,CisAccountIdList")`.
    pub fn target_account_ids(&self, stored: &StoredCisScanConfiguration) -> Vec<String> {
        let mut resolved: BTreeSet<String> = BTreeSet::new();
        for target in &stored.target_accounts {
            match target.as_str() {
                TARGET_ACCOUNT_SELF => {
                    resolved.insert(stored.owner_id.clone());
                }
                TARGET_ACCOUNT_ALL => {
                    resolved.extend(self.organization_account_ids(&stored.owner_id));
                }
                account_id => {
                    resolved.insert(account_id.to_string());
                }
            }
        }
        resolved.into_iter().collect()
    }

    /// One stored CIS scan configuration as the wire `CisScanConfiguration`: everything
    /// the create stored, with its targets resolved ([`Self::target_account_ids`]).
    ///
    /// Source: `smithy_query(query="shape", shape="CisScanConfiguration")` —
    /// `scanConfigurationArn` is required and every other member optional.
    pub fn cis_scan_configuration_view(
        &self,
        stored: &StoredCisScanConfiguration,
    ) -> crate::ops::types::CisScanConfiguration {
        crate::ops::types::CisScanConfiguration {
            scan_configuration_arn: stored.arn.clone(),
            owner_id: Some(stored.owner_id.clone()),
            scan_name: Some(stored.scan_name.clone()),
            security_level: Some(stored.security_level.clone()),
            schedule: Some(stored.schedule.clone()),
            targets: Some(crate::ops::types::CisTargets {
                account_ids: Some(self.target_account_ids(stored)),
                target_resource_tags: Some(
                    stored
                        .target_resource_tags
                        .iter()
                        .map(|(key, values)| (key.clone(), values.clone()))
                        .collect(),
                ),
            }),
            // Always reported, and empty for a configuration carrying no tags at all
            // ([`tag_map`]).
            tags: Some(tag_map(stored.tags.as_ref())),
        }
    }

    /// The Amazon Inspector deep inspection configuration `account_id` reports at `now`.
    ///
    /// A pure read: the status is DERIVED from the stored activation and the instant the
    /// activation change falls due, so reporting it writes nothing and a later caller sees
    /// exactly what this one did. An account with no record of its own reports what any
    /// account with no enablement and no deep inspection configuration reports.
    pub fn ec2_deep_inspection(
        &self,
        account_id: &str,
        now: SystemTime,
    ) -> Ec2DeepInspectionReport {
        let unconfigured = AccountEnablement::default();
        let account = self.accounts.get(account_id).unwrap_or(&unconfigured);
        Ec2DeepInspectionReport {
            status: account.deep_inspection_status(now).to_string(),
            package_paths: path_list(&account.deep_inspection.package_paths),
            org_package_paths: path_list(&self.org_package_paths),
        }
    }

    /// Apply an `UpdateEc2DeepInspectionConfiguration` to `account_id`: `activate` is the
    /// activation asked for (absent when the request only touches custom paths), and
    /// `package_paths` the account's custom paths in full (absent when the request only
    /// touches the activation).
    ///
    /// Only a CHANGE of activation starts a transition: a request asking for the
    /// activation already in force leaves the status terminal, because the service has
    /// nothing to apply for it.
    ///
    /// Adds at most one entry to [`Self::accounts`], through [`Self::account_mut`], which
    /// owns the bound on that map — so `false`, meaning the configuration was NOT applied,
    /// is the answer once [`MAX_ENABLED_ACCOUNTS`] accounts are tracked and `account_id` is
    /// not one of them. The caller refuses the request with the modelled error rather than
    /// reporting a configuration it did not store.
    #[must_use]
    pub fn update_ec2_deep_inspection(
        &mut self,
        account_id: &str,
        activate: Option<bool>,
        package_paths: Option<Vec<String>>,
        now: SystemTime,
    ) -> bool {
        let Some(account) = self.account_mut(account_id) else {
            return false;
        };
        if let Some(activate) = activate {
            if activate != account.deep_inspection_activated() {
                account.deep_inspection.pending_until = Some(now + DEEP_INSPECTION_TRANSITION);
            }
            account.deep_inspection.activated = Some(activate);
        }
        if let Some(paths) = package_paths {
            account.deep_inspection.package_paths = paths;
        }
        true
    }

    /// The enablement record of `account_id` to write through, created when nothing has
    /// been recorded for that account yet — and `None` once [`MAX_ENABLED_ACCOUNTS`]
    /// accounts are tracked and `account_id` is not one of them.
    ///
    /// The ONLY place an entry is added to [`Self::accounts`], so the bound on that map
    /// holds however many operations write to it: a caller looping any of them with fresh
    /// account ids cannot grow the map without bound. A caller that would add several
    /// accounts at once checks the whole batch against the bound BEFORE its first write,
    /// so a refused request stores nothing at all.
    pub fn account_mut(&mut self, account_id: &str) -> Option<&mut AccountEnablement> {
        if self.accounts.len() >= MAX_ENABLED_ACCOUNTS && !self.accounts.contains_key(account_id)
        {
            return None;
        }
        Some(self.accounts.entry(account_id.to_string()).or_default())
    }

    /// The aggregation rows the findings this service has recorded roll up to, for one
    /// `AggregationType` token.
    ///
    /// Amazon Inspector counts an aggregation over the findings its own scans produced:
    /// one row per key of the requested aggregation — an account, a repository, an image
    /// layer — and a key no finding falls under is left OUT of the answer rather than
    /// reported with zero counts. This store records account enablement and suppression
    /// filters, and no finding: Inspector mints a finding from a scan it runs itself and
    /// no operation of this API submits one, so nothing has recorded a finding for any
    /// account the store knows. The roll-up of that finding set is no rows at all — for
    /// every aggregation type, and whatever `accountIds` or aggregation-request filter a
    /// call carries, since an empty set has nothing for a filter to keep or drop. Rows
    /// appear here as soon as there are recorded findings to count.
    pub fn finding_aggregation(
        &self,
        _aggregation_type: &str,
    ) -> Vec<crate::ops::types::AggregationResponse> {
        Vec::new()
    }
}

/// The region this request is signed for, or [`DEFAULT_REGION`] when it is unsigned.
///
/// The SigV4 credential scope (`<akid>/<date>/<region>/<service>/aws4_request`) is the
/// only place an AWS request states its region, so every regional ARN this service mints
/// is built from it. The parsing itself is the vendored
/// `emulator_server::sigv4_credential_scope`, so no second scope parser exists; this
/// wrapper only supplies the default, and it lives per service because `vendor/` is
/// read-only here and a shared crate for four lines would be reached by one caller.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Encode the opaque `nextToken` cursor a paginated list operation hands out: the hex of
/// the LAST key of the page it follows, so the next call resumes at the first record
/// ordered after that key rather than at a position in a freshly built list, which stays
/// correct when records are added or removed between pages.
pub fn encode_cursor(key: &str) -> String {
    ::hex::encode(key.as_bytes())
}

/// The key an opaque cursor names, or `None` when `token` is not a cursor this service
/// minted. `is_key` decides what counts as a key of the records being listed — a filter
/// ARN, an account id — so an arbitrary even-length hex string is refused by the calling
/// operation rather than read as a position in its list.
pub fn decode_cursor(token: &str, is_key: impl Fn(&str) -> bool) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    is_key(&decoded).then_some(decoded)
}

/// Render a stored instant as the modelled `DateTimeTimestamp`.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsInspector2Inspector2State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsInspector2Inspector2State>)
}
