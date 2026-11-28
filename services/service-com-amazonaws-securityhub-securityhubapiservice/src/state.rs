// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-securityhub-securityhubapiservice service.
//!
//! Security Hub CSPM is subscribed per account **and** Region: `EnableSecurityHub`
//! creates the single `hub/default` resource for the caller's account/Region, and
//! `DisableSecurityHub` removes it together with the findings imported into it. The
//! state below therefore keys the hub on `(account id, Region)` and every finding on
//! `(account id, Region, ProductArn, Id)` — `ProductArn` + `Id` is the modelled
//! `AwsSecurityFindingIdentifier`, the identity `BatchUpdateFindings` addresses a
//! finding by, and the account/Region prefix is what makes "disable removes this
//! account's findings in this Region" expressible.
//!
//! A custom insight `CreateInsight` creates is keyed the same way — `(account id,
//! Region, InsightArn)` — because an insight is a saved query over exactly the
//! findings of the hub it was created in, so disabling the hub takes it with it.

use std::collections::{BTreeMap, HashMap};

use crate::ops::types::{
    AutomationRulesAction, AutomationRulesFindingFieldsUpdate, AutomationRulesFindingFilters,
    AwsSecurityFinding, BooleanFilter, DateFilter, DateRange, FindingHistoryUpdate, Member, Note,
    NoteUpdate, NumberFilter, Severity, Standard, StandardsControl, StandardsManagedBy,
    StandardsSubscription, StringFilter, Workflow,
};

/// The published security controls — the standard-agnostic control catalog and how a
/// control definition is reported — kept beside the per-standard control sets below
/// rather than inside them, because a security control belongs to no standard.
mod security_controls;

pub use security_controls::*;

/// What one account has CUSTOMIZED about a published security control — the parameter
/// values `UpdateSecurityControl` sets and `BatchGetSecurityControls` reports — kept
/// beside the published catalog rather than inside it, because the catalog is service
/// metadata identical for every caller while a customization belongs to one account's
/// hub.
mod security_control_parameters;

pub use security_control_parameters::*;

/// The hub V2 subscription — the resource the V2 API family is enabled and described
/// through — kept beside the CSPM hub below rather than inside it, because the two are
/// separate resources with separate ARNs and separate lifecycles.
mod hub_v2;

pub use hub_v2::*;

/// The organization's delegated administrator designations, kept beside the hub below
/// rather than inside it, because a designation belongs to the organization the
/// management account manages and not to any one account's subscription.
mod organization;

pub use organization::*;

/// The hub resource is one per account and Region, named `hub/default`.
pub type HubKey = (String, String);

/// A stored finding's identity: the hub it lives in, plus the modelled
/// `AwsSecurityFindingIdentifier` (`ProductArn` + `Id`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FindingKey {
    pub account_id: String,
    pub region: String,
    pub product_arn: String,
    pub finding_id: String,
}

/// A member association's identity: the administrator hub it belongs to (the
/// account/Region pair that made the `CreateMembers` call), plus the member account
/// id — the identity every member operation addresses an association by.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MemberKey {
    /// The administrator account the association belongs to.
    pub account_id: String,
    pub region: String,
    /// The member account id.
    pub member_account_id: String,
}

/// The member association `CreateMembers` creates and `GetMembers` reports.
#[derive(Debug, Clone)]
pub struct MemberAssociation {
    /// The email `AccountDetails$Email` carried, when the administrator supplied one.
    pub email: Option<String>,
    /// One of the documented `Member$MemberStatus` values.
    pub member_status: String,
    /// Set only once an invitation has been sent; `Member$InvitedAt` is "a timestamp
    /// for the date and time when the invitation was sent to the member account", so
    /// an association that was only created carries none.
    pub invited_at: Option<::aws_smithy_types::DateTime>,
    /// `Member$UpdatedAt`: when the member account was last updated.
    pub updated_at: ::aws_smithy_types::DateTime,
}

/// The `Hub` resource `EnableSecurityHub` creates and `DescribeHub` reports.
#[derive(Debug, Clone)]
pub struct Hub {
    pub hub_arn: String,
    /// ISO-8601 instant the account was subscribed; stable for the hub's lifetime.
    pub subscribed_at: String,
    pub auto_enable_controls: bool,
    pub control_finding_generator: String,
    pub enable_default_standards: bool,
    pub tags: HashMap<String, String>,
}

/// One finding change event, as `GetFindingHistory` reports it.
///
/// Appended by the two operations that write a finding — `BatchImportFindings` and
/// `BatchUpdateFindings`, the two values `FindingHistoryUpdateSourceType` models — so
/// the history is a record of writes that actually happened, never synthesized at
/// read time.
#[derive(Debug, Clone)]
pub struct FindingHistoryEvent {
    /// The instant the change was made, ISO-8601, as [`now_iso8601`] renders it.
    pub update_time: String,
    /// `True` only on the event that created the finding record.
    pub finding_created: bool,
    /// A `FindingHistoryUpdateSourceType`: which operation made the change.
    pub source_type: String,
    /// The identity that initiated the change: the product ARN for an import, the
    /// signing principal for an update.
    pub source_identity: String,
    /// The ASFF fields this event changed, with their old and new values.
    pub updates: Vec<FindingHistoryUpdate>,
    /// The write sequence of the change, the deterministic tie-breaker when two
    /// events carry the same `update_time`.
    pub seq: u64,
}

/// A custom action target's identity: the hub it lives in, plus the `Id` the
/// creator chose. `CreateActionTarget` documents `Id` as the identifier of the
/// custom action target, and the ARN it answers with is derived from it, so the two
/// cannot name different resources.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ActionTargetKey {
    pub account_id: String,
    pub region: String,
    pub target_id: String,
}

/// A custom action target as `CreateActionTarget` stored it and
/// `DescribeActionTargets` reports it (the modelled `ActionTarget` members).
#[derive(Debug, Clone)]
pub struct ActionTargetRecord {
    pub action_target_arn: String,
    pub name: String,
    pub description: String,
}

/// A finding as `BatchImportFindings` stored it, mutated in place by
/// `BatchUpdateFindings`.
#[derive(Debug, Clone)]
pub struct StoredFinding {
    pub finding: AwsSecurityFinding,
    /// Monotonic sequence of the import that first created this record.
    pub imported_seq: u64,
    /// Monotonic sequence of the most recent write, used as the deterministic
    /// tie-breaker when two findings carry the same `UpdatedAt`.
    pub updated_seq: u64,
    /// The change events this record has seen, oldest first and bounded by
    /// [`MAX_HISTORY_EVENTS_PER_FINDING`] / [`MAX_HISTORY_BYTES_PER_FINDING`];
    /// `GetFindingHistory` reports it. Append through
    /// [`StoredFinding::record_history`], never by pushing, so the bound holds.
    pub history: Vec<FindingHistoryEvent>,
    /// The text [`StoredFinding::history`] currently holds, kept as a running total
    /// so the byte bound costs nothing to enforce.
    pub history_bytes: usize,
}

/// How many change events one finding's history keeps. A history is append-only and
/// a caller may write the same finding without limit, so the store is bounded here:
/// the oldest events are dropped once it is full, which is also how the real service
/// ages a finding's history out. The bound is the emulator's own (AWS publishes a
/// retention window, not a count), chosen above `MAX_RESULTS_LIMIT` so a bounded
/// history still spans several pages.
pub const MAX_HISTORY_EVENTS_PER_FINDING: usize = 256;

/// How much event text one finding's history keeps. Each event carries the old and
/// new text of every field a write changed, which is caller-sized, so the count bound
/// alone does not bound memory: the oldest events are dropped until the retained text
/// fits this budget. The most recent event is always kept, however large it is, so a
/// write is never recorded as nothing.
pub const MAX_HISTORY_BYTES_PER_FINDING: usize = 128 * 1024;

/// The text one change event holds, as the byte bound counts it.
fn history_event_bytes(event: &FindingHistoryEvent) -> usize {
    let mut bytes = event.update_time.len() + event.source_type.len() + event.source_identity.len();
    for update in &event.updates {
        for value in [&update.updated_field, &update.old_value, &update.new_value] {
            bytes += value.as_ref().map_or(0, String::len);
        }
    }
    bytes
}

impl StoredFinding {
    /// A finding record as its creating import stores it, with that import as the
    /// first event of its history.
    pub fn new(finding: AwsSecurityFinding, seq: u64, created: FindingHistoryEvent) -> Self {
        let mut stored = Self {
            finding,
            imported_seq: seq,
            updated_seq: seq,
            history: Vec::new(),
            history_bytes: 0,
        };
        stored.record_history(created);
        stored
    }

    /// Append one change event, dropping the oldest events until the history is back
    /// inside both of its bounds. The newest event always survives, so the store is
    /// bounded without a write ever going unrecorded.
    pub fn record_history(&mut self, event: FindingHistoryEvent) {
        self.history_bytes += history_event_bytes(&event);
        self.history.push(event);
        while self.history.len() > 1
            && (self.history.len() > MAX_HISTORY_EVENTS_PER_FINDING
                || self.history_bytes > MAX_HISTORY_BYTES_PER_FINDING)
        {
            let dropped = self.history.remove(0);
            self.history_bytes = self
                .history_bytes
                .saturating_sub(history_event_bytes(&dropped));
        }
    }
}

/// A stored automation rule's identity: the hub it belongs to, plus the ARN
/// `CreateAutomationRule` minted for it. Automation rules are per account and
/// Region, exactly like the hub and the findings they act on.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AutomationRuleKey {
    pub account_id: String,
    pub region: String,
    pub rule_arn: String,
}

/// An automation rule as `CreateAutomationRule` stored it, mutated in place by
/// `BatchUpdateAutomationRules` and removed by `BatchDeleteAutomationRules`.
#[derive(Debug, Clone)]
pub struct AutomationRule {
    pub rule_arn: String,
    pub rule_status: String,
    pub rule_order: i32,
    pub rule_name: String,
    pub description: String,
    pub is_terminal: bool,
    pub criteria: AutomationRulesFindingFilters,
    pub actions: Vec<AutomationRulesAction>,
    pub created_at: ::aws_smithy_types::DateTime,
    pub updated_at: ::aws_smithy_types::DateTime,
    pub created_by: String,
    pub tags: HashMap<String, String>,
    /// Monotonic sequence of the create that stored this rule: the deterministic
    /// tie-breaker between two rules that carry the same `RuleOrder`.
    pub created_seq: u64,
}

/// A standards subscription's identity: the hub it belongs to plus the ARN of the
/// standard it subscribes to. One subscription per standard per account/Region, which
/// is what makes `BatchEnableStandards` of an already-enabled standard the same
/// resource rather than a second one.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StandardsSubscriptionKey {
    pub account_id: String,
    pub region: String,
    pub standards_arn: String,
}

/// An enabled standard: the `StandardsSubscription` resource `EnableSecurityHub`'s
/// default standards (and, once implemented, `BatchEnableStandards`) create and
/// `GetEnabledStandards` reports.
#[derive(Debug, Clone)]
pub struct StandardsSubscriptionRecord {
    pub standards_subscription_arn: String,
    /// The `StandardsInput` key-value pairs the subscription was created with.
    pub standards_input: HashMap<String, String>,
    /// A `StandardsStatus` value: `PENDING` until the enabling call's transition
    /// falls due, then `READY`; `DELETING` once a disabling call has started the
    /// removal it owes.
    pub standards_status: String,
    /// A `StandardsProvider` value — the cloud whose resources the standard evaluates.
    pub provider: String,
    /// Whole nanoseconds since the epoch at which the `PENDING` -> `READY`
    /// transition the enabling call owes becomes due. A reporting operation
    /// materialises it once the clock passes this instant.
    pub ready_due_nanos: i128,
    /// Whole nanoseconds since the epoch at which the removal a disabling call owes
    /// this subscription becomes due, or `None` while no disable has been asked for.
    /// A reporting operation drops the subscription once the clock passes this
    /// instant, which is what makes a disabled standard stop being enabled.
    pub removal_due_nanos: Option<i128>,
    /// The instant the standard was enabled, which is when every control of the
    /// subscription got the status it starts in — so a control nobody has touched
    /// reports this as its `ControlStatusUpdatedAt`, stable for as long as the
    /// subscription lives.
    pub enabled_at: ::aws_smithy_types::DateTime,
    /// The controls of this subscription whose status a caller has changed, keyed by
    /// the `ControlId` the standard addresses the control by. A control absent here
    /// still carries the status the enabling call gave it, so the map holds changes
    /// only; it belongs to the subscription because a control's enablement is a
    /// property of one account's subscription to one standard, and so ends with it.
    pub control_statuses: HashMap<String, StandardsControlStatus>,
}

/// The status of one control of one standards subscription, as `UpdateStandardsControl`
/// last set it and `DescribeStandardsControls` reports it.
#[derive(Debug, Clone)]
pub struct StandardsControlStatus {
    /// A `ControlStatus` value (`ENABLED` or `DISABLED`).
    pub control_status: String,
    /// `StandardsControl$DisabledReason`: "The reason provided for the most recent
    /// change in status for the control", which only a disable carries.
    pub disabled_reason: Option<String>,
    /// When this status was set.
    pub control_status_updated_at: ::aws_smithy_types::DateTime,
}

/// Per-service state for com-amazonaws-securityhub-securityhubapiservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSecurityhubSecurityhubapiserviceState {
    /// Finding aggregators, keyed by account id: cross-Region aggregation is one
    /// account-wide setting, so an account holds at most one and
    /// `ListFindingAggregators` answers the same ARN in every Region.
    pub finding_aggregators: HashMap<String, StoredFindingAggregator>,
    /// Custom insights, keyed by hub + the ARN `CreateInsight` minted.
    pub insights: HashMap<InsightKey, StoredInsight>,
    /// The subscribed hubs, one per `(account id, Region)`.
    pub hubs: HashMap<HubKey, Hub>,
    /// The hub V2 subscriptions, one per `(account id, Region)`: the resource the V2
    /// API family is enabled through, independent of the CSPM hub above.
    pub hubs_v2: HashMap<HubKey, HubV2>,
    /// Imported findings, keyed by hub + `AwsSecurityFindingIdentifier`.
    pub findings: HashMap<FindingKey, StoredFinding>,
    /// Member associations, keyed by administrator account/Region + member account id.
    pub members: HashMap<MemberKey, MemberAssociation>,
    /// Custom action targets, keyed by hub + the creator-chosen `Id`.
    pub action_targets: HashMap<ActionTargetKey, ActionTargetRecord>,
    /// The automation rules of each hub, keyed by hub + minted rule ARN.
    pub automation_rules: HashMap<AutomationRuleKey, AutomationRule>,
    /// The enabled standards, keyed by hub + standard ARN.
    pub standards: HashMap<StandardsSubscriptionKey, StandardsSubscriptionRecord>,
    /// The organization's delegated administrator designations, keyed by Region +
    /// `SecurityHubFeature` — the identity [`OrganizationAdminKey`] describes.
    pub organization_admins: HashMap<OrganizationAdminKey, OrganizationAdminRecord>,
    /// What each hub has customized about a published security control, keyed by hub +
    /// `SecurityControlId`: a control's custom parameter values are per account and
    /// Region, exactly like the hub whose findings the control's checks produce.
    pub security_control_customizations:
        HashMap<SecurityControlCustomizationKey, SecurityControlCustomization>,
    /// Source of [`StoredFinding::imported_seq`] / [`StoredFinding::updated_seq`]
    /// and of [`AutomationRule::created_seq`].
    pub write_seq: u64,
}

impl ComAmazonawsSecurityhubSecurityhubapiserviceState {
    /// The next monotonic write sequence. Never zero, so a stored value is always
    /// distinguishable from `Default`.
    pub fn next_write_seq(&mut self) -> u64 {
        self.write_seq += 1;
        self.write_seq
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSecurityhubSecurityhubapiserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSecurityhubSecurityhubapiserviceState>)
}

/// The ARN namespace Security Hub signs and ARNs with (`awsService.arnNamespace`
/// / `sigv4Name` in this service's Smithy model).
pub const ARN_NAMESPACE: &str = "securityhub";

/// The message a handler answers with when this service's store is unreachable,
/// kept in one place so every operation refuses identically.
pub const STATE_UNAVAILABLE_MESSAGE: &str = "the Security Hub CSPM state store is unavailable";

/// The message a hub-scoped operation refuses a caller with when the account is not
/// subscribed to Security Hub CSPM in the request's Region.
///
/// The caller's account id is the ONLY variable part: the subscription is per
/// account/Region, but the refusal names just the account, with no Region clause, and
/// spells the product with its full name.
///
/// Only a HUB-SCOPED operation — one that reads or writes this account's hub,
/// findings, insights, action targets, members or standards subscriptions — has that
/// precondition. An operation that reports the service's own published metadata (the
/// standards catalog, the product catalog, the security-control definitions) answers
/// every caller, subscribed or not: those are the reads a client makes to discover
/// what it can enable, before any hub exists.
pub fn not_subscribed_message(account_id: &str) -> String {
    format!("Account {account_id} is not subscribed to AWS Security Hub CSPM")
}

/// The whole message `EnableSecurityHub` refuses a second enable of an already
/// subscribed account/Region with. Security Hub states the precondition that failed
/// and nothing else: the message interpolates no account id and no Region, so a
/// caller that enables-if-not-enabled can match it as a constant.
pub const ALREADY_SUBSCRIBED_MESSAGE: &str = "Account is already subscribed to Security Hub";

/// The whole message an operation that CONFIGURES the hub resource is refused with
/// when the calling account/Region holds no subscription. Security Hub states the
/// failed precondition on the subscription itself and nothing else — the mirror image
/// of [`ALREADY_SUBSCRIBED_MESSAGE`] — so the message interpolates no account id and
/// no Region and a caller can match it as a constant.
///
/// Distinct from [`not_subscribed_message`], which is what an operation that works
/// INSIDE the hub (findings, insights, standards, members) refuses with.
pub const HUB_NOT_SUBSCRIBED_MESSAGE: &str = "Account is not subscribed to Security Hub";

/// The message an operation addressed by an ARN refuses with when that ARN names no
/// resource of the caller's hub: the ARN the request carried, quoted back as it
/// arrived, and the bare statement that nothing answers to it.
pub fn arn_does_not_exist_message(arn: &str) -> String {
    format!("Given Arn: {arn} does not exist")
}

/// The whole message an INSIGHT-addressed call is refused with when its `InsightArn`
/// resolves to no insight of the caller's hub. Security Hub CSPM states only that no
/// insight answers to the ARN: the message interpolates nothing from the request — not
/// the ARN, not the account and not the Region — so a caller can match it as a
/// constant.
pub const INSIGHT_NOT_FOUND_MESSAGE: &str = "Cannot find any insight for the given Arn";

/// The message `CreateActionTarget` refuses a duplicate with: the conflict is keyed on
/// the `Id` the client chose, which is the only name it holds at that point, not on
/// the ARN the first create derived from it.
pub fn action_target_id_conflict_message(id: &str) -> String {
    format!("Action with Id: {id} already exists")
}

/// The whole message `UpdateStandardsControl` refuses a disable that names no reason
/// with. `DisabledReason` is "A description of the reason why you are disabling a
/// security standard control. If you are disabling a control, then this is required"
/// (`smithy_query(query="doc", shape="UpdateStandardsControlRequest",
/// member="DisabledReason")`), and the refusal states that precondition followed by
/// the control the request named, quoted back as it arrived.
pub fn disabled_reason_required_message(standards_control_arn: &str) -> String {
    format!("DisabledReason must be given when disabling control: {standards_control_arn}")
}

/// The message `BatchEnableStandards` refuses a batch with when one or more of its
/// `StandardsSubscriptionRequests` name a standard that cannot be enabled: the fixed
/// text followed by the rejected requests as a JSON array, each entry carrying the
/// lower-camel `standardsArn` member with the value the request supplied. Every
/// rejected entry of the batch is listed, in request order, so a caller can retry
/// with the entries that were not rejected.
pub fn invalid_standards_subscription_requests_message(standards_arns: &[String]) -> String {
    let rejected = ::serde_json::Value::Array(
        standards_arns
            .iter()
            .map(|arn| ::serde_json::json!({ "standardsArn": arn }))
            .collect(),
    );
    format!("Invalid StandardsSubscriptionRequest(s): {rejected}")
}

/// How many custom action targets one hub (`(account, Region)`) may hold. The AWS
/// Security Hub quotas table publishes 100 custom actions per account per Region, and
/// `CreateActionTarget` models `LimitExceededException` for a create "beyond the
/// current Amazon Web Services account … limits"; the bound is also what keeps this
/// store from growing with the number of calls a caller makes.
pub const MAX_ACTION_TARGETS_PER_HUB: usize = 100;

/// The longest `Description` this store keeps for a custom action target. The model
/// documents no bound for that member, so the cap mirrors the one this crate already
/// applies to a finding's `Description` (`MAX_DESCRIPTION_LEN` in
/// `ops/batch_import_findings.rs`): one record's size must not grow with whatever a
/// caller sends.
pub const MAX_ACTION_TARGET_DESCRIPTION_LEN: usize = 1024;

/// How many member associations one hub (`(account, Region)`) may hold. The AWS
/// Security Hub quotas table publishes 1,000 member accounts per administrator
/// account, and `CreateMembers` models `LimitExceededException` for a create "beyond
/// the current Amazon Web Services account … limits"; the bound is also what keeps
/// this store from growing with the number of calls a caller makes.
pub const MAX_MEMBERS_PER_HUB: usize = 1_000;

/// The longest `Email` this store keeps for a member association. The model
/// documents no bound for `AccountDetails$Email`, so the cap is the longest address
/// SMTP itself allows (RFC 5321 §4.5.3.1.3 bounds a forward path at 256 octets
/// including the angle brackets, so 254 characters of address): one record's size
/// must not grow with whatever a caller sends.
pub const MAX_MEMBER_EMAIL_LEN: usize = 254;

/// How many accounts one member operation may address in a single call: the bound
/// the model already puts on `CreateMembers`' own batch (`AccountDetailsList`, "len
/// 1..50"), applied to every member operation's account list so neither the work one
/// call does under the state lock nor the response it produces grows with what a
/// caller sends. `GetMembers`' `AccountIdList` carries no `length` trait, so its
/// handler enforces this.
pub const MAX_MEMBER_BATCH: usize = 50;

/// A `NextToken` resuming a list at `offset`, carrying `prefix` so a token minted for
/// another list — or a value a caller invented — is rejected rather than read as an
/// offset. Opaque to the caller: the cursor is hex-encoded, so nothing in the
/// response invites arithmetic on it.
pub fn encode_list_token(prefix: &str, offset: usize) -> String {
    ::hex::encode(format!("{prefix}{offset}"))
}

/// The offset a `NextToken` resumes at, or `None` when it is not one the list named
/// by `prefix` minted.
pub fn decode_list_token(prefix: &str, token: &str) -> Option<usize> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    decoded.strip_prefix(prefix)?.parse().ok()
}

/// The hub resource's fixed name: Security Hub CSPM creates exactly one hub per
/// account/Region and calls it `default`.
pub const HUB_RESOURCE: &str = "hub/default";

/// `EnableSecurityHub`'s documented default for `ControlFindingGenerator`: "For
/// accounts that aren't part of an organization, the default value of this field is
/// `SECURITY_CONTROL` if you enabled Security Hub CSPM on or after February 23,
/// 2023" (`smithy_query(query="doc", shape="EnableSecurityHubRequest",
/// member="ControlFindingGenerator")`). The emulator's clock is always after that
/// date, so this is the default for an omitted member.
pub const DEFAULT_CONTROL_FINDING_GENERATOR: &str = "SECURITY_CONTROL";

/// `EnableDefaultStandards`'s documented default: "If you don't provide a value for
/// `EnableDefaultStandards`, it is set to true."
pub const DEFAULT_ENABLE_DEFAULT_STANDARDS: bool = true;

/// A newly subscribed hub enables new controls of enabled standards automatically;
/// `UpdateSecurityHubConfiguration` is what turns it off, so a fresh hub reports
/// `true`.
pub const DEFAULT_AUTO_ENABLE_CONTROLS: bool = true;

/// One standard Security Hub CSPM supports, as the AWS Security Hub user guide's
/// "Security Hub standards reference" publishes it: the slug and version that make up
/// its ARNs, the display name and description `DescribeStandards` reports, the
/// `StandardsProvider` whose resources it evaluates, and whether `EnableSecurityHub`
/// enables it by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StandardDefinition {
    /// The `<slug>` in `standards/<slug>/v/<version>`.
    pub slug: &'static str,
    /// The `<version>` in `standards/<slug>/v/<version>`.
    pub version: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// A `StandardsProvider` value, as `STANDARDS_PROVIDER_VALUES` lists them.
    pub provider: &'static str,
    /// `Standard$EnabledByDefault`: "When Security Hub CSPM is enabled using the
    /// `EnableSecurityHub` API operation, the standard is enabled by default unless
    /// `EnableDefaultStandards` is set to `false`" (`smithy_query(query="shape",
    /// shape="Standard")`, member `EnabledByDefault`).
    pub enabled_by_default: bool,
    /// The CIS AWS Foundations Benchmark v1.2.0 standard predates the per-Region
    /// `standards/` ARN form and keeps its partition-wide `arn:aws:securityhub:::ruleset/...`
    /// ARN; every other standard's ARN carries the Region.
    pub partition_wide_ruleset: bool,
}

/// The standards this emulator knows, from the AWS Security Hub user guide's
/// "Security Hub standards reference" (names, ARNs, descriptions, the evaluated cloud
/// provider and the two standards enabled by default). `GetEnabledStandards` reports
/// the subscriptions created from these, so a standard absent here is one no operation
/// can enable.
///
/// Every entry is one managed standard of that published catalog, carrying the
/// catalog's own wording: the FULL published name of the standard (the catalog spells
/// a standard out — "NIST Special Publication 800-53 Revision 5" — rather than
/// abbreviating it, and a name is what `DescribeStandards` callers key the catalog on)
/// and the catalog's description text for it, copied rather than summarised, with that
/// entry's own typography preserved (non-ASCII punctuation such as U+2019 included).
/// A paraphrase of either is a divergence, because a client reads a standard out of
/// this catalog before it drives `BatchEnableStandards` from the entry, and anything
/// that surfaces the catalog renders this text.
///
/// The text is per entry, not generated from a family template: entries of one family
/// differ in whether the catalog expands an acronym (the NIST entries do not, the PCI
/// and CIS entries do), in "checks your" versus "checks for your", in how the
/// requirement subset is named (the four CIS AWS benchmark revisions all say "a subset
/// of CIS requirements" while the two PCI DSS revisions disagree with each other — the
/// older says "a subset of PCI DSS requirements", the newer "a subset of PCI DSS
/// V4.0.1 requirements"), and in the typography of a possessive. So each description is
/// copied from the catalog for that standard, never derived from a neighbour's by
/// substituting a version token.
///
/// Ordered by `Name`, which is the order `DescribeStandards` reports the catalog in,
/// so a page boundary falls in the same place on every call.
pub const STANDARDS_CATALOG: [StandardDefinition; 13] = [
    StandardDefinition {
        slug: "ai-security-best-practices",
        version: "1.0.0",
        name: "AI Security Best Practices v1.0.0",
        description: "The AI Security Best Practices standard is a set of automated security checks that detect when deployed AI resources do not align with security best practices. The standard is defined by AWS security experts. This curated set of controls helps improve your security posture for most popular AI services.",
        provider: "AWS",
        enabled_by_default: false,
        partition_wide_ruleset: false,
    },
    StandardDefinition {
        slug: "aws-foundational-security-best-practices",
        version: "1.0.0",
        name: "AWS Foundational Security Best Practices v1.0.0",
        // The catalog text carries AWS's own typography: the possessive in "AWS’s"
        // is the U+2019 RIGHT SINGLE QUOTATION MARK, not an ASCII apostrophe.
        description: "The AWS Foundational Security Best Practices standard is a set of automated security checks that detect when AWS accounts and deployed resources do not align to security best practices. The standard is defined by AWS security experts. This curated set of controls helps improve your security posture in AWS, and cover AWS\u{2019}s most popular and foundational services.",
        provider: "AWS",
        enabled_by_default: true,
        partition_wide_ruleset: false,
    },
    StandardDefinition {
        slug: "aws-resource-tagging-standard",
        version: "1.0.0",
        name: "AWS Resource Tagging Standard v1.0.0",
        description: "The AWS Resource Tagging standard is a set of automated security checks that detect when AWS resources are not tagged.",
        provider: "AWS",
        enabled_by_default: false,
        partition_wide_ruleset: false,
    },
    StandardDefinition {
        slug: "azure-foundational-security-best-practices",
        version: "1.0.0",
        name: "Azure Foundational Security Best Practices v1.0.0",
        // The catalog's own text for this standard, which follows the AWS
        // Foundational Security Best Practices entry's wording rather than the AI
        // standard's — but NOT its typography: this entry's possessive "Azure's" is
        // a plain ASCII apostrophe where the AWS entry's "AWS\u{2019}s" is U+2019, so
        // typography is per entry and not a catalog-wide rule.
        description: "The Azure Foundational Security Best Practices standard is a set of automated security checks that detect when Azure resources do not align to security best practices. The standard is defined by AWS security experts. This curated set of controls helps improve your security posture in Azure, and cover Azure's most popular and foundational services.",
        provider: "Azure",
        enabled_by_default: false,
        partition_wide_ruleset: false,
    },
    StandardDefinition {
        slug: "cis-aws-foundations-benchmark",
        version: "1.2.0",
        name: "CIS AWS Foundations Benchmark v1.2.0",
        description: "The Center for Internet Security (CIS) AWS Foundations Benchmark v1.2.0 is a set of security configuration best practices for AWS. This Security Hub standard automatically checks for your compliance readiness against a subset of CIS requirements.",
        provider: "AWS",
        enabled_by_default: true,
        // The recorded catalog reports this standard with the partition-wide
        // `ruleset/` ARN that predates the per-Region `standards/` form; every other
        // standard carries the Region.
        partition_wide_ruleset: true,
    },
    StandardDefinition {
        slug: "cis-aws-foundations-benchmark",
        version: "1.4.0",
        name: "CIS AWS Foundations Benchmark v1.4.0",
        description: "The Center for Internet Security (CIS) AWS Foundations Benchmark v1.4.0 is a set of security configuration best practices for AWS. This Security Hub standard automatically checks for your compliance readiness against a subset of CIS requirements.",
        provider: "AWS",
        enabled_by_default: false,
        partition_wide_ruleset: false,
    },
    StandardDefinition {
        slug: "cis-aws-foundations-benchmark",
        version: "3.0.0",
        name: "CIS AWS Foundations Benchmark v3.0.0",
        description: "The Center for Internet Security (CIS) AWS Foundations Benchmark v3.0.0 is a set of security configuration best practices for AWS. This Security Hub standard automatically checks for your compliance readiness against a subset of CIS requirements.",
        provider: "AWS",
        enabled_by_default: false,
        partition_wide_ruleset: false,
    },
    StandardDefinition {
        slug: "cis-aws-foundations-benchmark",
        version: "5.0.0",
        name: "CIS AWS Foundations Benchmark v5.0.0",
        description: "The Center for Internet Security (CIS) AWS Foundations Benchmark v5.0.0 is a set of security configuration best practices for AWS. This Security Hub standard automatically checks for your compliance readiness against a subset of CIS requirements.",
        provider: "AWS",
        enabled_by_default: false,
        partition_wide_ruleset: false,
    },
    StandardDefinition {
        slug: "cis-azure-foundations-benchmark",
        version: "4.0.0",
        name: "CIS Azure Foundations Benchmark v4.0.0",
        // The catalog spells this benchmark's provider out as "Microsoft Azure" and
        // says "checks your compliance readiness", where the older CIS AWS entries
        // above say "checks for your": the text is per entry, not generated from a
        // family template.
        description: "The Center for Internet Security (CIS) Microsoft Azure Foundations Benchmark v4.0.0 is a set of security configuration best practices for Azure. This Security Hub standard automatically checks your compliance readiness against a subset of CIS requirements.",
        provider: "Azure",
        enabled_by_default: false,
        partition_wide_ruleset: false,
    },
    StandardDefinition {
        slug: "nist-800-171",
        version: "2.0.0",
        name: "NIST Special Publication 800-171 Revision 2",
        // The catalog opens with the publication's name as published (it does not
        // expand the NIST acronym), capitalises the defined term "Controlled
        // Unclassified Information", and names the subset in the catalog's short
        // form "NIST 800-171 R2".
        description: "NIST Special Publication 800-171 Revision 2 provides recommended security requirements for protecting the confidentiality of Controlled Unclassified Information. This Security Hub standard automatically checks for your compliance readiness against a subset of NIST 800-171 R2 requirements.",
        provider: "AWS",
        enabled_by_default: false,
        partition_wide_ruleset: false,
    },
    StandardDefinition {
        slug: "nist-800-53",
        version: "5.0.0",
        name: "NIST Special Publication 800-53 Revision 5",
        // Same catalog wording as the 800-171 entry above: the publication's name
        // unexpanded, the publication's own summary of what it provides, and the
        // subset named in the catalog's short form "NIST 800-53 R5".
        description: "NIST Special Publication 800-53 Revision 5 provides a catalog of security and privacy controls for information systems and organizations. This Security Hub standard automatically checks for your compliance readiness against a subset of NIST 800-53 R5 requirements.",
        provider: "AWS",
        enabled_by_default: false,
        partition_wide_ruleset: false,
    },
    StandardDefinition {
        slug: "pci-dss",
        version: "3.2.1",
        name: "PCI DSS v3.2.1",
        // This revision's catalog text names the requirement subset without a version
        // ("a subset of PCI DSS requirements"), unlike the v4.0.1 entry below: the
        // recorded catalog confirms both, so neither is derived from the other.
        description: "The Payment Card Industry Data Security Standard (PCI DSS) v3.2.1 is an information security standard for entities that store, process, and/or transmit cardholder data. This Security Hub standard automatically checks for your compliance readiness against a subset of PCI DSS requirements.",
        provider: "AWS",
        enabled_by_default: false,
        partition_wide_ruleset: false,
    },
    StandardDefinition {
        slug: "pci-dss",
        version: "4.0.1",
        name: "PCI DSS v4.0.1",
        // This revision's OWN published catalog text, which is not the v3.2.1 entry's
        // sentence with the version swapped: where the v3.2.1 entry names the
        // requirement subset "a subset of PCI DSS requirements", this entry qualifies
        // it with the revision — "a subset of PCI DSS V4.0.1 requirements", with the
        // catalog's own capital "V" there beside the lowercase "v4.0.1" of the opening
        // clause. One more reason a description is copied per entry and never derived
        // from a sibling revision's.
        description: "The Payment Card Industry Data Security Standard (PCI DSS) v4.0.1 is an information security standard for entities that store, process, and/or transmit cardholder data. This Security Hub standard automatically checks for your compliance readiness against a subset of PCI DSS V4.0.1 requirements.",
        provider: "AWS",
        enabled_by_default: false,
        partition_wide_ruleset: false,
    },
];

/// `StandardsStatus$PENDING`: "The standard is in the process of being enabled"
/// (`smithy_query(query="shape", shape="StandardsSubscription")`, member
/// `StandardsStatus`).
pub const STANDARDS_STATUS_PENDING: &str = "PENDING";

/// `StandardsStatus$READY`: "The standard is enabled."
pub const STANDARDS_STATUS_READY: &str = "READY";

/// `StandardsStatus$DELETING`: "The standard is in the process of being disabled"
/// (`smithy_query(query="shape", shape="StandardsSubscription")`, member
/// `StandardsStatus`) — the status a disabling call puts a subscription in until the
/// removal it owes falls due.
pub const STANDARDS_STATUS_DELETING: &str = "DELETING";

/// `StandardsControlsUpdatable$NOT_READY_FOR_UPDATES`: "Controls in the standard
/// cannot be retrieved or configured" — what a subscription still being enabled
/// reports.
pub const STANDARDS_CONTROLS_NOT_READY: &str = "NOT_READY_FOR_UPDATES";

/// `StandardsControlsUpdatable$READY_FOR_UPDATES`: "Controls in the standard can be
/// retrieved and configured."
pub const STANDARDS_CONTROLS_READY: &str = "READY_FOR_UPDATES";

/// How long after the enabling call a standards subscription's `PENDING` -> `READY`
/// transition falls due. AWS publishes no duration for it, so this is a chosen span
/// inside the emulator's 250-750 ms band: short enough not to stall a caller, long
/// enough that a caller reading straight back still observes `PENDING`.
pub const STANDARDS_READY_AFTER_NANOS: i128 = 500_000_000;

/// How long after the disabling call a standards subscription's `DELETING` ->
/// removed transition falls due. AWS publishes no duration for it, so this is a
/// chosen span: long enough that a caller reading straight back still observes the
/// `DELETING` the call owes, short enough not to stall one.
pub const STANDARDS_REMOVED_AFTER_NANOS: i128 = 3_000_000_000;

/// The ARN of a standard itself. AWS's standard ARNs carry no account (and, for the
/// one partition-wide ruleset, no Region either), so they are not built through
/// `engine::emulator_arn`, which always stamps the emulator account.
pub fn standard_arn(region: &str, definition: &StandardDefinition) -> String {
    let (slug, version) = (definition.slug, definition.version);
    if definition.partition_wide_ruleset {
        format!("arn:aws:{ARN_NAMESPACE}:::ruleset/{slug}/v/{version}")
    } else {
        format!("arn:aws:{ARN_NAMESPACE}:{region}::standards/{slug}/v/{version}")
    }
}

/// The ARN of the caller's subscription to a standard:
/// `arn:aws:securityhub:<region>:<account>:subscription/<slug>/v/<version>`.
pub fn standards_subscription_arn(key: &HubKey, definition: &StandardDefinition) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        &key.1,
        &format!(
            "subscription/{}/v/{}",
            definition.slug, definition.version
        ),
    )
}

/// The subscription an enabling call creates for one standard: `PENDING` now, with
/// the `READY` transition it owes due [`STANDARDS_READY_AFTER_NANOS`] later.
pub fn new_standards_subscription(
    key: &HubKey,
    definition: &StandardDefinition,
    standards_input: HashMap<String, String>,
) -> (StandardsSubscriptionKey, StandardsSubscriptionRecord) {
    (
        StandardsSubscriptionKey {
            account_id: key.0.clone(),
            region: key.1.clone(),
            standards_arn: standard_arn(&key.1, definition),
        },
        StandardsSubscriptionRecord {
            standards_subscription_arn: standards_subscription_arn(key, definition),
            standards_input,
            standards_status: STANDARDS_STATUS_PENDING.to_string(),
            provider: definition.provider.to_string(),
            ready_due_nanos: now_nanos() + STANDARDS_READY_AFTER_NANOS,
            // Nothing has asked for this subscription to go away.
            removal_due_nanos: None,
            enabled_at: now_date_time(),
            // Enabling a standard enables its controls, so no control carries a
            // changed status until a caller changes one.
            control_statuses: HashMap::new(),
        },
    )
}

/// Materialise, for one hub, every standards-subscription transition the clock has
/// already made due: a `PENDING` subscription whose due instant has passed is
/// `READY`, and a `DELETING` one whose removal fell due is gone — whether or not
/// anybody called. A reporting operation calls this before it reads, so the status it
/// reports, and the set of subscriptions it reports at all, is what the clock owes.
pub fn settle_standards(
    store: &mut ComAmazonawsSecurityhubSecurityhubapiserviceState,
    key: &HubKey,
) {
    let now = now_nanos();
    store.standards.retain(|subscription_key, record| {
        if subscription_key.account_id != key.0 || subscription_key.region != key.1 {
            return true;
        }
        if let Some(due) = record.removal_due_nanos {
            // A disabling call owes the subscription's removal; once that instant has
            // passed the account no longer subscribes to the standard, so the record
            // — and the per-control status it carried — goes with it.
            return now < due;
        }
        if record.standards_status == STANDARDS_STATUS_PENDING && now >= record.ready_due_nanos {
            record.standards_status = STANDARDS_STATUS_READY.to_string();
        }
        true
    });
}

/// Start the removal a disabling call owes one subscription: it reports `DELETING`
/// until [`STANDARDS_REMOVED_AFTER_NANOS`] has passed, when [`settle_standards`]
/// drops it. Idempotent — a second disable of the same subscription neither restarts
/// the span nor changes what the first one owes.
pub fn begin_standards_subscription_removal(record: &mut StandardsSubscriptionRecord) {
    if record.removal_due_nanos.is_some() {
        return;
    }
    record.standards_status = STANDARDS_STATUS_DELETING.to_string();
    record.removal_due_nanos = Some(now_nanos() + STANDARDS_REMOVED_AFTER_NANOS);
}

/// The key of the subscription of hub `key` whose `StandardsSubscriptionArn` is
/// `standards_subscription_arn`, or `None` when that hub has no such subscription.
/// The single definition of how a subscription-ARN-addressed operation resolves the
/// resource it was given.
pub fn standards_subscription_by_arn(
    store: &ComAmazonawsSecurityhubSecurityhubapiserviceState,
    key: &HubKey,
    standards_subscription_arn: &str,
) -> Option<StandardsSubscriptionKey> {
    store
        .standards
        .iter()
        .find(|(subscription_key, record)| {
            subscription_key.account_id == key.0
                && subscription_key.region == key.1
                && record.standards_subscription_arn == standards_subscription_arn
        })
        .map(|(subscription_key, _)| subscription_key.clone())
}

/// The published standard a `StandardsArn` names for a request in `region`, or `None`
/// when no standard of the catalog carries that ARN there. The single definition of
/// which `StandardsArn` values an account can enable, so every operation that takes
/// one resolves it the same way.
pub fn standard_by_arn(region: &str, standards_arn: &str) -> Option<StandardDefinition> {
    STANDARDS_CATALOG
        .into_iter()
        .find(|definition| standard_arn(region, definition) == standards_arn)
}

/// One stored subscription as the modelled `StandardsSubscription` every operation
/// that reports a subscription answers with, so `BatchEnableStandards` and
/// `GetEnabledStandards` cannot describe the same resource differently.
pub fn standards_subscription_view(
    key: &StandardsSubscriptionKey,
    record: &StandardsSubscriptionRecord,
) -> StandardsSubscription {
    StandardsSubscription {
        standards_subscription_arn: record.standards_subscription_arn.clone(),
        standards_arn: key.standards_arn.clone(),
        standards_input: record.standards_input.clone(),
        standards_status: record.standards_status.clone(),
        standards_controls_updatable: Some(
            standards_controls_updatable(&record.standards_status).to_string(),
        ),
        // `StandardsStatusReason` reports why a standard could not be enabled or
        // disabled; a subscription that is enabling or enabled has no reason to
        // report.
        standards_status_reason: None,
        provider: Some(record.provider.clone()),
    }
}

/// Whether controls of a subscription in this `StandardsStatus` can be retrieved and
/// configured: only an enabled (`READY`) standard exposes its controls.
pub fn standards_controls_updatable(standards_status: &str) -> &'static str {
    if standards_status == STANDARDS_STATUS_READY {
        STANDARDS_CONTROLS_READY
    } else {
        STANDARDS_CONTROLS_NOT_READY
    }
}

/// `ControlStatus$ENABLED`: the control is checked. Every control of a standard is
/// enabled when the standard is enabled, so this is the status a control carries
/// until a caller disables it (`smithy_query(query="shape", shape="ControlStatus")`).
pub const CONTROL_STATUS_ENABLED: &str = "ENABLED";

/// `ControlStatus$DISABLED`: "Security Hub CSPM does not check against disabled
/// controls" (`smithy_query(query="shape", shape="StandardsControl")`, member
/// `ControlStatus`).
pub const CONTROL_STATUS_DISABLED: &str = "DISABLED";

/// One control of a published standard, as the AWS Security Hub user guide's
/// "Security Hub controls reference" publishes it FOR THAT STANDARD: the `ControlId`
/// that standard addresses the control by, the title and description that standard
/// publishes for it, the severity of the findings it generates and the requirements of
/// the standard's own framework it is related to.
///
/// The text is per standard, not per control: a standard that predates consolidated
/// control findings (the CIS benchmark and PCI DSS entries below) publishes each
/// control under its own numbered requirement wording, where a standard built on the
/// security controls themselves publishes the control's own title.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StandardsControlDefinition {
    /// `StandardsControl$ControlId`: "The identifier of the security standard
    /// control" — the last segment of the control's ARN.
    pub control_id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    /// A `SeverityRating` value (`smithy_query(query="shape",
    /// shape="SeverityRating")`: `LOW`, `MEDIUM`, `HIGH`, `CRITICAL`).
    pub severity_rating: &'static str,
    /// `StandardsControl$RelatedRequirements`: "The list of requirements that are
    /// related to this control", in the numbering of the standard's own framework.
    /// Empty for a standard that publishes none, which reports the member as absent
    /// rather than as an empty list.
    pub related_requirements: &'static [&'static str],
}

/// The controls one published standard carries, keyed by the `slug` and `version` of
/// that standard's [`StandardDefinition`].
#[derive(Debug, Clone, Copy)]
pub struct StandardControlSet {
    pub slug: &'static str,
    pub version: &'static str,
    /// In the order the controls reference publishes them, which is the order
    /// `DescribeStandardsControls` reports them in, so a page boundary falls in the
    /// same place on every call.
    pub controls: &'static [StandardsControlDefinition],
}

// The security controls this emulator publishes, from the AWS Security Hub user
// guide's "Security Hub controls reference": one entry per control, carrying the
// control's own id, the title and description the reference publishes for it and the
// severity of the findings it generates. A standard built on the security controls
// themselves addresses each one by this id, so these entries are shared by every such
// standard's control set below rather than restated per standard.

const ACM_1: StandardsControlDefinition = StandardsControlDefinition {
    control_id: "ACM.1",
    title: "Imported and ACM-issued certificates should be renewed after a specified time period",
    description: "This control checks whether an AWS Certificate Manager (ACM) certificate is renewed within the specified time period. It checks both imported certificates and certificates provided by ACM. The control fails if the certificate isn't renewed within the specified time period. Unless you provide a custom parameter value for the renewal period, Security Hub uses a default value of 30 days.",
    severity_rating: "MEDIUM",
    related_requirements: &[],
};

const ACM_2: StandardsControlDefinition = StandardsControlDefinition {
    control_id: "ACM.2",
    title: "RSA certificates managed by ACM should use a key length of at least 2,048 bits",
    description: "This control checks whether RSA certificates managed by AWS Certificate Manager use a key length of at least 2,048 bits. The control fails if the key length is smaller than 2,048 bits.",
    severity_rating: "HIGH",
    related_requirements: &[],
};

const ACM_3: StandardsControlDefinition = StandardsControlDefinition {
    control_id: "ACM.3",
    title: "ACM certificates should be tagged",
    description: "This control checks whether an AWS Certificate Manager (ACM) certificate has tags with the specific keys defined in the parameter requiredTagKeys. The control fails if the certificate doesn't have any tag keys or if it doesn't have all the keys specified in the parameter requiredTagKeys. If the parameter requiredTagKeys isn't provided, the control only checks for the existence of a tag key and fails if the certificate isn't tagged with any key.",
    severity_rating: "LOW",
    related_requirements: &[],
};

const CLOUDTRAIL_1: StandardsControlDefinition = StandardsControlDefinition {
    control_id: "CloudTrail.1",
    title: "CloudTrail should be enabled and configured with at least one multi-Region trail that includes read and write management events",
    description: "This control checks whether an AWS CloudTrail trail is enabled in your account and whether at least one trail is a multi-Region trail that records both read and write management events. The control fails if no trail is enabled, or if no enabled trail is a multi-Region trail that records read and write management events.",
    severity_rating: "HIGH",
    related_requirements: &[],
};

const CLOUDTRAIL_2: StandardsControlDefinition = StandardsControlDefinition {
    control_id: "CloudTrail.2",
    title: "CloudTrail should have encryption at-rest enabled",
    description: "This control checks whether an AWS CloudTrail trail is configured to use server-side encryption with an AWS KMS key. The control fails if the KmsKeyId field of the trail is empty. Server-side encryption with an AWS KMS key adds a layer of protection to the sensitive information a trail's log files carry.",
    severity_rating: "MEDIUM",
    related_requirements: &[],
};

const CLOUDWATCH_1: StandardsControlDefinition = StandardsControlDefinition {
    control_id: "CloudWatch.1",
    title: "A log metric filter and alarm should exist for usage of the \"root\" user",
    description: "This control checks whether a CloudWatch Logs metric filter and an alarm on that filter exist for use of the root user. The control fails if the account has no log group receiving the management events of a multi-Region CloudTrail trail with such a metric filter and alarm. Monitoring use of the root user lets you detect use of the account's most privileged credentials.",
    severity_rating: "LOW",
    related_requirements: &[],
};

const CONFIG_1: StandardsControlDefinition = StandardsControlDefinition {
    control_id: "Config.1",
    title: "AWS Config should be enabled and use the service-linked role for resource recording",
    description: "This control checks whether the AWS Config configuration recorder is enabled in your account and Region, records all resources, and uses the service-linked role for recording. The control fails if AWS Config isn't enabled, if the recorder doesn't record all resources, or if it records with a role other than the service-linked role.",
    severity_rating: "MEDIUM",
    related_requirements: &[],
};

const EC2_2: StandardsControlDefinition = StandardsControlDefinition {
    control_id: "EC2.2",
    title: "VPC default security groups should not allow inbound or outbound traffic",
    description: "This control checks whether the default security group of a VPC allows inbound or outbound traffic. The control fails if the default security group has one or more inbound or outbound rules. The rules of a default security group apply to every resource associated with it, so leaving them in place grants those resources traffic they were never configured for.",
    severity_rating: "MEDIUM",
    related_requirements: &[],
};

const EC2_13: StandardsControlDefinition = StandardsControlDefinition {
    control_id: "EC2.13",
    title: "Security groups should not allow ingress from 0.0.0.0/0 or ::/0 to port 22",
    description: "This control checks whether a security group allows unrestricted incoming traffic to port 22 from the IPv4 address 0.0.0.0/0 or the IPv6 address ::/0. The control fails if the security group allows ingress to port 22 from either of those addresses.",
    severity_rating: "HIGH",
    related_requirements: &[],
};

const EC2_52: StandardsControlDefinition = StandardsControlDefinition {
    control_id: "EC2.52",
    title: "EC2 transit gateways should be tagged",
    description: "This control checks whether an Amazon EC2 transit gateway has tags with the specific keys defined in the parameter requiredTagKeys. The control fails if the transit gateway doesn't have any tag keys or if it doesn't have all the keys specified in the parameter requiredTagKeys. If the parameter requiredTagKeys isn't provided, the control only checks for the existence of a tag key and fails if the transit gateway isn't tagged with any key.",
    severity_rating: "LOW",
    related_requirements: &[],
};

const ECS_13: StandardsControlDefinition = StandardsControlDefinition {
    control_id: "ECS.13",
    title: "ECS services should be tagged",
    description: "This control checks whether an Amazon ECS service has tags with the specific keys defined in the parameter requiredTagKeys. The control fails if the service doesn't have any tag keys or if it doesn't have all the keys specified in the parameter requiredTagKeys. If the parameter requiredTagKeys isn't provided, the control only checks for the existence of a tag key and fails if the service isn't tagged with any key.",
    severity_rating: "LOW",
    related_requirements: &[],
};

const GUARDDUTY_1: StandardsControlDefinition = StandardsControlDefinition {
    control_id: "GuardDuty.1",
    title: "GuardDuty should be enabled",
    description: "This control checks whether Amazon GuardDuty is enabled in your account and Region. The control fails if GuardDuty isn't enabled. GuardDuty analyzes the account's management events, network traffic and DNS queries to detect unexpected and potentially unauthorized activity.",
    severity_rating: "HIGH",
    related_requirements: &[],
};

const IAM_4: StandardsControlDefinition = StandardsControlDefinition {
    control_id: "IAM.4",
    title: "IAM root user access key should not exist",
    description: "This control checks whether the root user of your account has an access key. The control fails if an access key exists for the root user. An access key gives programmatic access to every resource in the account, so removing the root user's access keys limits what a compromise of those keys can reach.",
    severity_rating: "CRITICAL",
    related_requirements: &[],
};

const IAM_7: StandardsControlDefinition = StandardsControlDefinition {
    control_id: "IAM.7",
    title: "Password policies for IAM users should have strong configurations",
    description: "This control checks whether the account password policy for IAM users requires the character classes and numeric bounds the control's parameters name. The control fails if the password policy is weaker than any of those parameters requires. Unless you provide custom parameter values, Security Hub uses the default value of each parameter.",
    severity_rating: "MEDIUM",
    related_requirements: &[],
};

const KMS_4: StandardsControlDefinition = StandardsControlDefinition {
    control_id: "KMS.4",
    title: "AWS KMS key rotation should be enabled",
    description: "This control checks whether automatic key rotation is enabled for a customer managed AWS KMS key. The control fails if automatic key rotation is disabled for the key. Rotating a key's cryptographic material limits the amount of data that one key version protects.",
    severity_rating: "MEDIUM",
    related_requirements: &[],
};

const S3_1: StandardsControlDefinition = StandardsControlDefinition {
    control_id: "S3.1",
    title: "S3 general purpose buckets should have block public access settings enabled",
    description: "This control checks whether the account-level Amazon S3 block public access settings are enabled. The control fails if any of the BlockPublicAcls, BlockPublicPolicy, IgnorePublicAcls and RestrictPublicBuckets settings is set to false.",
    severity_rating: "MEDIUM",
    related_requirements: &[],
};

const S3_5: StandardsControlDefinition = StandardsControlDefinition {
    control_id: "S3.5",
    title: "S3 general purpose buckets should require requests to use SSL",
    description: "This control checks whether an Amazon S3 general purpose bucket has a policy that requires requests to use SSL. The control fails if the bucket policy doesn't deny requests whose aws:SecureTransport condition key is false.",
    severity_rating: "MEDIUM",
    related_requirements: &[],
};

const BEDROCK_1: StandardsControlDefinition = StandardsControlDefinition {
    control_id: "Bedrock.1",
    title: "Amazon Bedrock model invocation logging should be enabled",
    description: "This control checks whether model invocation logging is enabled for Amazon Bedrock in your account and Region, and whether the invocation logs are delivered to at least one destination. The control fails if model invocation logging is disabled, or if it is enabled with neither an Amazon S3 bucket nor a CloudWatch Logs log group as a destination.",
    severity_rating: "MEDIUM",
    related_requirements: &[],
};

const SAGEMAKER_1: StandardsControlDefinition = StandardsControlDefinition {
    control_id: "SageMaker.1",
    title: "Amazon SageMaker AI notebook instances should not have direct internet access",
    description: "This control checks whether direct internet access is disabled for an Amazon SageMaker AI notebook instance. The control fails if the DirectInternetAccess field of the notebook instance is enabled. With direct internet access disabled, a notebook instance reaches the internet only through the VPC it is launched in.",
    severity_rating: "HIGH",
    related_requirements: &[],
};

const SAGEMAKER_2: StandardsControlDefinition = StandardsControlDefinition {
    control_id: "SageMaker.2",
    title: "SageMaker AI notebook instances should be launched in a custom VPC",
    description: "This control checks whether an Amazon SageMaker AI notebook instance is launched in a custom virtual private cloud (VPC). The control fails if the notebook instance isn't configured with a subnet of a custom VPC, in which case it runs in the service-managed VPC instead.",
    severity_rating: "HIGH",
    related_requirements: &[],
};

const SAGEMAKER_3: StandardsControlDefinition = StandardsControlDefinition {
    control_id: "SageMaker.3",
    title: "Users should not have root access to SageMaker AI notebook instances",
    description: "This control checks whether root access is disabled for the users of an Amazon SageMaker AI notebook instance. The control fails if the RootAccess field of the notebook instance is enabled, which lets the notebook's users run commands with administrator privileges.",
    severity_rating: "HIGH",
    related_requirements: &[],
};

// The controls of the standards whose provider is Azure, which evaluate Azure
// resources and so carry the controls reference's Azure control ids.

const AZURE_STORAGE_1: StandardsControlDefinition = StandardsControlDefinition {
    control_id: "AzureStorage.1",
    title: "Azure storage accounts should not allow public blob access",
    description: "This control checks whether an Azure storage account allows anonymous public read access to its blob containers. The control fails if public blob access is enabled for the storage account, which lets a caller read a container's blobs without credentials.",
    severity_rating: "MEDIUM",
    related_requirements: &[],
};

const AZURE_SQL_1: StandardsControlDefinition = StandardsControlDefinition {
    control_id: "AzureSQL.1",
    title: "Azure SQL servers should have auditing enabled",
    description: "This control checks whether auditing is enabled for an Azure SQL server. The control fails if the server's auditing setting is disabled, in which case the statements run against its databases are not recorded.",
    severity_rating: "MEDIUM",
    related_requirements: &[],
};

const AZURE_VM_1: StandardsControlDefinition = StandardsControlDefinition {
    control_id: "AzureVirtualMachine.1",
    title: "Azure virtual machines should have disk encryption enabled",
    description: "This control checks whether the operating system and data disks attached to an Azure virtual machine are encrypted. The control fails if any disk attached to the virtual machine is not encrypted.",
    severity_rating: "HIGH",
    related_requirements: &[],
};

const AZURE_KEY_VAULT_1: StandardsControlDefinition = StandardsControlDefinition {
    control_id: "AzureKeyVault.1",
    title: "Azure key vaults should have purge protection enabled",
    description: "This control checks whether purge protection is enabled for an Azure key vault. The control fails if purge protection is disabled for the vault, in which case a soft-deleted vault or key can be purged before its retention period ends.",
    severity_rating: "MEDIUM",
    related_requirements: &[],
};

/// The controls of the AI Security Best Practices standard: the controls of the
/// AI services whose resources it evaluates.
const AI_SECURITY_BEST_PRACTICES_CONTROLS: [StandardsControlDefinition; 4] =
    [BEDROCK_1, SAGEMAKER_1, SAGEMAKER_2, SAGEMAKER_3];

/// The controls of the AWS Foundational Security Best Practices standard, which
/// addresses each one by the security control's own id.
const FSBP_CONTROLS: [StandardsControlDefinition; 11] = [
    ACM_1,
    ACM_2,
    CLOUDTRAIL_1,
    CLOUDTRAIL_2,
    CLOUDWATCH_1,
    CONFIG_1,
    EC2_2,
    GUARDDUTY_1,
    IAM_4,
    IAM_7,
    S3_1,
];

/// The controls of the AWS Resource Tagging Standard, which is made up of the
/// "should be tagged" controls — the ones that check whether a resource carries the
/// tag keys the control's parameters name.
const RESOURCE_TAGGING_CONTROLS: [StandardsControlDefinition; 3] = [ACM_3, EC2_52, ECS_13];

/// The controls of the Azure Foundational Security Best Practices standard.
const AZURE_FSBP_CONTROLS: [StandardsControlDefinition; 4] =
    [AZURE_STORAGE_1, AZURE_SQL_1, AZURE_VM_1, AZURE_KEY_VAULT_1];

/// The controls of the CIS Microsoft Azure Foundations Benchmark v4.0.0 standard.
const CIS_AZURE_FOUNDATIONS_4_0_0_CONTROLS: [StandardsControlDefinition; 3] =
    [AZURE_STORAGE_1, AZURE_SQL_1, AZURE_VM_1];

/// The controls of the CIS AWS Foundations Benchmark v1.4.0 standard: a revision
/// built on the security controls, so it addresses each one by the control's own id
/// rather than by the benchmark's requirement number.
const CIS_AWS_FOUNDATIONS_1_4_0_CONTROLS: [StandardsControlDefinition; 5] =
    [CLOUDTRAIL_1, CLOUDWATCH_1, CONFIG_1, IAM_4, S3_1];

/// The controls of the CIS AWS Foundations Benchmark v3.0.0 standard.
const CIS_AWS_FOUNDATIONS_3_0_0_CONTROLS: [StandardsControlDefinition; 5] =
    [CLOUDTRAIL_1, CONFIG_1, EC2_2, IAM_4, KMS_4];

/// The controls of the CIS AWS Foundations Benchmark v5.0.0 standard.
const CIS_AWS_FOUNDATIONS_5_0_0_CONTROLS: [StandardsControlDefinition; 5] =
    [CLOUDTRAIL_1, CONFIG_1, EC2_13, IAM_4, S3_1];

/// The controls of the NIST Special Publication 800-171 Revision 2 standard.
const NIST_800_171_CONTROLS: [StandardsControlDefinition; 5] =
    [CLOUDTRAIL_1, CONFIG_1, IAM_4, KMS_4, S3_1];

/// The controls of the NIST Special Publication 800-53 Revision 5 standard.
const NIST_800_53_CONTROLS: [StandardsControlDefinition; 8] = [
    ACM_1,
    CLOUDTRAIL_1,
    CLOUDTRAIL_2,
    CONFIG_1,
    GUARDDUTY_1,
    IAM_4,
    KMS_4,
    S3_1,
];

/// The controls of the PCI DSS v4.0.1 standard: the newer revision is built on the
/// security controls, so — unlike the v3.2.1 revision below — it addresses each one
/// by the control's own id.
const PCI_DSS_4_0_1_CONTROLS: [StandardsControlDefinition; 5] =
    [CLOUDTRAIL_1, CLOUDTRAIL_2, IAM_4, S3_1, S3_5];

/// The controls of the CIS AWS Foundations Benchmark v1.2.0 standard this emulator
/// publishes, from the AWS Security Hub user guide's controls reference. This
/// standard predates consolidated control findings: it addresses each control by the
/// benchmark's own numbered requirement, prefixed `CIS.`, publishes the benchmark's
/// requirement statement as the control title, and relates each control to the
/// benchmark requirement of the same number.
const CIS_AWS_FOUNDATIONS_1_2_0_CONTROLS: [StandardsControlDefinition; 11] = [
    StandardsControlDefinition {
        control_id: "CIS.1.1",
        title: "Avoid the use of the \"root\" account",
        description: "The root account has unrestricted access to every resource in the account. This control checks whether the root account was used in the last day.",
        severity_rating: "LOW",
        related_requirements: &["CIS AWS Foundations 1.1"],
    },
    StandardsControlDefinition {
        control_id: "CIS.1.2",
        title: "Ensure multi-factor authentication (MFA) is enabled for all IAM users that have a console password",
        description: "This control checks whether multi-factor authentication is enabled for every IAM user that has a console password.",
        severity_rating: "MEDIUM",
        related_requirements: &["CIS AWS Foundations 1.2"],
    },
    StandardsControlDefinition {
        control_id: "CIS.1.3",
        title: "Ensure credentials unused for 90 days or greater are disabled",
        description: "This control checks whether IAM users have passwords or access keys that have not been used within 90 days.",
        severity_rating: "MEDIUM",
        related_requirements: &["CIS AWS Foundations 1.3"],
    },
    StandardsControlDefinition {
        control_id: "CIS.1.4",
        title: "Ensure access keys are rotated every 90 days or less",
        description: "This control checks whether the active access keys of IAM users are rotated at least every 90 days.",
        severity_rating: "MEDIUM",
        related_requirements: &["CIS AWS Foundations 1.4"],
    },
    StandardsControlDefinition {
        control_id: "CIS.1.12",
        title: "Ensure no root account access key exists",
        description: "This control checks whether an access key exists for the root account. Removing the root account's access keys limits the damage a compromise of those keys can do.",
        severity_rating: "CRITICAL",
        related_requirements: &["CIS AWS Foundations 1.12"],
    },
    StandardsControlDefinition {
        control_id: "CIS.1.13",
        title: "Ensure MFA is enabled for the \"root\" account",
        description: "This control checks whether multi-factor authentication is enabled for the root account.",
        severity_rating: "CRITICAL",
        related_requirements: &["CIS AWS Foundations 1.13"],
    },
    StandardsControlDefinition {
        control_id: "CIS.2.1",
        title: "Ensure CloudTrail is enabled in all regions",
        description: "This control checks whether at least one multi-Region CloudTrail trail records management events in the account.",
        severity_rating: "HIGH",
        related_requirements: &["CIS AWS Foundations 2.1"],
    },
    StandardsControlDefinition {
        control_id: "CIS.2.3",
        title: "Ensure the S3 bucket used to store CloudTrail logs is not publicly accessible",
        description: "This control checks whether the Amazon S3 bucket a CloudTrail trail delivers its log files to is publicly accessible.",
        severity_rating: "CRITICAL",
        related_requirements: &["CIS AWS Foundations 2.3"],
    },
    StandardsControlDefinition {
        control_id: "CIS.2.5",
        title: "Ensure AWS Config is enabled",
        description: "This control checks whether AWS Config is enabled in the account and Region and records all resources.",
        severity_rating: "MEDIUM",
        related_requirements: &["CIS AWS Foundations 2.5"],
    },
    StandardsControlDefinition {
        control_id: "CIS.4.1",
        title: "Ensure no security groups allow ingress from 0.0.0.0/0 to port 22",
        description: "This control checks whether a security group allows unrestricted incoming traffic to port 22.",
        severity_rating: "HIGH",
        related_requirements: &["CIS AWS Foundations 4.1"],
    },
    StandardsControlDefinition {
        control_id: "CIS.4.2",
        title: "Ensure no security groups allow ingress from 0.0.0.0/0 to port 3389",
        description: "This control checks whether a security group allows unrestricted incoming traffic to port 3389.",
        severity_rating: "HIGH",
        related_requirements: &["CIS AWS Foundations 4.2"],
    },
];

/// The controls of the PCI DSS v3.2.1 standard this emulator publishes, from the AWS
/// Security Hub user guide's controls reference. This standard addresses each control
/// by the security control's id prefixed `PCI.`, and relates each one to the PCI DSS
/// requirements it covers.
const PCI_DSS_3_2_1_CONTROLS: [StandardsControlDefinition; 5] = [
    StandardsControlDefinition {
        control_id: "PCI.AutoScaling.1",
        title: "Auto Scaling groups associated with a load balancer should use health checks",
        description: "This control checks whether an Auto Scaling group that is associated with a load balancer uses Elastic Load Balancing health checks.",
        severity_rating: "LOW",
        related_requirements: &["PCI DSS 2.2"],
    },
    StandardsControlDefinition {
        control_id: "PCI.CloudTrail.1",
        title: "CloudTrail logs should be encrypted at rest using AWS KMS CMKs",
        description: "This control checks whether a CloudTrail trail encrypts its log files at rest with an AWS KMS customer managed key.",
        severity_rating: "MEDIUM",
        related_requirements: &["PCI DSS 3.4"],
    },
    StandardsControlDefinition {
        control_id: "PCI.CloudTrail.2",
        title: "CloudTrail should be enabled",
        description: "This control checks whether CloudTrail is enabled in the account, so that the actions taken in it are recorded.",
        severity_rating: "HIGH",
        related_requirements: &["PCI DSS 10.1"],
    },
    StandardsControlDefinition {
        control_id: "PCI.IAM.1",
        title: "IAM root user access key should not exist",
        description: "This control checks whether an access key exists for the root user of the account.",
        severity_rating: "CRITICAL",
        related_requirements: &["PCI DSS 2.1", "PCI DSS 7.2.1"],
    },
    StandardsControlDefinition {
        control_id: "PCI.S3.1",
        title: "S3 buckets should prohibit public write access",
        description: "This control checks whether an Amazon S3 bucket allows public write access through its access control list or bucket policy.",
        severity_rating: "CRITICAL",
        related_requirements: &["PCI DSS 1.2.1", "PCI DSS 1.3.1"],
    },
];

/// The controls this emulator publishes for each standard of [`STANDARDS_CATALOG`].
/// `DescribeStandardsControls` reports the set of the standard its subscription
/// names, so every published standard carries one: a subscription whose standard has
/// controls is never reported as having none.
///
/// Each entry is the subset of that standard's published controls this emulator
/// carries, in the order the reference publishes them. A standard built on the
/// security controls addresses each control by the control's own id, so its entry
/// names the shared control definitions above; the two revisions that predate
/// consolidated control findings — CIS AWS Foundations Benchmark v1.2.0 and PCI DSS
/// v3.2.1 — address a control by their own prefixed requirement id and publish their
/// own requirement wording for it, so each of those has its own list.
pub const STANDARDS_CONTROL_CATALOG: [StandardControlSet; 13] = [
    StandardControlSet {
        slug: "ai-security-best-practices",
        version: "1.0.0",
        controls: &AI_SECURITY_BEST_PRACTICES_CONTROLS,
    },
    StandardControlSet {
        slug: "aws-foundational-security-best-practices",
        version: "1.0.0",
        controls: &FSBP_CONTROLS,
    },
    StandardControlSet {
        slug: "aws-resource-tagging-standard",
        version: "1.0.0",
        controls: &RESOURCE_TAGGING_CONTROLS,
    },
    StandardControlSet {
        slug: "azure-foundational-security-best-practices",
        version: "1.0.0",
        controls: &AZURE_FSBP_CONTROLS,
    },
    StandardControlSet {
        slug: "cis-aws-foundations-benchmark",
        version: "1.2.0",
        controls: &CIS_AWS_FOUNDATIONS_1_2_0_CONTROLS,
    },
    StandardControlSet {
        slug: "cis-aws-foundations-benchmark",
        version: "1.4.0",
        controls: &CIS_AWS_FOUNDATIONS_1_4_0_CONTROLS,
    },
    StandardControlSet {
        slug: "cis-aws-foundations-benchmark",
        version: "3.0.0",
        controls: &CIS_AWS_FOUNDATIONS_3_0_0_CONTROLS,
    },
    StandardControlSet {
        slug: "cis-aws-foundations-benchmark",
        version: "5.0.0",
        controls: &CIS_AWS_FOUNDATIONS_5_0_0_CONTROLS,
    },
    StandardControlSet {
        slug: "cis-azure-foundations-benchmark",
        version: "4.0.0",
        controls: &CIS_AZURE_FOUNDATIONS_4_0_0_CONTROLS,
    },
    StandardControlSet {
        slug: "nist-800-171",
        version: "2.0.0",
        controls: &NIST_800_171_CONTROLS,
    },
    StandardControlSet {
        slug: "nist-800-53",
        version: "5.0.0",
        controls: &NIST_800_53_CONTROLS,
    },
    StandardControlSet {
        slug: "pci-dss",
        version: "3.2.1",
        controls: &PCI_DSS_3_2_1_CONTROLS,
    },
    StandardControlSet {
        slug: "pci-dss",
        version: "4.0.1",
        controls: &PCI_DSS_4_0_1_CONTROLS,
    },
];

/// The controls `definition`'s standard publishes, in the published order. The single
/// definition of which controls a standard carries, so every operation that addresses
/// a standards control resolves the same set.
pub fn standard_controls(
    definition: &StandardDefinition,
) -> &'static [StandardsControlDefinition] {
    STANDARDS_CONTROL_CATALOG
        .iter()
        .find(|set| set.slug == definition.slug && set.version == definition.version)
        .map_or(&[], |set| set.controls)
}

/// The ARN of one control of one account's subscription to a standard:
/// `arn:aws:securityhub:<region>:<account>:control/<slug>/v/<version>/<ControlId>` —
/// the form the `StandardsControlArn` example in this service's `smithy-model/`
/// carries (`:control/pci-dss/v/3.2.1/PCI.AutoScaling.1`), so a control's ARN names
/// the standard it belongs to and the account and Region whose subscription it is
/// part of.
pub fn standards_control_arn(
    region: &str,
    definition: &StandardDefinition,
    control_id: &str,
) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!(
            "control/{}/v/{}/{control_id}",
            definition.slug, definition.version
        ),
    )
}

/// `StandardsControl$RemediationUrl`: "A link to remediation information for the
/// control in the Security Hub CSPM user documentation" — the console deep link the
/// user guide publishes for a control of a standard, which carries the control's own
/// id and so is the one remediation link a control id does determine.
///
/// This is NOT the link a `SecurityControlDefinition` reports: that member is "a link
/// to Security Hub CSPM documentation that explains how to remediate a failed finding
/// for a security control" (`smithy_query(query="doc",
/// shape="SecurityControlDefinition", member="RemediationUrl")`), a page catalogued per
/// control in [`SecurityControlCatalogEntry::remediation_url`] rather than addressed by
/// id.
pub fn standards_control_remediation_url(control_id: &str) -> String {
    format!("https://docs.aws.amazon.com/console/securityhub/{control_id}/remediation")
}

/// The `ControlStatus` a subscription holds for one of its controls: the status a
/// caller last set for it, or `ENABLED` — the status enabling the standard gave every
/// control of it — while nobody has changed it. The single definition of that default,
/// so every reader of a control's enablement agrees on it.
pub fn subscription_control_status<'a>(
    record: &'a StandardsSubscriptionRecord,
    control_id: &str,
) -> &'a str {
    record
        .control_statuses
        .get(control_id)
        .map_or(CONTROL_STATUS_ENABLED, |status| {
            status.control_status.as_str()
        })
}

/// One control of one subscription as the modelled `StandardsControl` every operation
/// that reports a control answers with: the control's published metadata, plus the
/// status this subscription holds for it.
///
/// A control the subscription holds no changed status for carries the status enabling
/// the standard gave it — `ENABLED`, last set when the standard was enabled.
pub fn standards_control_view(
    region: &str,
    definition: &StandardDefinition,
    control: &StandardsControlDefinition,
    record: &StandardsSubscriptionRecord,
) -> StandardsControl {
    let status = record.control_statuses.get(control.control_id);
    StandardsControl {
        standards_control_arn: Some(standards_control_arn(region, definition, control.control_id)),
        control_status: Some(subscription_control_status(record, control.control_id).to_string()),
        disabled_reason: status.and_then(|status| status.disabled_reason.clone()),
        control_status_updated_at: Some(
            status.map_or(record.enabled_at, |status| status.control_status_updated_at),
        ),
        control_id: Some(control.control_id.to_string()),
        title: Some(control.title.to_string()),
        description: Some(control.description.to_string()),
        remediation_url: Some(standards_control_remediation_url(control.control_id)),
        severity_rating: Some(control.severity_rating.to_string()),
        related_requirements: (!control.related_requirements.is_empty()).then(|| {
            control
                .related_requirements
                .iter()
                .map(|requirement| (*requirement).to_string())
                .collect()
        }),
    }
}

/// The only schema version the `AwsSecurityFinding` format defines: "The schema
/// version that a finding is formatted for. The value is `2018-10-08`"
/// (`smithy_query(query="shape", shape="AwsSecurityFinding")`, member
/// `SchemaVersion`).
pub const FINDING_SCHEMA_VERSION: &str = "2018-10-08";

/// The `RecordState` an ingested finding starts in: "ACTIVE - The finding must be
/// investigated or remediated" is the live state, `ARCHIVED` the other
/// (`smithy_query(query="shape", shape="RecordState")`). Security Hub populates it,
/// so a provider that submits none still gets one.
pub const DEFAULT_RECORD_STATE: &str = "ACTIVE";

/// The `Workflow.Status` an ingested finding starts in: "NEW - The initial state of a
/// finding, before it is reviewed" (`smithy_query(query="doc", shape="Workflow",
/// member="Status")`).
pub const DEFAULT_WORKFLOW_STATUS: &str = "NEW";

/// The longest `Note.Text` a finding keeps: "Length Constraints: Minimum of 1.
/// Maximum of 512." (`smithy_query(query="doc", shape="Note", member="Text")`). The
/// bound is the service's own, and it is what keeps one note write bounded: a note is
/// caller-sized text copied onto EVERY finding the write addresses, so without the
/// documented ceiling a single request would grow this store by (findings written) ×
/// (whatever text the caller sent).
pub const MAX_NOTE_TEXT_LEN: usize = 512;

/// Why this note text cannot be stored, naming the member and the constraint it
/// violates, or `None` when it satisfies them.
///
/// The documented length constraint is on the stored `Note$Text`, so it holds wherever
/// a note is written — a provider's `Note` on an imported finding as much as a
/// customer's `NoteUpdate` — which is why the bound and its wording live here and
/// every writer asks this one function. `Note$Text`'s minimum of 1 is already refused
/// at the request boundary, where the member targets `NonEmptyString`
/// (`@pattern("\\S")`).
pub fn note_text_violation(text: &str) -> Option<String> {
    let characters = text.chars().count();
    (characters > MAX_NOTE_TEXT_LEN).then(|| {
        format!("Note.Text must be at most {MAX_NOTE_TEXT_LEN} characters, got {characters}")
    })
}

/// [`note_text_violation`] for the text a `NoteUpdate` carries, so an operation that
/// holds one asks it directly.
pub fn note_update_violation(note: &NoteUpdate) -> Option<String> {
    note_text_violation(&note.text)
}

/// The Region an emulated request is for: the SigV4 credential scope's Region when
/// the request is signed, else the emulator's default Region.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The Region an unsigned request is attributed to. Emulators default to
/// `us-east-1` (see `emulator_server::emulator_arn`).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The account the emulator presents for every caller.
pub fn request_account_id() -> String {
    ::engine::emulator_account_id().to_string()
}

/// The hub key for the request `ctx` belongs to.
pub fn hub_key(ctx: &smithy_server_runtime::OperationContext<'_>) -> HubKey {
    (request_account_id(), request_region(ctx))
}

/// The key of a member association of the administrator hub `hub` — the single
/// definition of how every member operation addresses an association.
pub fn member_key(hub: &HubKey, member_account_id: &str) -> MemberKey {
    MemberKey {
        account_id: hub.0.clone(),
        region: hub.1.clone(),
        member_account_id: member_account_id.to_string(),
    }
}

/// An Amazon Web Services account id is exactly 12 decimal digits (the
/// `AwsSecurityFinding$AwsAccountId` length constraint: "Length Constraints: 12").
/// The `AccountId` shape itself carries no `length`/`pattern` trait, so the
/// framework's boundary validation cannot refuse a malformed id and each member
/// operation checks it.
pub const ACCOUNT_ID_LEN: usize = 12;

/// Whether `candidate` is a well-formed Amazon Web Services account id.
pub fn is_account_id(candidate: &str) -> bool {
    candidate.len() == ACCOUNT_ID_LEN && candidate.bytes().all(|byte| byte.is_ascii_digit())
}

// The documented `Member$MemberStatus` values (`smithy_query(query="shape",
// shape="Member")`, member `MemberStatus`).

/// "`Created` - Indicates that the administrator account added the member account,
/// but has not yet invited the member account."
pub const MEMBER_STATUS_CREATED: &str = "Created";
/// "`Invited` - Indicates that the administrator account invited the member account.
/// The member account has not yet responded to the invitation."
pub const MEMBER_STATUS_INVITED: &str = "Invited";
/// "`Enabled` - Indicates that the member account is currently active."
pub const MEMBER_STATUS_ENABLED: &str = "Enabled";
/// "`Removed` - Indicates that the administrator account disassociated the member
/// account."
pub const MEMBER_STATUS_REMOVED: &str = "Removed";
/// "`Resigned` - Indicates that the member account disassociated themselves from the
/// administrator account."
pub const MEMBER_STATUS_RESIGNED: &str = "Resigned";
/// "`Deleted` - Indicates that the administrator account deleted the member account."
pub const MEMBER_STATUS_DELETED: &str = "Deleted";

/// Whether a member status means the association is no longer in force, so
/// `CreateMembers` re-creates it rather than leaving it alone: "`CreateMembers` is
/// only used in the following cases: … The account was disassociated or deleted in
/// Security Hub CSPM" (`smithy_query(query="operation", operation="CreateMembers")`).
pub fn member_status_is_dissolved(status: &str) -> bool {
    matches!(
        status,
        MEMBER_STATUS_REMOVED | MEMBER_STATUS_RESIGNED | MEMBER_STATUS_DELETED
    )
}

/// Whether a member status means the account is still associated with its
/// administrator — the complement of [`member_status_is_dissolved`]: `Created`,
/// `Invited` and `Enabled` all describe a relationship that is in force, while
/// `Removed`, `Resigned` and `Deleted` describe one the administrator or the member
/// ended (`smithy_query(query="shape", shape="Member")`, member `MemberStatus`).
pub fn member_status_is_associated(status: &str) -> bool {
    !member_status_is_dissolved(status)
}

/// The one projection a read reports a member association with, so `GetMembers` and
/// `ListMembers` can never describe the same stored association differently.
///
/// `Member$MasterId` "is replaced by `AdministratorId`" and carries "the Amazon Web
/// Services account ID of the Security Hub CSPM administrator account associated with
/// this member account" (`smithy_query(query="shape", shape="Member")`), so both
/// members report the administrator account of the hub the association belongs to.
pub fn member_view(
    member_account_id: &str,
    association: &MemberAssociation,
    administrator_id: &str,
) -> Member {
    Member {
        account_id: Some(member_account_id.to_string()),
        email: association.email.clone(),
        master_id: Some(administrator_id.to_string()),
        administrator_id: Some(administrator_id.to_string()),
        member_status: Some(association.member_status.clone()),
        invited_at: association.invited_at,
        updated_at: Some(association.updated_at),
    }
}

/// The resource prefix a custom action target's ARN carries:
/// `arn:aws:securityhub:<region>:<account>:action/custom/<Id>` — the form the
/// model's own `ActionTargetArn` examples carry (every one of them matches
/// `"ActionTargetArn":"arn:aws:securityhub:<region>:<account>:action/custom/<Id>"`
/// in this service's `smithy-model/`).
pub const ACTION_TARGET_RESOURCE_PREFIX: &str = "action/custom/";

/// The ARN `CreateActionTarget` answers with for `target_id` in `region`.
pub fn action_target_arn(region: &str, target_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{ACTION_TARGET_RESOURCE_PREFIX}{target_id}"),
    )
}

/// `arn:aws:securityhub:<region>:<account>:hub/default` for a hub key.
pub fn hub_arn(key: &HubKey) -> String {
    // `emulator_arn` stamps the canonical emulator account id; the key's account is
    // that same id, so the two cannot drift.
    ::engine::emulator_arn(ARN_NAMESPACE, &key.1, HUB_RESOURCE)
}

/// The current instant as a modelled `Timestamp` — the type every `date-time`
/// member of this service (`Member$UpdatedAt`, `Member$InvitedAt`, …) carries.
pub fn now_date_time() -> ::aws_smithy_types::DateTime {
    let since_epoch = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        since_epoch.subsec_nanos(),
    )
}

/// The current instant in the ISO-8601 (RFC-3339) form every Security Hub
/// timestamp member uses.
pub fn now_iso8601() -> String {
    now_date_time()
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|_| String::new())
}

/// The current instant in whole nanoseconds since the epoch, the unit
/// [`parse_iso8601_nanos`] answers in, so a relative date filter compares in one
/// unit.
pub fn now_nanos() -> i128 {
    let since_epoch = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default();
    i128::try_from(since_epoch.as_nanos()).unwrap_or(i128::MAX)
}

/// Parse an ISO-8601 (RFC-3339) timestamp into whole nanoseconds since the epoch,
/// for ordering and date filtering. `None` when the text is not a valid instant.
pub fn parse_iso8601_nanos(text: &str) -> Option<i128> {
    Some(datetime_nanos(&parse_iso8601(text)?))
}

/// Parse an ISO-8601 (RFC-3339) timestamp into the modelled `Timestamp` type, for a
/// response member the model types as a timestamp rather than a string. `None` when
/// the text is not a valid instant.
pub fn parse_iso8601(text: &str) -> Option<::aws_smithy_types::DateTime> {
    ::aws_smithy_types::DateTime::from_str(
        text,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()
}

/// An instant in the whole nanoseconds since the epoch that [`parse_iso8601_nanos`]
/// and [`now_nanos`] answer in, so a bound a caller sent as a modelled `Timestamp`
/// compares against a stored timestamp in one unit.
pub fn datetime_nanos(instant: &::aws_smithy_types::DateTime) -> i128 {
    i128::from(instant.secs()) * 1_000_000_000 + i128::from(instant.subsec_nanos())
}

/// The greatest number of tags one Security Hub resource carries: the modelled
/// `TagMap`'s own `length` maximum (`smithy_query(query="shape", shape="TagMap")`
/// reports `length {min: 1, max: 50}`), which is the shape `ListTagsForResource`
/// reports a resource's whole tag set in.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// Whether `arn` is a well-formed ARN: the six colon-separated ARN fields
/// `arn:<partition>:<service>:<region>:<account>:<resource>`, with a non-empty
/// partition, service and resource (Region and account are empty on the
/// service-owned ARNs Security Hub mints, e.g. a product ARN).
///
/// The modelled `ResourceArn` pattern (`^arn:aws:securityhub:`) only pins the
/// prefix, so the boundary validator admits a truncated ARN; that is a bad input
/// parameter, not a missing resource.
pub fn is_well_formed_arn(arn: &str) -> bool {
    let mut fields = arn.splitn(6, ':');
    let (Some("arn"), Some(partition), Some(service), Some(_region), Some(_account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return false;
    };
    !partition.is_empty() && !service.is_empty() && !resource.is_empty()
}

/// The tag set of the resource `resource_arn` names, for reading, as seen by the
/// caller whose account and Region are `key`. `None` when that caller has no
/// resource carrying that ARN.
///
/// Tagging addresses a resource by ARN, and Security Hub CSPM is subscribed per
/// account and Region behind per-Region endpoints, so the search is scoped to the
/// requesting account/Region: an ARN naming another Region's resource resolves to
/// nothing here, which is the 404 the call owes it. Every resource kind this crate
/// persists and the model gives a tag set is looked up here — today the
/// `hub/default` resource `EnableSecurityHub` creates, the `hubv2/default` resource
/// `EnableSecurityHubV2` creates and the `automation-rule/...` resources
/// `CreateAutomationRule` creates; a resource kind whose create operation is still a
/// stub has nothing in state to find.
pub fn resource_tags<'a>(
    store: &'a ComAmazonawsSecurityhubSecurityhubapiserviceState,
    key: &HubKey,
    resource_arn: &str,
) -> Option<&'a HashMap<String, String>> {
    if let Some(tags) = store
        .hubs
        .get(key)
        .filter(|hub| hub.hub_arn == resource_arn)
        .map(|hub| &hub.tags)
    {
        return Some(tags);
    }
    if let Some(tags) = store
        .hubs_v2
        .get(key)
        .filter(|hub| hub.hub_v2_arn == resource_arn)
        .map(|hub| &hub.tags)
    {
        return Some(tags);
    }
    store
        .automation_rules
        .get(&AutomationRuleKey {
            account_id: key.0.clone(),
            region: key.1.clone(),
            rule_arn: resource_arn.to_string(),
        })
        .map(|rule| &rule.tags)
}

/// The tag set of the resource `resource_arn` names, for writing — the mutable twin
/// of [`resource_tags`], so `TagResource` and `UntagResource` resolve an ARN exactly
/// as `ListTagsForResource` does.
pub fn resource_tags_mut<'a>(
    store: &'a mut ComAmazonawsSecurityhubSecurityhubapiserviceState,
    key: &HubKey,
    resource_arn: &str,
) -> Option<&'a mut HashMap<String, String>> {
    // The hub is checked first and the borrow released before the rule map is
    // reached, so one lookup never holds the store mutably for the other.
    if store
        .hubs
        .get(key)
        .is_some_and(|hub| hub.hub_arn == resource_arn)
    {
        return store.hubs.get_mut(key).map(|hub| &mut hub.tags);
    }
    if store
        .hubs_v2
        .get(key)
        .is_some_and(|hub| hub.hub_v2_arn == resource_arn)
    {
        return store.hubs_v2.get_mut(key).map(|hub| &mut hub.tags);
    }
    store
        .automation_rules
        .get_mut(&AutomationRuleKey {
            account_id: key.0.clone(),
            region: key.1.clone(),
            rule_arn: resource_arn.to_string(),
        })
        .map(|rule| &mut rule.tags)
}

/// The company every standard in [`STANDARDS_CATALOG`] is managed by. The model's
/// prose says "For existing standards, the value is equal to `Amazon Web Services`"
/// (`smithy_query(query="shape", shape="StandardsManagedBy")`, member `Company`), but
/// the service answers the short company identifier `AWS` on the wire, which is what
/// a client comparing `StandardsManagedBy.Company` sees; the wire form wins over the
/// doc's expansion of it.
pub const STANDARDS_MANAGED_BY_COMPANY: &str = "AWS";

/// The product every standard in [`STANDARDS_CATALOG`] is managed by: "the value is
/// equal to the Amazon Web Services service that manages the standard"
/// (`smithy_query(query="shape", shape="StandardsManagedBy")`, member `Product`).
pub const STANDARDS_MANAGED_BY_PRODUCT: &str = "Security Hub";

/// [`STANDARDS_CATALOG`] as the modelled `Standard` list `DescribeStandards`
/// reports, with every ARN built for `region`.
///
/// Every published standard is managed by Amazon Web Services, so each entry
/// carries the same `StandardsManagedBy` pair.
pub fn standards_catalog(region: &str) -> Vec<Standard> {
    STANDARDS_CATALOG
        .iter()
        .map(|definition| Standard {
            standards_arn: Some(standard_arn(region, definition)),
            name: Some(definition.name.to_string()),
            description: Some(definition.description.to_string()),
            enabled_by_default: Some(definition.enabled_by_default),
            provider: Some(definition.provider.to_string()),
            standards_managed_by: Some(StandardsManagedBy {
                company: Some(STANDARDS_MANAGED_BY_COMPANY.to_string()),
                product: Some(STANDARDS_MANAGED_BY_PRODUCT.to_string()),
            }),
        })
        .collect()
}

/// The documented `Normalized` value for a `SeverityLabel`: "If you provide `Label`
/// and don't provide `Normalized`, then `Normalized` is set automatically as
/// follows: INFORMATIONAL - 0, LOW - 1, MEDIUM - 40, HIGH - 70, CRITICAL - 90"
/// (`smithy_query(query="doc", shape="Severity", member="Normalized")`).
pub fn normalized_for_label(label: &str) -> Option<i32> {
    match label {
        "INFORMATIONAL" => Some(0),
        "LOW" => Some(1),
        "MEDIUM" => Some(40),
        "HIGH" => Some(70),
        "CRITICAL" => Some(90),
        _ => None,
    }
}

/// The documented `SeverityLabel` for a `Normalized` value: "If you provide
/// `Normalized` and don't provide `Label`, `Label` is set automatically as follows:
/// 0 - INFORMATIONAL, 1–39 - LOW, 40–69 - MEDIUM, 70–89 - HIGH, 90–100 - CRITICAL"
/// (`smithy_query(query="doc", shape="SeverityUpdate", member="Normalized")`).
pub fn label_for_normalized(normalized: i32) -> Option<&'static str> {
    match normalized {
        0 => Some("INFORMATIONAL"),
        1..=39 => Some("LOW"),
        40..=69 => Some("MEDIUM"),
        70..=89 => Some("HIGH"),
        90..=100 => Some("CRITICAL"),
        _ => None,
    }
}

/// Fill in whichever of `Label` / `Normalized` the submitter left out, from the
/// other, per the two documented mappings above. A severity that carries both, or
/// neither, is left exactly as submitted.
pub fn complete_severity(severity: &mut Severity) {
    match (severity.label.clone(), severity.normalized) {
        (Some(_), Some(_)) | (None, None) => {}
        (Some(label), None) => severity.normalized = normalized_for_label(&label),
        (None, Some(normalized)) => {
            severity.label = label_for_normalized(normalized).map(str::to_string);
        }
    }
}

// ---------------------------------------------------------------------------
// Finding filter primitives
//
// `StringFilter` / `NumberFilter` / `DateFilter` / `BooleanFilter` and the finding
// attributes they read are SHAPE-level, not operation-level: `GetFindings$Filters`
// (`AwsSecurityFindingFilters`) and an automation rule's `Criteria`
// (`AutomationRulesFindingFilters`) are two member sets over the same filter shapes
// and the same `AwsSecurityFinding`. The predicates therefore live here, where every
// operation that filters findings reaches them, instead of being re-implemented per
// operation and drifting apart.
// ---------------------------------------------------------------------------

/// The `StringFilterComparison` values that select findings which HAVE the filter
/// value; `StringFilter$Comparison` documents them as joined by `OR` on one field.
pub const POSITIVE_STRING_COMPARISONS: [&str; 3] = ["EQUALS", "PREFIX", "CONTAINS"];

/// The published breadth limit on a finding filter: `GetFindings$Filters`, "You can
/// filter by up to 10 finding attributes." Every surface that filters findings — the
/// query and an automation rule's stored `Criteria` — is held to it, so no request
/// can store or evaluate an unbounded predicate set.
pub const MAX_FILTER_ATTRIBUTES: usize = 10;

/// The published depth limit on one filtered attribute: `GetFindings$Filters`, "For
/// each attribute, you can provide up to 20 filter values."
pub const MAX_FILTER_VALUES_PER_ATTRIBUTE: usize = 20;

/// The `StringFilterComparison` values that select findings which do NOT have the
/// filter value; `StringFilter$Comparison` documents them as joined by `AND` on one
/// field.
pub const NEGATIVE_STRING_COMPARISONS: [&str; 3] =
    ["NOT_EQUALS", "PREFIX_NOT_EQUALS", "NOT_CONTAINS"];

/// Does `value` satisfy `comparison` against `filter_value`, read as the positive
/// form of the comparison? `NOT_*` comparisons are evaluated by negating their
/// positive twin, which is why they share one function.
pub fn positive_form_matches(value: &str, filter_value: &str, comparison: &str) -> bool {
    match comparison {
        "EQUALS" | "NOT_EQUALS" => value == filter_value,
        "PREFIX" | "PREFIX_NOT_EQUALS" => value.starts_with(filter_value),
        // Filter values are case sensitive (`StringFilter$Value`).
        "CONTAINS" | "NOT_CONTAINS" => value.contains(filter_value),
        _ => false,
    }
}

/// The comparison a `StringFilter` asks for; `EQUALS` when the member is absent, the
/// only comparison that reads a bare value as itself.
pub fn comparison_of(filter: &StringFilter) -> &str {
    filter.comparison.as_deref().unwrap_or("EQUALS")
}

/// Refuse the filter combinations `StringFilter$Comparison` documents as errors:
/// "You can't have both a `CONTAINS` filter and a `NOT_CONTAINS` filter on the same
/// field. Similarly, you can't provide both an `EQUALS` filter and a `NOT_EQUALS` or
/// `PREFIX_NOT_EQUALS` filter on the same field. ... `CONTAINS` filters can only be
/// used with other `CONTAINS` filters. `NOT_CONTAINS` filters can only be used with
/// other `NOT_CONTAINS` filters."
///
/// `CONTAINS_WORD` is refused here too: that comparison "works only in the
/// `GetFindingsV2`, `GetFindingStatisticsV2`, `GetResourcesV2`, and
/// `GetResourcesStatisticsV2` APIs", none of which is a v1 finding filter.
pub fn check_string_filter_combination(
    attribute: &str,
    filters: &[StringFilter],
) -> Result<(), String> {
    let comparisons: Vec<&str> = filters.iter().map(comparison_of).collect();
    if comparisons.contains(&"CONTAINS_WORD") {
        return Err(format!(
            "Filter {attribute}: the CONTAINS_WORD comparison works only in the V2 finding APIs"
        ));
    }
    // A comparison that is neither a positive nor a negative form names no predicate
    // this emulator can evaluate. Refusing it is what keeps a filtered request from
    // quietly answering as if the filter were not there.
    if let Some(unknown) = comparisons.iter().find(|c| {
        !POSITIVE_STRING_COMPARISONS.contains(c) && !NEGATIVE_STRING_COMPARISONS.contains(c)
    }) {
        return Err(format!(
            "Filter {attribute}: {unknown} is not a StringFilterComparison this emulator evaluates"
        ));
    }
    let has = |wanted: &str| comparisons.contains(&wanted);
    if has("CONTAINS") && comparisons.iter().any(|c| *c != "CONTAINS") {
        return Err(format!(
            "Filter {attribute}: CONTAINS filters can only be used with other CONTAINS filters"
        ));
    }
    if has("NOT_CONTAINS") && comparisons.iter().any(|c| *c != "NOT_CONTAINS") {
        return Err(format!(
            "Filter {attribute}: NOT_CONTAINS filters can only be used with other NOT_CONTAINS filters"
        ));
    }
    if has("EQUALS") && (has("NOT_EQUALS") || has("PREFIX_NOT_EQUALS")) {
        return Err(format!(
            "Filter {attribute}: an EQUALS filter cannot be combined with a NOT_EQUALS or PREFIX_NOT_EQUALS filter on the same field"
        ));
    }
    Ok(())
}

/// Evaluate one field's string filters against every value the finding carries for
/// that field (a list-valued attribute such as `Type` carries several).
pub fn string_filters_match(values: &[&str], filters: &[StringFilter]) -> bool {
    let mut saw_positive = false;
    let mut positive_hit = false;
    for filter in filters {
        let Some(filter_value) = filter.value.as_deref() else {
            continue;
        };
        let comparison = comparison_of(filter);
        let any_value_matches = values
            .iter()
            .any(|value| positive_form_matches(value, filter_value, comparison));
        if POSITIVE_STRING_COMPARISONS.contains(&comparison) {
            saw_positive = true;
            positive_hit |= any_value_matches;
        } else if NEGATIVE_STRING_COMPARISONS.contains(&comparison) && any_value_matches {
            // Negative filters are joined by AND: one hit excludes the finding.
            return false;
        }
    }
    !saw_positive || positive_hit
}

/// Evaluate one field's number filters. Several filters on one field are joined by
/// `OR`, matching the string-filter rule; the bounds within one filter are joined by
/// `AND`.
pub fn number_filters_match(value: Option<f64>, filters: &[NumberFilter]) -> bool {
    if filters.is_empty() {
        return true;
    }
    let Some(value) = value else {
        return false;
    };
    filters.iter().any(|filter| {
        // `NumberFilter$Eq` is an equality predicate on the value the finding
        // carries, so this is EXACT equality on purpose: the filtered attributes
        // (`Confidence`, `Criticality`, `SeverityNormalized`, `SeverityProduct`) are
        // compared to the value the caller sent, not approximated.
        filter.eq.is_none_or(|eq| value == eq)
            && filter.gte.is_none_or(|gte| value >= gte)
            && filter.lte.is_none_or(|lte| value <= lte)
            && filter.gt.is_none_or(|gt| value > gt)
            && filter.lt.is_none_or(|lt| value < lt)
    })
}

/// Evaluate one field's date filters, joined by `OR` like the other filter kinds.
/// `Start`/`End` are absolute inclusive bounds; a `DateRange` is relative to `now`,
/// `WITHIN` the window or `OLDER_THAN` it ("If you don't specify a value, the default
/// is `WITHIN`" — `DateRange$Comparison`).
pub fn date_filters_match(
    attribute: &str,
    value: Option<&str>,
    filters: &[DateFilter],
    now_nanos: i128,
) -> Result<bool, String> {
    if filters.is_empty() {
        return Ok(true);
    }
    let Some(instant) = value.and_then(parse_iso8601_nanos) else {
        return Ok(false);
    };
    let mut any = false;
    for filter in filters {
        let mut matches = true;
        if let Some(start) = filter.start.as_deref() {
            let start = parse_iso8601_nanos(start)
                .ok_or_else(|| format!("Filter {attribute}: Start must be an ISO-8601 timestamp"))?;
            matches &= instant >= start;
        }
        if let Some(end) = filter.end.as_deref() {
            let end = parse_iso8601_nanos(end)
                .ok_or_else(|| format!("Filter {attribute}: End must be an ISO-8601 timestamp"))?;
            matches &= instant <= end;
        }
        if let Some(range) = filter.date_range.as_ref() {
            let window = date_range_nanos(attribute, range)?;
            let boundary = now_nanos - window;
            matches &= match range.comparison.as_deref().unwrap_or("WITHIN") {
                "WITHIN" => instant >= boundary,
                "OLDER_THAN" => instant < boundary,
                other => {
                    return Err(format!(
                        "Filter {attribute}: DateRange.Comparison {other} is not a supported comparison"
                    ))
                }
            };
        }
        any |= matches;
    }
    Ok(any)
}

/// The width of a `DateRange` in nanoseconds. `DateRangeUnit` models `DAYS` only, and
/// a range names a window, so its `Value` is required and cannot be negative.
pub fn date_range_nanos(attribute: &str, range: &DateRange) -> Result<i128, String> {
    let Some(value) = range.value else {
        return Err(format!(
            "Filter {attribute}: DateRange.Value is required when DateRange is set"
        ));
    };
    if value < 0 {
        return Err(format!(
            "Filter {attribute}: DateRange.Value must not be negative, got {value}"
        ));
    }
    let value = i128::from(value);
    match range.unit.as_deref().unwrap_or("DAYS") {
        "DAYS" => Ok(value * 24 * 60 * 60 * 1_000_000_000),
        other => Err(format!(
            "Filter {attribute}: DateRange.Unit {other} is not a supported unit"
        )),
    }
}

/// Evaluate one field's boolean filters, joined by `OR`.
pub fn boolean_filters_match(value: Option<bool>, filters: &[BooleanFilter]) -> bool {
    if filters.is_empty() {
        return true;
    }
    filters
        .iter()
        .any(|filter| filter.value.is_none_or(|wanted| value == Some(wanted)))
}

/// The value(s) a finding carries for a string-valued finding attribute, or `None`
/// when the attribute is not one this emulator knows. The attribute names are the
/// filter member names, which are also the names `SortCriterion$Field` uses.
pub fn string_attribute<'a>(
    finding: &'a AwsSecurityFinding,
    attribute: &str,
) -> Option<Vec<&'a str>> {
    let one = |value: Option<&'a str>| Some(value.into_iter().collect::<Vec<&'a str>>());
    match attribute {
        "Id" => one(Some(finding.id.as_str())),
        "ProductArn" => one(Some(finding.product_arn.as_str())),
        "AwsAccountId" => one(Some(finding.aws_account_id.as_str())),
        "AwsAccountName" => one(finding.aws_account_name.as_deref()),
        "GeneratorId" => one(Some(finding.generator_id.as_str())),
        "Region" => one(finding.region.as_deref()),
        "ProductName" => one(finding.product_name.as_deref()),
        "CompanyName" => one(finding.company_name.as_deref()),
        "Title" => one(Some(finding.title.as_str())),
        "Description" => one(Some(finding.description.as_str())),
        "SourceUrl" => one(finding.source_url.as_deref()),
        "SeverityLabel" => one(finding.severity.as_ref().and_then(|s| s.label.as_deref())),
        "RecordState" => one(finding.record_state.as_deref()),
        "WorkflowState" => one(finding.workflow_state.as_deref()),
        "WorkflowStatus" => one(finding.workflow.as_ref().and_then(|w| w.status.as_deref())),
        "VerificationState" => one(finding.verification_state.as_deref()),
        "ComplianceStatus" => one(finding.compliance.as_ref().and_then(|c| c.status.as_deref())),
        "ComplianceSecurityControlId" => one(finding
            .compliance
            .as_ref()
            .and_then(|c| c.security_control_id.as_deref())),
        "RecommendationText" => one(finding
            .remediation
            .as_ref()
            .and_then(|r| r.recommendation.as_ref())
            .and_then(|r| r.text.as_deref())),
        "NoteText" => one(finding.note.as_ref().map(|n| n.text.as_str())),
        "NoteUpdatedBy" => one(finding.note.as_ref().map(|n| n.updated_by.as_str())),
        "Type" => Some(
            finding
                .types
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(String::as_str)
                .collect(),
        ),
        _ => None,
    }
}

/// The value a finding carries for a number-valued finding attribute; the outer
/// `None` means the attribute is not one this emulator knows.
pub fn number_attribute(finding: &AwsSecurityFinding, attribute: &str) -> Option<Option<f64>> {
    match attribute {
        "Confidence" => Some(finding.confidence.map(f64::from)),
        "Criticality" => Some(finding.criticality.map(f64::from)),
        "SeverityNormalized" => Some(
            finding
                .severity
                .as_ref()
                .and_then(|s| s.normalized)
                .map(f64::from),
        ),
        "SeverityProduct" => Some(finding.severity.as_ref().and_then(|s| s.product)),
        _ => None,
    }
}

/// The value a finding carries for a timestamp-valued finding attribute; the outer
/// `None` means the attribute is not one this emulator knows.
pub fn date_attribute<'a>(
    finding: &'a AwsSecurityFinding,
    attribute: &str,
) -> Option<Option<&'a str>> {
    match attribute {
        "CreatedAt" => Some(Some(finding.created_at.as_str())),
        "UpdatedAt" => Some(Some(finding.updated_at.as_str())),
        "FirstObservedAt" => Some(finding.first_observed_at.as_deref()),
        "LastObservedAt" => Some(finding.last_observed_at.as_deref()),
        "ProcessedAt" => Some(finding.processed_at.as_deref()),
        "NoteUpdatedAt" => Some(finding.note.as_ref().map(|n| n.updated_at.as_str())),
        _ => None,
    }
}

/// The value a finding carries for a boolean-valued finding attribute; the outer
/// `None` means the attribute is not one this emulator knows.
pub fn boolean_attribute(finding: &AwsSecurityFinding, attribute: &str) -> Option<Option<bool>> {
    match attribute {
        "Sample" => Some(finding.sample),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Automation rules
// ---------------------------------------------------------------------------

/// `RuleStatus`' two values (`smithy_query(query="shape", shape="RuleStatus")`).
pub const RULE_STATUS_ENABLED: &str = "ENABLED";
/// See [`RULE_STATUS_ENABLED`].
pub const RULE_STATUS_DISABLED: &str = "DISABLED";

/// The only `AutomationRulesActionType` the model defines
/// (`smithy_query(query="shape", shape="AutomationRulesActionType")`), and the only
/// action kind a rule can therefore carry.
pub const ACTION_TYPE_FINDING_FIELDS_UPDATE: &str = "FINDING_FIELDS_UPDATE";

/// `IsTerminal`'s documented default: "By default, a rule isn't terminal."
pub const DEFAULT_IS_TERMINAL: bool = false;

/// `RuleStatus` is optional and says the rule "is active after it is created", with
/// `BatchUpdateAutomationRules` named as the way to change that value afterwards: a
/// create that does not name a status produces the active rule.
pub const DEFAULT_RULE_STATUS: &str = RULE_STATUS_ENABLED;

/// The quota on automation rules per account and Region, from the AWS Security Hub
/// CSPM quotas page ("Automation rules — 100 per account per Region"). The create
/// that would exceed it is what `LimitExceededException` answers.
pub const MAX_AUTOMATION_RULES_PER_HUB: usize = 100;

/// The `UnprocessedAutomationRule$ErrorCode` for a rule ARN that names no rule in
/// the caller's account and Region: the status of the `ResourceNotFoundException`
/// that same condition carries when it refuses a whole request.
pub const RULE_ERROR_CODE_NOT_FOUND: i32 = 404;

/// The `UnprocessedAutomationRule$ErrorMessage` for that same condition: the batch
/// reports the CLASS of the per-item failure — the name of the exception the condition
/// raises when it refuses a whole request — and leaves the identifier to the sibling
/// `RuleArn` member rather than repeating it in the text. One definition, so the three
/// batch rule operations cannot word the same per-item failure differently.
pub const RULE_ERROR_MESSAGE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The longest `RuleName` this store keeps for an automation rule. The model
/// documents no bound for that member, so the cap mirrors the one this crate already
/// applies to a finding's `Title` (`MAX_TITLE_LEN` in `ops/batch_import_findings.rs`):
/// one record's size must not grow with whatever a caller sends.
pub const MAX_RULE_NAME_LEN: usize = 256;

/// The longest `Description` this store keeps for an automation rule, mirroring the
/// cap this crate applies to a finding's `Description` (`MAX_DESCRIPTION_LEN` in
/// `ops/batch_import_findings.rs`) for the same reason as [`MAX_RULE_NAME_LEN`].
pub const MAX_RULE_DESCRIPTION_LEN: usize = 1024;

/// Why a rule's `RuleName` / `Description` cannot be stored, or `None` when both are
/// within the caps above. Each argument is `None` for a member the request left out,
/// which an update may do.
pub fn rule_text_violation(rule_name: Option<&str>, description: Option<&str>) -> Option<String> {
    for (member, value, max) in [
        ("RuleName", rule_name, MAX_RULE_NAME_LEN),
        ("Description", description, MAX_RULE_DESCRIPTION_LEN),
    ] {
        if let Some(value) = value {
            if value.len() > max {
                return Some(format!(
                    "{member} must be at most {max} characters, got {}",
                    value.len()
                ));
            }
        }
    }
    None
}

/// `arn:aws:securityhub:<region>:<account>:automation-rule/<id>` for a freshly
/// minted rule, where the id is the 8-4-4-4-12 hex grouping a real rule ARN carries.
///
/// The id comes from [`mint_resource_id`], the construction every resource this
/// service mints already uses (`CreateInsight`, `CreateFindingAggregator`): SHA-256
/// over the hub, the store's monotonic write sequence and the clock, so two creates
/// cannot mint the same ARN even in one instant, and the id is derived from no
/// caller input.
pub fn new_automation_rule_arn(
    account_id: &str,
    region: &str,
    write_seq: u64,
    now_nanos: i128,
) -> String {
    let rule_id = mint_resource_id(
        &[account_id.as_bytes(), region.as_bytes()],
        write_seq,
        now_nanos,
    );
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("automation-rule/{rule_id}"),
    )
}

/// The `AutomationRulesFindingFilters` string members this emulator evaluates,
/// paired with the finding attribute each one filters. Every criteria member name is
/// also the finding attribute name, which is what lets a rule's `Criteria` reuse
/// [`string_attribute`].
pub fn criteria_string_bindings(
    criteria: &AutomationRulesFindingFilters,
) -> Vec<(&'static str, &[StringFilter])> {
    let bindings: [(&'static str, Option<&Vec<StringFilter>>); 19] = [
        ("ProductArn", criteria.product_arn.as_ref()),
        ("AwsAccountId", criteria.aws_account_id.as_ref()),
        ("AwsAccountName", criteria.aws_account_name.as_ref()),
        ("Id", criteria.id.as_ref()),
        ("GeneratorId", criteria.generator_id.as_ref()),
        ("Type", criteria.r#type.as_ref()),
        ("Title", criteria.title.as_ref()),
        ("Description", criteria.description.as_ref()),
        ("SourceUrl", criteria.source_url.as_ref()),
        ("ProductName", criteria.product_name.as_ref()),
        ("CompanyName", criteria.company_name.as_ref()),
        ("SeverityLabel", criteria.severity_label.as_ref()),
        ("ComplianceStatus", criteria.compliance_status.as_ref()),
        (
            "ComplianceSecurityControlId",
            criteria.compliance_security_control_id.as_ref(),
        ),
        ("VerificationState", criteria.verification_state.as_ref()),
        ("WorkflowStatus", criteria.workflow_status.as_ref()),
        ("RecordState", criteria.record_state.as_ref()),
        ("NoteText", criteria.note_text.as_ref()),
        ("NoteUpdatedBy", criteria.note_updated_by.as_ref()),
    ];
    bindings
        .into_iter()
        .filter_map(|(name, value)| value.map(|v| (name, v.as_slice())))
        .collect()
}

/// The number-filter criteria members this emulator evaluates.
pub fn criteria_number_bindings(
    criteria: &AutomationRulesFindingFilters,
) -> Vec<(&'static str, &[NumberFilter])> {
    let bindings: [(&'static str, Option<&Vec<NumberFilter>>); 2] = [
        ("Confidence", criteria.confidence.as_ref()),
        ("Criticality", criteria.criticality.as_ref()),
    ];
    bindings
        .into_iter()
        .filter_map(|(name, value)| value.map(|v| (name, v.as_slice())))
        .collect()
}

/// The date-filter criteria members this emulator evaluates.
pub fn criteria_date_bindings(
    criteria: &AutomationRulesFindingFilters,
) -> Vec<(&'static str, &[DateFilter])> {
    let bindings: [(&'static str, Option<&Vec<DateFilter>>); 5] = [
        ("CreatedAt", criteria.created_at.as_ref()),
        ("UpdatedAt", criteria.updated_at.as_ref()),
        ("FirstObservedAt", criteria.first_observed_at.as_ref()),
        ("LastObservedAt", criteria.last_observed_at.as_ref()),
        ("NoteUpdatedAt", criteria.note_updated_at.as_ref()),
    ];
    bindings
        .into_iter()
        .filter_map(|(name, value)| value.map(|v| (name, v.as_slice())))
        .collect()
}

/// The first `Criteria` member the caller set that this emulator does not evaluate.
/// Reported as a 501 by the operation that was handed it, never ignored: a rule
/// stored with a criterion nothing evaluates would act on findings it must not act
/// on.
pub fn unsupported_criteria_member(
    criteria: &AutomationRulesFindingFilters,
) -> Option<&'static str> {
    let set: [(&'static str, bool); 15] = [
        ("ResourceType", criteria.resource_type.is_some()),
        ("ResourceId", criteria.resource_id.is_some()),
        ("ResourcePartition", criteria.resource_partition.is_some()),
        ("ResourceRegion", criteria.resource_region.is_some()),
        ("ResourceTags", criteria.resource_tags.is_some()),
        (
            "ResourceDetailsOther",
            criteria.resource_details_other.is_some(),
        ),
        (
            "ComplianceAssociatedStandardsId",
            criteria.compliance_associated_standards_id.is_some(),
        ),
        (
            "RelatedFindingsProductArn",
            criteria.related_findings_product_arn.is_some(),
        ),
        ("RelatedFindingsId", criteria.related_findings_id.is_some()),
        ("UserDefinedFields", criteria.user_defined_fields.is_some()),
        (
            "ResourceApplicationArn",
            criteria.resource_application_arn.is_some(),
        ),
        (
            "ResourceApplicationName",
            criteria.resource_application_name.is_some(),
        ),
        ("ResourceProvider", criteria.resource_provider.is_some()),
        (
            "ResourceOwnerAccountId",
            criteria.resource_owner_account_id.is_some(),
        ),
        (
            "ResourceOwnerOrgId",
            criteria.resource_owner_org_id.is_some(),
        ),
    ];
    set.into_iter()
        .find_map(|(name, present)| present.then_some(name))
}

/// Refuse a `Criteria` the filter shapes' own rules reject, BEFORE the rule is
/// stored: a rule whose criteria cannot be evaluated could never be applied, so the
/// create or update that carries it is a 400 rather than a rule that silently never
/// (or always) matches.
///
/// The published breadth and depth limits on a finding filter
/// ([`MAX_FILTER_ATTRIBUTES`] / [`MAX_FILTER_VALUES_PER_ATTRIBUTE`]) are enforced
/// here too, exactly as `GetFindings` enforces them on the same shapes: a stored
/// rule is evaluated against every finding of every later write, so an unbounded
/// criteria would be an unbounded amount of work per import, and an unbounded record
/// to clone, for one create.
pub fn validate_criteria(criteria: &AutomationRulesFindingFilters) -> Result<(), String> {
    let mut attributes = 0usize;
    for (attribute, count) in criteria_string_bindings(criteria)
        .iter()
        .map(|(name, filters)| (*name, filters.len()))
        .chain(
            criteria_number_bindings(criteria)
                .iter()
                .map(|(name, filters)| (*name, filters.len())),
        )
        .chain(
            criteria_date_bindings(criteria)
                .iter()
                .map(|(name, filters)| (*name, filters.len())),
        )
    {
        // An attribute with an empty filter list expresses no condition, so it does
        // not count toward the breadth limit.
        if count == 0 {
            continue;
        }
        attributes += 1;
        if count > MAX_FILTER_VALUES_PER_ATTRIBUTE {
            return Err(format!(
                "Filter {attribute}: at most {MAX_FILTER_VALUES_PER_ATTRIBUTE} filter values are allowed per attribute, got {count}"
            ));
        }
    }
    if attributes > MAX_FILTER_ATTRIBUTES {
        return Err(format!(
            "Criteria: at most {MAX_FILTER_ATTRIBUTES} finding attributes may be filtered on, got {attributes}"
        ));
    }

    for (attribute, filters) in criteria_string_bindings(criteria) {
        check_string_filter_combination(attribute, filters)?;
        // A filter with no `Value` names no predicate; a rule may not act on
        // findings a criterion did not actually select.
        if filters.iter().any(|f| f.value.is_none()) {
            return Err(format!("Filter {attribute}: every filter must carry a Value"));
        }
    }
    for (attribute, filters) in criteria_number_bindings(criteria) {
        if filters.iter().any(|f| {
            f.eq.is_none() && f.gte.is_none() && f.lte.is_none() && f.gt.is_none() && f.lt.is_none()
        }) {
            return Err(format!(
                "Filter {attribute}: every filter must carry at least one of Eq, Gt, Gte, Lt or Lte"
            ));
        }
    }
    for (attribute, filters) in criteria_date_bindings(criteria) {
        for filter in filters {
            if filter.start.is_none() && filter.end.is_none() && filter.date_range.is_none() {
                return Err(format!(
                    "Filter {attribute}: every filter must carry Start, End or DateRange"
                ));
            }
            for (member, bound) in [("Start", filter.start.as_deref()), ("End", filter.end.as_deref())]
            {
                if let Some(bound) = bound {
                    if parse_iso8601_nanos(bound).is_none() {
                        return Err(format!(
                            "Filter {attribute}: {member} must be an ISO-8601 timestamp, got {bound}"
                        ));
                    }
                }
            }
            if let Some(range) = filter.date_range.as_ref() {
                date_range_nanos(attribute, range)?;
                match range.comparison.as_deref().unwrap_or("WITHIN") {
                    "WITHIN" | "OLDER_THAN" => {}
                    other => {
                        return Err(format!(
                            "Filter {attribute}: DateRange.Comparison {other} is not a supported comparison"
                        ))
                    }
                }
            }
        }
    }
    Ok(())
}

/// `FindingHistoryUpdateSource.Type` for a change `BatchImportFindings` made; one of
/// the two values `FindingHistoryUpdateSourceType` models.
pub const HISTORY_SOURCE_BATCH_IMPORT_FINDINGS: &str = "BATCH_IMPORT_FINDINGS";

/// `FindingHistoryUpdateSource.Type` for a change `BatchUpdateFindings` made.
pub const HISTORY_SOURCE_BATCH_UPDATE_FINDINGS: &str = "BATCH_UPDATE_FINDINGS";

/// The top-level `AwsSecurityFinding` members that carry a timestamp, which
/// `GetFindingHistory` excludes from a change event: "The history includes changes
/// made to any fields in the Amazon Web Services Security Finding Format (ASFF)
/// except top-level timestamp fields, such as the CreatedAt and UpdatedAt fields"
/// (`smithy_query(query="operation", operation="GetFindingHistory")` docs). These are
/// every member of `AwsSecurityFinding` whose documentation calls it a timestamp
/// (`smithy_query(query="shape", shape="AwsSecurityFinding")`).
pub const HISTORY_EXCLUDED_FIELDS: [&str; 5] = [
    "CreatedAt",
    "UpdatedAt",
    "FirstObservedAt",
    "LastObservedAt",
    "ProcessedAt",
];

/// The IAM user name the emulator attributes every signed request to, and the one
/// `GetCallerIdentity` answers with: see `AMBIENT_USER_NAME` in
/// `services/service-com-amazonaws-sts-awssecuritytokenservicev20110615/src/state.rs`
/// ("The identity of the ambient caller: the IAM user whose long-term key this
/// emulator publishes").
pub const CALLER_USER_NAME: &str = "emulator";

/// The IAM principal a request is attributed to, which is what names the customer
/// that called `BatchUpdateFindings` in a change event's `UpdateSource.Identity`.
///
/// The emulator publishes ONE long-term identity for every signed request, and STS's
/// `GetCallerIdentity` is where a caller reads it; a finding change event names the
/// caller with that same ARN rather than minting a second principal for the one
/// credential.
pub fn caller_arn() -> String {
    ::engine::emulator_arn("iam", "", &format!("user/{CALLER_USER_NAME}"))
}

/// Each top-level ASFF field of a finding, keyed by the `AwsSecurityFinding` member
/// name, rendered as the text a change event reports for it.
///
/// The finding is rendered with the protocol's own JSON codec, so the keys are the
/// modelled ASFF field names however a client sent them, and a field this emulator
/// never special-cases is still accounted for. A change event names "The ASFF field
/// that changed" (`smithy_query(query="doc", shape="FindingHistoryUpdate",
/// member="UpdatedField")`), and an ASFF field is one member of the finding: a scalar
/// renders as its own text, and a structure, list or map renders as its JSON, so one
/// changed field is one entry whatever its shape. An absent or empty field is no entry
/// at all: `FindingHistoryUpdate`'s `OldValue` / `NewValue` are `NonEmptyString`.
fn finding_field_values(finding: &AwsSecurityFinding) -> BTreeMap<String, String> {
    use ::aws_smithy_schema::codec::{Codec, FinishSerializer};
    use ::aws_smithy_schema::serde::ShapeSerializer;

    let mut serializer = ::aws_smithy_schema_server::json::JsonCodec::rest_json().create_serializer();
    let mut values = BTreeMap::new();
    if serializer
        .write_struct(&AwsSecurityFinding::SCHEMA, finding)
        .is_err()
    {
        return values;
    }
    let Ok(rendered) = ::serde_json::from_slice::<::serde_json::Value>(&serializer.finish()) else {
        return values;
    };
    let ::serde_json::Value::Object(fields) = rendered else {
        return values;
    };
    for (name, value) in fields {
        match value {
            ::serde_json::Value::Null => {}
            ::serde_json::Value::String(text) => {
                if !text.trim().is_empty() {
                    values.insert(name, text);
                }
            }
            other => {
                let rendered = other.to_string();
                // An empty list or map is no value, like an absent member: clearing a
                // collection reads as a field that no longer has a value.
                if rendered != "[]" && rendered != "{}" {
                    values.insert(name, rendered);
                }
            }
        }
    }
    values
}

/// The fields a finding was created with, as the creating event's `Updates`: one
/// entry per ASFF field the import set, each with no prior value, with the top-level
/// timestamp members ([`HISTORY_EXCLUDED_FIELDS`]) left out.
pub fn finding_created_fields(finding: &AwsSecurityFinding) -> Vec<FindingHistoryUpdate> {
    finding_field_values(finding)
        .into_iter()
        .filter(|(field, _)| !HISTORY_EXCLUDED_FIELDS.contains(&field.as_str()))
        .map(|(field, value)| FindingHistoryUpdate {
            updated_field: Some(field),
            old_value: None,
            new_value: Some(value),
        })
        .collect()
}

/// The ASFF fields that differ between a finding before and after a write, as a
/// change event's `Updates`: one entry per field whose value changed, in field-name
/// order, with the top-level timestamp members ([`HISTORY_EXCLUDED_FIELDS`]) left out.
pub fn finding_changes(
    before: &AwsSecurityFinding,
    after: &AwsSecurityFinding,
) -> Vec<FindingHistoryUpdate> {
    let before_values = finding_field_values(before);
    let after_values = finding_field_values(after);
    let mut changes = Vec::new();
    let mut fields: Vec<&String> = before_values.keys().chain(after_values.keys()).collect();
    fields.sort_unstable();
    fields.dedup();
    for field in fields {
        if HISTORY_EXCLUDED_FIELDS.contains(&field.as_str()) {
            continue;
        }
        let old_value = before_values.get(field);
        let new_value = after_values.get(field);
        if old_value == new_value {
            continue;
        }
        changes.push(FindingHistoryUpdate {
            updated_field: Some(field.clone()),
            old_value: old_value.cloned(),
            new_value: new_value.cloned(),
        });
    }
    changes
}

/// Does this finding satisfy every criteria member the rule set? Every member is
/// joined by `AND`, as in `GetFindings$Filters`: "a finding matches the conditions
/// specified in this parameter" is one condition set, not a choice between them.
///
/// `Err` carries the text of the `InvalidInputException` the create or update that
/// stored this criteria should have been refused with; [`validate_criteria`] refuses
/// it there, so a stored rule cannot reach it.
pub fn finding_matches_criteria(
    finding: &AwsSecurityFinding,
    criteria: &AutomationRulesFindingFilters,
    now_nanos: i128,
) -> Result<bool, String> {
    for (attribute, filters) in criteria_string_bindings(criteria) {
        let values = string_attribute(finding, attribute).unwrap_or_default();
        if !string_filters_match(&values, filters) {
            return Ok(false);
        }
    }
    for (attribute, filters) in criteria_number_bindings(criteria) {
        let value = number_attribute(finding, attribute).unwrap_or_default();
        if !number_filters_match(value, filters) {
            return Ok(false);
        }
    }
    for (attribute, filters) in criteria_date_bindings(criteria) {
        let value = date_attribute(finding, attribute).unwrap_or_default();
        if !date_filters_match(attribute, value, filters, now_nanos)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Apply one `FINDING_FIELDS_UPDATE` action to a finding.
///
/// `AutomationRulesFindingFieldsUpdate` names the same finding fields
/// `BatchUpdateFindingsRequest` does, so a member the action carries overwrites that
/// field and a member it omits leaves the field as it was — the same semantics
/// `BatchUpdateFindings` applies, because it is the same update.
pub fn apply_finding_fields_update(
    finding: &mut AwsSecurityFinding,
    update: &AutomationRulesFindingFieldsUpdate,
    applied_at: &str,
) {
    if let Some(note) = update.note.as_ref() {
        finding.note = Some(Note {
            text: note.text.clone(),
            updated_by: note.updated_by.clone(),
            updated_at: applied_at.to_string(),
        });
    }
    if let Some(severity) = update.severity.as_ref() {
        // `Product` and `Original` are provider-owned and are not part of
        // `SeverityUpdate`'s Label/Normalized pair, so they survive an update that
        // does not carry them.
        let previous = finding.severity.clone().unwrap_or_default();
        let mut updated = Severity {
            product: severity.product.or(previous.product),
            label: severity.label.clone(),
            normalized: severity.normalized,
            original: previous.original,
        };
        complete_severity(&mut updated);
        finding.severity = Some(updated);
    }
    if let Some(verification_state) = update.verification_state.as_ref() {
        finding.verification_state = Some(verification_state.clone());
    }
    if let Some(confidence) = update.confidence {
        finding.confidence = Some(confidence);
    }
    if let Some(criticality) = update.criticality {
        finding.criticality = Some(criticality);
    }
    if let Some(types) = update.types.as_ref() {
        finding.types = Some(types.clone());
    }
    if let Some(user_defined_fields) = update.user_defined_fields.as_ref() {
        finding.user_defined_fields = Some(user_defined_fields.clone());
    }
    if let Some(status) = update.workflow.as_ref().and_then(|w| w.status.as_ref()) {
        finding.workflow = Some(Workflow {
            status: Some(status.clone()),
        });
    }
    if let Some(related_findings) = update.related_findings.as_ref() {
        finding.related_findings = Some(related_findings.clone());
    }
}

impl ComAmazonawsSecurityhubSecurityhubapiserviceState {
    /// The hub's rules in the order Security Hub CSPM applies them, `ENABLED` and
    /// `DISABLED` alike — what `ListAutomationRules` reports: "The response includes
    /// rules with a RuleStatus of ENABLED and DISABLED."
    ///
    /// The order is the applied one: "Security Hub CSPM applies rules with lower
    /// values for this parameter first" (`RuleOrderValue`), with the creation
    /// sequence breaking a tie so the order is total and two findings can never be
    /// acted on in different orders. This is the single place that order is defined.
    ///
    /// Cloned rather than borrowed so a caller can keep mutating the findings map
    /// while it applies them; a hub holds at most
    /// [`MAX_AUTOMATION_RULES_PER_HUB`] rules.
    pub fn rules_in_order(&self, hub: &HubKey) -> Vec<AutomationRule> {
        let mut rules: Vec<AutomationRule> = self
            .automation_rules
            .iter()
            .filter(|(key, _)| key.account_id == hub.0 && key.region == hub.1)
            .map(|(_, rule)| rule.clone())
            .collect();
        rules.sort_by(|left, right| {
            left.rule_order
                .cmp(&right.rule_order)
                .then_with(|| left.created_seq.cmp(&right.created_seq))
        });
        rules
    }

    /// [`Self::rules_in_order`] narrowed to the rules that are `ENABLED`: the ones a
    /// write actually applies, since a `DISABLED` rule changes nothing.
    pub fn enabled_rules_in_order(&self, hub: &HubKey) -> Vec<AutomationRule> {
        let mut rules = self.rules_in_order(hub);
        rules.retain(|rule| rule.rule_status == RULE_STATUS_ENABLED);
        rules
    }
}

/// Apply `rules` to a finding that is being written, in the order they were given.
///
/// A rule acts only on a finding that matches its `Criteria`, and a matching rule
/// that `IsTerminal` ends the walk: "If a rule is terminal, Security Hub CSPM
/// applies the rule action to a finding that matches the rule criteria and doesn't
/// evaluate other rules for the finding." Rules the caller filtered to `ENABLED`
/// are the only ones passed in, which is what makes a `DISABLED` rule a rule that
/// changes nothing.
///
/// Answers the ARNs of the rules that acted, so a caller can tell "no rule matched"
/// from "a rule matched and changed nothing".
pub fn apply_automation_rules(
    rules: &[AutomationRule],
    finding: &mut AwsSecurityFinding,
    applied_at: &str,
    now_nanos: i128,
) -> Vec<String> {
    let mut applied: Vec<String> = Vec::new();
    for rule in rules {
        let matches = match finding_matches_criteria(finding, &rule.criteria, now_nanos) {
            Ok(matches) => matches,
            Err(message) => {
                // Unreachable: `validate_criteria` refused this criteria when the
                // rule was created or updated. Reported rather than silently
                // dropped, so a regression there is visible instead of a rule that
                // quietly stops acting.
                debug_assert!(
                    false,
                    "stored rule {} carries criteria validate_criteria would refuse: {message}",
                    rule.rule_arn
                );
                ::tracing::warn!(
                    rule_arn = %rule.rule_arn,
                    reason = %message,
                    "skipping an automation rule whose criteria cannot be evaluated"
                );
                false
            }
        };
        if !matches {
            continue;
        }
        for action in &rule.actions {
            if action.r#type.as_deref() != Some(ACTION_TYPE_FINDING_FIELDS_UPDATE) {
                continue;
            }
            if let Some(update) = action.finding_fields_update.as_ref() {
                apply_finding_fields_update(finding, update, applied_at);
            }
        }
        applied.push(rule.rule_arn.clone());
        if rule.is_terminal {
            break;
        }
    }
    applied
}

/// A custom insight's identity: the hub it lives in, plus the ARN `CreateInsight`
/// minted for it. The ARN alone would be unique, but the account/Region prefix is
/// what makes "disable removes this account's insights in this Region" expressible,
/// exactly as it does for [`FindingKey`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InsightKey {
    pub account_id: String,
    pub region: String,
    pub insight_arn: String,
}

/// A custom insight as `CreateInsight` stored it: the three members the modelled
/// `Insight` shape carries beyond its ARN. `GetInsightResults` evaluates it against
/// the hub's findings.
///
/// `Filters` names its type by path rather than through this module's `use`, so
/// adding an insight store costs no line in an import list every new operation of
/// this service also edits.
#[derive(Debug, Clone)]
pub struct StoredInsight {
    pub name: String,
    pub filters: crate::ops::types::AwsSecurityFindingFilters,
    pub group_by_attribute: String,
}

/// `arn:aws:securityhub:<region>:<account>:insight/<account>/custom/<id>` — the ARN
/// Security Hub CSPM mints for a CUSTOM insight (the insights a caller creates,
/// as opposed to the service's own managed ones).
pub fn custom_insight_arn(key: &HubKey, insight_id: &str) -> String {
    // `emulator_arn` stamps the canonical emulator account id; the key's account is
    // that same id, so the two cannot drift.
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        &key.1,
        &format!("insight/{}/custom/{insight_id}", key.0),
    )
}

// ---------------------------------------------------------------------------
// Cross-Region aggregation (finding aggregators). Kept together at the end of
// this module so the whole resource — its record, its bounds and its ARN
// helpers — reads in one place.
// ---------------------------------------------------------------------------

/// The account's cross-Region aggregation settings, as `CreateFindingAggregator`
/// stored them and `GetFindingAggregator` / `ListFindingAggregators` report them.
///
/// Named with a `Stored` prefix because the modelled `FindingAggregator` shape in
/// `ops::types` carries only the ARN; this is the whole resource behind it.
#[derive(Debug, Clone)]
pub struct StoredFindingAggregator {
    pub arn: String,
    /// The home Region: "This operation can be invoked from the home Region only"
    /// (`smithy_query(query="operation", operation="CreateFindingAggregator")` doc),
    /// so the Region the create was signed for IS the home Region.
    pub finding_aggregation_region: String,
    pub region_linking_mode: String,
    /// The excluded or included Regions. Empty for a `RegionLinkingMode` that names
    /// no Region list.
    pub regions: Vec<String>,
}

/// How many Regions a finding aggregator's `Regions` list may name. `Regions` is a
/// `StringList` the model puts no `@length` trait on, and the list enumerates Regions
/// of ONE partition — every AWS partition has well under this many — so the cap is
/// above anything a faithful caller sends while keeping a stored record's size
/// independent of what a caller chooses to send.
pub const MAX_AGGREGATOR_REGIONS: usize = 64;

/// The longest Region name a finding aggregator's `Regions` list may carry. A Region
/// name is a short dash-separated code (`ap-southeast-4` is 14 characters), so this
/// is far above any real one; the model documents no bound for the member, so this is
/// what keeps one entry from growing with whatever a caller sends.
pub const MAX_REGION_NAME_LEN: usize = 64;

/// The resource type of a finding aggregator ARN
/// (`arn:aws:securityhub:<home-region>:<account>:finding-aggregator/<id>`).
pub const FINDING_AGGREGATOR_RESOURCE_TYPE: &str = "finding-aggregator";

/// The documented `RegionLinkingMode` values, in the order
/// `smithy_query(query="doc", shape="CreateFindingAggregatorRequest",
/// member="RegionLinkingMode")` lists them: `ALL_REGIONS`,
/// `ALL_REGIONS_EXCEPT_SPECIFIED`, `SPECIFIED_REGIONS`, `NO_REGIONS`. The member is
/// modelled `NonEmptyString`, not an enum, so the handler is what refuses anything
/// else.
pub const REGION_LINKING_MODES: [&str; 4] = [
    "ALL_REGIONS",
    "ALL_REGIONS_EXCEPT_SPECIFIED",
    "SPECIFIED_REGIONS",
    "NO_REGIONS",
];

/// Whether a `RegionLinkingMode` consumes the `Regions` list. `Regions` is
/// documented as "the list of Regions that don't replicate" for
/// `ALL_REGIONS_EXCEPT_SPECIFIED` and "the list of Regions that do replicate" for
/// `SPECIFIED_REGIONS` — those two modes read it; `ALL_REGIONS` and `NO_REGIONS`
/// name no list at all (`smithy_query(query="doc",
/// shape="CreateFindingAggregatorRequest", member="Regions")`).
pub fn region_linking_mode_takes_regions(mode: &str) -> bool {
    matches!(mode, "ALL_REGIONS_EXCEPT_SPECIFIED" | "SPECIFIED_REGIONS")
}

/// A fresh finding aggregator id, in the 8-4-4-4-12 hex grouping Security Hub CSPM's
/// own finding aggregator ARNs carry.
///
/// Minted through [`mint_resource_id`] from the account, the home Region, the store's
/// monotonic write sequence and the clock, so two creates — even in the same instant,
/// even after a delete — cannot mint the same id, and the id is derived from no caller
/// input.
pub fn new_finding_aggregator_id(
    account_id: &str,
    region: &str,
    write_seq: u64,
    now_nanos: i128,
) -> String {
    mint_resource_id(
        &[account_id.as_bytes(), region.as_bytes()],
        write_seq,
        now_nanos,
    )
}

/// A 128-bit identifier for a resource this service mints, rendered in the 8-4-4-4-12
/// hex grouping Security Hub CSPM's own resource ids carry.
///
/// The bits come from SHA-256 over `fields` (each one separated, so two different field
/// splits cannot hash alike), the store's monotonic write sequence and the clock, so two
/// creates cannot mint the same identifier — not even when a deleted resource's fields
/// are used again in the same instant, because the write sequence has moved on. Shared
/// by every operation here that mints such an id (`CreateInsight`,
/// `CreateFindingAggregator`) so the construction is stated once.
pub fn mint_resource_id(fields: &[&[u8]], write_seq: u64, now_nanos: i128) -> String {
    use ::sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    for field in fields {
        hasher.update(field);
        // A separator, so two different field splits cannot hash alike.
        hasher.update([0u8]);
    }
    hasher.update(write_seq.to_be_bytes());
    hasher.update(now_nanos.to_be_bytes());
    let digest = hasher.finalize();
    let hex = ::hex::encode(&digest[..16]);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// `arn:aws:securityhub:<home-region>:<account>:finding-aggregator/<id>`.
pub fn finding_aggregator_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{FINDING_AGGREGATOR_RESOURCE_TYPE}/{id}"),
    )
}
