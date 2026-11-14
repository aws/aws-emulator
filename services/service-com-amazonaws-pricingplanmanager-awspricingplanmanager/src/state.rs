// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-pricingplanmanager-awspricingplanmanager service.
//!
//! The service has one primary resource, the `Subscription`, so this module holds
//! the stored form of that record plus the few rules every handler shares: how an
//! `ETag` is derived, how a `nextToken` is encoded, when the current billing period
//! ends, and how the published plan tiers rank. Handlers compute their responses
//! from [`SubscriptionRecord`]; nothing on the wire is synthesised per request.

use std::collections::{BTreeMap, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    ScheduledChange, Subscription, SubscriptionSummary, APPROVAL_MODE_VALUES,
    SCHEDULED_CHANGE_TYPE_VALUES, STATUS_VALUES,
};

/// Whether the resources a subscription names are eligible for the plan family it asks
/// for — a question about state ANOTHER service holds, so it lives on its own.
pub mod resource_eligibility;

pub use resource_eligibility::{
    ineligible_resources, ineligible_resources_message, CLOUDFRONT_PLAN_FAMILY,
};

/// The region an unsigned request is served in. A signed request carries its own
/// region in the SigV4 credential scope; see [`request_region`].
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN namespace this service's resources live under (`OPERATIONS.md`, "ARN
/// namespace").
pub const ARN_NAMESPACE: &str = "pricingplanmanager";

/// How many subscriptions one emulator holds at once.
///
/// `CreateSubscription` models `ServiceQuotaExceededException` (HTTP 402) — "You have
/// reached the maximum number of subscriptions allowed for your account" — but AWS
/// publishes no number for it, so this bound is the emulator's own. It is what keeps
/// [`ComAmazonawsPricingplanmanagerAwspricingplanmanagerState`] finite: both of its maps
/// are bounded by it (see [`ComAmazonawsPricingplanmanagerAwspricingplanmanagerState::remove_subscription`]),
/// and so is the per-request cost of [`ComAmazonawsPricingplanmanagerAwspricingplanmanagerState::apply_due_changes`],
/// so no volume of caller traffic can grow the store or the work per call without limit.
pub const MAX_SUBSCRIPTIONS: usize = 200;

/// How many `SubscriptionSummary` values one `ListSubscriptions` page carries.
///
/// `ListSubscriptions` models no `maxResults` member, so the page size is the
/// service's own choice; it is bounded so paging is real — a store larger than one
/// page is handed back across several pages joined by `nextToken`.
pub const LIST_PAGE_SIZE: usize = 10;

/// The published plan tiers, lowest to highest.
///
/// Source: the `Subscription$planTier` member doc — "The current tier level of the
/// pricing plan, such as `FREE`, `PRO`, `BUSINESS`, or `PREMIUM`"
/// (`smithy_query(query="shape", shape="Subscription")`) — which lists them in
/// ascending order. `planTier` is a plain string in the model, so a tier outside
/// this list is accepted and simply has no rank.
pub const PLAN_TIERS_ASCENDING: &[&str] = &["FREE", "PRO", "BUSINESS", "PREMIUM"];

/// The tier that is always activated immediately: `CreateSubscription`'s doc says
/// "Free-tier subscriptions are always activated immediately regardless of approval
/// mode".
pub const FREE_PLAN_TIER: &str = "FREE";

/// The rank of `tier` in [`PLAN_TIERS_ASCENDING`], or `None` for a tier the service
/// docs do not publish.
pub fn plan_tier_rank(tier: &str) -> Option<usize> {
    PLAN_TIERS_ASCENDING.iter().position(|known| *known == tier)
}

/// Whether `tier` is the free tier, which is never held for approval.
pub fn is_free_plan_tier(tier: &str) -> bool {
    tier == FREE_PLAN_TIER
}

/// The modelled `Status` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Created with manual approval; waiting for an `ApprovePaidSubscription` call.
    PendingApproval,
    /// Active; the resources are covered by flat-rate pricing.
    Active,
    /// A change is being applied.
    SyncInProgress,
    /// The subscription encountered an error; `statusReason` carries the detail.
    Failed,
}

impl Status {
    /// The wire value, one of [`STATUS_VALUES`].
    pub fn wire(self) -> &'static str {
        let value = match self {
            Self::PendingApproval => "PENDING_APPROVAL",
            Self::Active => "ACTIVE",
            Self::SyncInProgress => "SYNC_IN_PROGRESS",
            Self::Failed => "FAILED",
        };
        debug_assert!(
            STATUS_VALUES.contains(&value),
            "{value} is not a modelled Status value"
        );
        value
    }
}

/// The modelled `ScheduledChangeType` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeType {
    /// The subscription tier is being lowered at the end of the billing period.
    Downgrade,
    /// The subscription is being terminated at the end of the billing period.
    Cancellation,
}

impl ChangeType {
    /// The wire value, one of [`SCHEDULED_CHANGE_TYPE_VALUES`].
    pub fn wire(self) -> &'static str {
        let value = match self {
            Self::Downgrade => "DOWNGRADE",
            Self::Cancellation => "CANCELLATION",
        };
        debug_assert!(
            SCHEDULED_CHANGE_TYPE_VALUES.contains(&value),
            "{value} is not a modelled ScheduledChangeType value"
        );
        value
    }
}

/// The modelled `ApprovalMode` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalMode {
    /// Requires a separate `ApprovePaidSubscription` call before billing starts.
    Manual,
    /// Activates the subscription right away.
    Immediate,
}

impl ApprovalMode {
    /// Parse a wire value from [`APPROVAL_MODE_VALUES`]; `None` for anything else.
    pub fn parse(value: &str) -> Option<Self> {
        let parsed = match value {
            "MANUAL" => Self::Manual,
            "IMMEDIATE" => Self::Immediate,
            _ => return None,
        };
        debug_assert!(APPROVAL_MODE_VALUES.contains(&value));
        Some(parsed)
    }
}

/// A pending change on a subscription, as stored.
#[derive(Debug, Clone)]
pub struct ScheduledChangeRecord {
    /// Which kind of change is pending.
    pub change_type: ChangeType,
    /// When the change takes effect.
    pub effective_date: Option<DateTime>,
    /// For a downgrade, the tier the subscription will move to.
    pub plan_tier: Option<String>,
    /// For a downgrade, the usage level after the change takes effect.
    pub usage_level: Option<String>,
}

impl ScheduledChangeRecord {
    /// The modelled `ScheduledChange` shape for this record.
    pub fn to_shape(&self) -> ScheduledChange {
        ScheduledChange {
            change_type: self.change_type.wire().to_string(),
            effective_date: self.effective_date,
            plan_tier: self.plan_tier.clone(),
            usage_level: self.usage_level.clone(),
        }
    }
}

/// A subscription, as stored. Every member of the modelled `Subscription` and
/// `SubscriptionSummary` shapes is read from here.
#[derive(Debug, Clone)]
pub struct SubscriptionRecord {
    /// The ARN that identifies this subscription, minted at create time.
    pub arn: String,
    /// The pricing plan family.
    pub plan_family: String,
    /// The current tier level.
    pub plan_tier: String,
    /// The usage level within the tier.
    pub usage_level: Option<String>,
    /// The current status.
    pub status: Status,
    /// Extra context for the current status, when there is any.
    pub status_reason: Option<String>,
    /// The resources covered by this subscription.
    pub resource_arns: Vec<String>,
    /// The change pending at the end of the current billing period, if any.
    pub scheduled_change: Option<ScheduledChangeRecord>,
    /// When the subscription was created.
    pub created_at: DateTime,
    /// When the subscription was last modified.
    pub updated_at: DateTime,
    /// The `clientToken` whose `CreateSubscription` call minted this subscription, when
    /// one was supplied. Never on the wire: it is held so that deleting the record also
    /// drops its entry from the idempotency map, which keeps that map bounded by the
    /// number of live subscriptions rather than by the number of calls ever made.
    pub client_token: Option<String>,
    /// This record's revision: how many times the record has been written, counting the
    /// writes the `CreateSubscription` call itself made. It IS the concurrency token
    /// [`Self::etag`] publishes, so only a write moves it.
    pub revision: u64,
}

/// The revision a subscription record carries when it has just been minted and nothing
/// else has been written to it yet.
///
/// Minting is itself a write, so the counter starts at one rather than at zero and the
/// token is never empty. A subscription the create also puts into service has taken a
/// second write by the time the create answers — see
/// [`SubscriptionRecord::record_initial_activation`].
pub const FIRST_REVISION: u64 = 1;

impl SubscriptionRecord {
    /// This record's entity tag: its [`revision`](Self::revision) in decimal.
    ///
    /// The token is a monotonic per-subscription revision rather than a digest over the
    /// record's content, so it is still derived from state and not minted per request:
    /// two reads of an unchanged subscription return the same token, and every mutation
    /// returns the next one.
    pub fn etag(&self) -> String {
        self.revision.to_string()
    }

    /// The modelled `Subscription` shape for this record.
    pub fn to_subscription(&self) -> Subscription {
        Subscription {
            arn: self.arn.clone(),
            plan_family: self.plan_family.clone(),
            plan_tier: self.plan_tier.clone(),
            usage_level: self.usage_level.clone(),
            scheduled_change: self.scheduled_change.as_ref().map(ScheduledChangeRecord::to_shape),
            status: self.status.wire().to_string(),
            status_reason: self.status_reason.clone(),
            resource_arns: self.resource_arns.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }

    /// The modelled `SubscriptionSummary` shape for this record. `eTag` is a BODY
    /// member here, not a response header, so it travels inside the summary.
    pub fn to_summary(&self) -> SubscriptionSummary {
        SubscriptionSummary {
            arn: self.arn.clone(),
            plan_family: self.plan_family.clone(),
            plan_tier: self.plan_tier.clone(),
            usage_level: self.usage_level.clone(),
            scheduled_change: self.scheduled_change.as_ref().map(ScheduledChangeRecord::to_shape),
            status: self.status.wire().to_string(),
            status_reason: self.status_reason.clone(),
            resource_arns: self.resource_arns.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            e_tag: self.etag(),
        }
    }

    /// Record a mutation made at `now`: bump `updatedAt` and the revision the
    /// eTag is, so the next read sees a new modification time and the next
    /// concurrency token.
    pub fn touch(&mut self, now: DateTime) {
        self.updated_at = now;
        self.revision = self.revision.saturating_add(1);
    }

    /// Record the write that puts a just-minted subscription into service.
    ///
    /// Creating a subscription that is not held for approval does two things to the
    /// record: it mints it, and it activates it so the flat-rate coverage takes effect
    /// ("Free-tier subscriptions are always activated immediately regardless of approval
    /// mode"; "When `approvalMode` is set to `IMMEDIATE` or is not specified, the
    /// subscription is activated immediately"). Both are writes, so an active
    /// subscription's first published revision is one past a mint, while a subscription
    /// waiting for `ApprovePaidSubscription` has taken the mint only and that approval is
    /// what advances it.
    pub fn record_initial_activation(&mut self, now: DateTime) {
        if self.status == Status::Active {
            self.touch(now);
        }
    }
}

/// Why a mutating operation could not reach the subscription it named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreconditionFailure {
    /// No subscription with that ARN is stored.
    NotFound,
    /// The caller's `If-Match` did not equal the record's current eTag.
    ETagMismatch,
}

/// How this service states an `If-Match` that does not match the stored eTag.
///
/// The wording follows `ConflictException`'s own documentation: "The request conflicts
/// with the current state of the resource. This typically occurs when the `ETag` value in
/// the `If-Match` header does not match the current version of the subscription. Retrieve
/// the latest version and retry."
pub const IF_MATCH_CONFLICT_MESSAGE: &str =
    "the If-Match header does not match the current version of the subscription; \
     retrieve the latest version and retry";

/// How every handler states a store it could not reach. Reaching it is infallible in
/// the assembled emulator, so this only covers an engine the service was not mounted on.
pub const STORE_UNAVAILABLE_MESSAGE: &str = "the pricing plan manager store is unavailable";

/// Per-service state for com-amazonaws-pricingplanmanager-awspricingplanmanager. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsPricingplanmanagerAwspricingplanmanagerState {
    /// Every subscription in the account, keyed by ARN. A `BTreeMap` so
    /// `ListSubscriptions` enumerates in one stable order its `nextToken` can
    /// resume from.
    pub subscriptions: BTreeMap<String, SubscriptionRecord>,
    /// `clientToken` -> the ARN the `CreateSubscription` call carrying it minted,
    /// so an `@idempotencyToken` replay returns the original subscription instead
    /// of creating a second one.
    pub idempotency: HashMap<String, String>,
    /// The next sequence number [`Self::next_subscription_sequence`] hands out. Never on
    /// the wire: it only guarantees that two subscriptions minted in the same instant
    /// still get different ARNs.
    pub subscription_sequence: u64,
}

impl ComAmazonawsPricingplanmanagerAwspricingplanmanagerState {
    /// The subscription `arn` names, for a reporting operation.
    pub fn subscription(&self, arn: &str) -> Option<&SubscriptionRecord> {
        self.subscriptions.get(arn)
    }

    /// Whether another subscription would exceed [`MAX_SUBSCRIPTIONS`].
    pub fn at_capacity(&self) -> bool {
        self.subscriptions.len() >= MAX_SUBSCRIPTIONS
    }

    /// Take the next subscription sequence number. Feeds [`mint_subscription_arn`].
    pub fn next_subscription_sequence(&mut self) -> u64 {
        let sequence = self.subscription_sequence;
        self.subscription_sequence = sequence.saturating_add(1);
        sequence
    }

    /// Store `record`, together with the idempotency entry for its `clientToken`.
    ///
    /// The caller must have checked [`Self::at_capacity`] first; this is the one place a
    /// subscription enters the store, so the two maps cannot drift apart.
    pub fn insert_subscription(&mut self, record: SubscriptionRecord) {
        if let Some(token) = record.client_token.clone() {
            self.idempotency.insert(token, record.arn.clone());
        }
        self.subscriptions.insert(record.arn.clone(), record);
    }

    /// Remove the subscription `arn` names, together with its idempotency entry.
    ///
    /// Dropping the token with the record is what bounds `idempotency`: it never holds
    /// more entries than there are live subscriptions, so a deleted subscription leaves
    /// nothing behind.
    pub fn remove_subscription(&mut self, arn: &str) -> Option<SubscriptionRecord> {
        let removed = self.subscriptions.remove(arn)?;
        if let Some(token) = removed.client_token.as_deref() {
            self.idempotency.remove(token);
        }
        Some(removed)
    }

    /// Resolve `arn` for a mutating call and enforce the caller's `If-Match`
    /// against the record's current eTag.
    pub fn resolve_for_mutation(
        &mut self,
        arn: &str,
        if_match: &str,
    ) -> Result<&mut SubscriptionRecord, PreconditionFailure> {
        let record = self
            .subscriptions
            .get_mut(arn)
            .ok_or(PreconditionFailure::NotFound)?;
        if record.etag() != if_match {
            return Err(PreconditionFailure::ETagMismatch);
        }
        Ok(record)
    }

    /// Apply every scheduled change whose effective instant has already passed.
    ///
    /// A `ScheduledChange` takes effect at the end of the billing period on its own —
    /// "the cancellation is scheduled to take effect at the end of the current billing
    /// period", "the current tier remains unchanged until the end of the billing cycle" —
    /// so by the time `now` is past that instant the change has happened whether or not
    /// anybody called. Every operation, reporting or mutating, brings the store up to
    /// the clock before it answers, which is why a read may write here: it only
    /// materialises what the clock already made due, and it is a no-op for a store with
    /// nothing due.
    ///
    /// Costs one pass over the store, which [`MAX_SUBSCRIPTIONS`] bounds, so the work a
    /// single call can cause is bounded by a constant rather than by caller traffic.
    pub fn apply_due_changes(&mut self, now: DateTime) {
        let mut cancelled: Vec<String> = Vec::new();
        for (arn, record) in &mut self.subscriptions {
            let Some(change) = record.scheduled_change.as_ref() else {
                continue;
            };
            // A change the billing system has not dated yet is not due; its
            // `effectiveDate` "is populated after the change is confirmed".
            let Some(effective) = change.effective_date else {
                continue;
            };
            if instant_key(effective) > instant_key(now) {
                continue;
            }
            match change.change_type {
                ChangeType::Cancellation => cancelled.push(arn.clone()),
                ChangeType::Downgrade => {
                    if let Some(tier) = change.plan_tier.clone() {
                        record.plan_tier = tier;
                    }
                    record.usage_level = change.usage_level.clone();
                    record.scheduled_change = None;
                    record.touch(now);
                }
            }
        }
        for arn in cancelled {
            self.remove_subscription(&arn);
        }
    }
}

/// An instant as an orderable key. `aws_smithy_types::DateTime` is compared through
/// this rather than relying on an ordering impl the type does not promise.
fn instant_key(value: DateTime) -> (i64, u32) {
    (value.secs(), value.subsec_nanos())
}

/// The current instant, at the millisecond resolution the model's `date-time`
/// timestamps carry.
pub fn now() -> DateTime {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default();
    DateTime::from_millis(millis)
}

/// `value` rendered as an RFC 3339 `date-time` by `aws-smithy-types` — the same
/// `DateTime` the wire serializer uses, so there is no second date implementation here.
/// `None` when the instant is not a date that type can express.
pub fn format_date_time(value: DateTime) -> Option<String> {
    value
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()
}

/// The instant an RFC 3339 `date-time` names, parsed by that same date type: month
/// lengths, leap years, the hour/minute/second ranges and the representable range of
/// dates are its rules, not restated here.
pub fn parse_date_time(text: &str) -> Option<DateTime> {
    DateTime::from_str(text, ::aws_smithy_types::date_time::Format::DateTime).ok()
}

/// The last instant this service can express as a `date-time`: RFC 3339 carries a
/// four-digit year, so there is no January of year 10000 to roll into. Reached only by a
/// clock already in December 9999, and still not earlier than that clock.
const LAST_REPRESENTABLE_DATE_TIME: &str = "9999-12-31T23:59:59Z";

/// The instant the billing period containing `now` ends: the first instant of the
/// next calendar month, UTC.
///
/// The billing period is a calendar month because the model says so —
/// `UpdateSubscription`: "Downgrades are scheduled and the current tier remains
/// unchanged until the end of the billing cycle (calendar month)" — so this boundary is
/// derived from the documented cycle, not chosen. Only the month count is arithmetic
/// here: which days a month has, leap years and the representable range all come from
/// the `aws-smithy-types` date type, through [`format_date_time`]/[`parse_date_time`].
/// The result is never earlier than `now`, so it can never read as already due.
pub fn end_of_billing_period(now: DateTime) -> DateTime {
    let saturated = || parse_date_time(LAST_REPRESENTABLE_DATE_TIME).unwrap_or(now);
    let Some(rendered) = format_date_time(now) else {
        return saturated();
    };
    // `YYYY-MM-DD...`: the two leading fields are all this needs.
    let mut fields = rendered.split('-');
    let (Some(Ok(year)), Some(Ok(month))) = (
        fields.next().map(str::parse::<i64>),
        fields.next().map(str::parse::<i64>),
    ) else {
        return saturated();
    };
    let (year, month) = if month >= 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    // A year RFC 3339 cannot carry fails to parse, which is the range check.
    parse_date_time(&format!("{year:04}-{month:02}-01T00:00:00Z")).unwrap_or_else(saturated)
}

/// Mint the ARN of a new subscription:
/// `arn:aws:pricingplanmanager:<region>:<account>:subscription/<id>`.
///
/// `sequence` is this store's monotonic counter, which is what makes the id unique; the
/// digest over it and the minting instant is what makes the id opaque, so it discloses
/// neither the store's size nor anything from the request. The result is grouped
/// 8-4-4-4-12, the shape AWS uses for this resource's id.
pub fn mint_subscription_arn(region: &str, sequence: u64, now: DateTime) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(sequence.to_be_bytes());
    hasher.update(now.secs().to_be_bytes());
    hasher.update(now.subsec_nanos().to_be_bytes());
    let digest = hex::encode(&hasher.finalize()[..16]);
    let id = format!(
        "{}-{}-{}-{}-{}",
        &digest[0..8],
        &digest[8..12],
        &digest[12..16],
        &digest[16..20],
        &digest[20..32]
    );
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("subscription/{id}"))
}

/// The region the request was signed for, or [`DEFAULT_REGION`] when it carries no
/// SigV4 credential scope.
pub fn request_region(ctx: &::smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Encode a `ListSubscriptions` page's resume point — the ARN of the last summary
/// the page returned — as an opaque `nextToken`.
pub fn encode_page_token(arn: &str) -> String {
    hex::encode(arn.as_bytes())
}

/// Decode a `nextToken` produced by [`encode_page_token`]; `None` for a token this
/// service could not have minted.
///
/// A token only ever carries a resume ARN of this service's own shape — it is not
/// encrypted, just hex of that ARN — so a decoded value that is not one cannot have come
/// from here and is refused rather than used as a resume point. The length guard keeps
/// the decode allocation bounded by the model's own `arn` limit (`@length(max: 2048)`,
/// two hex characters per byte) instead of by what a caller sends. A token whose
/// subscription has since been deleted is still valid: the token is a cursor into the
/// ordered store, not a handle on a record.
pub fn decode_page_token(token: &str) -> Option<String> {
    if token.len() > 2 * MAX_SUBSCRIPTION_ARN_LENGTH {
        return None;
    }
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    is_minted_subscription_arn(&decoded).then_some(decoded)
}

/// The modelled maximum length of a `SubscriptionArn` (`@length(min: 1, max: 2048)`).
const MAX_SUBSCRIPTION_ARN_LENGTH: usize = 2048;

/// Whether `value` has the shape [`mint_subscription_arn`] produces:
/// `arn:aws:<arn namespace>:<region>:<account>:subscription/<id>`.
pub fn is_minted_subscription_arn(value: &str) -> bool {
    value
        .strip_prefix("arn:aws:")
        .and_then(|rest| rest.strip_prefix(ARN_NAMESPACE))
        .is_some_and(|rest| rest.starts_with(':') && rest.contains(":subscription/"))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsPricingplanmanagerAwspricingplanmanagerState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsPricingplanmanagerAwspricingplanmanagerState>)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The end of a billing period is always midnight UTC on the 1st of the month that
    /// follows the instant's own, including across a December boundary and out of a leap
    /// day. The month lengths and leap rules come from the `aws-smithy-types` date type,
    /// so what is checked here is the roll.
    #[test]
    fn end_of_billing_period_is_the_first_of_the_next_month() {
        let cases = [
            ("2026-01-15T12:34:56Z", "2026-02-01T00:00:00Z"),
            ("2026-01-31T23:59:59Z", "2026-02-01T00:00:00Z"),
            ("2026-02-28T00:00:00Z", "2026-03-01T00:00:00Z"),
            ("2024-02-29T23:59:59Z", "2024-03-01T00:00:00Z"),
            ("2026-11-30T23:59:59Z", "2026-12-01T00:00:00Z"),
            ("2026-12-01T00:00:00Z", "2027-01-01T00:00:00Z"),
            ("2026-12-31T23:59:59Z", "2027-01-01T00:00:00Z"),
            ("1970-01-01T00:00:00Z", "1970-02-01T00:00:00Z"),
        ];
        for (start_text, expected) in cases {
            let start = parse_date_time(start_text).expect("the case is a date-time");
            let boundary = end_of_billing_period(start);
            assert_eq!(
                format_date_time(boundary).as_deref(),
                Some(expected),
                "case {start_text}: got {:?}",
                format_date_time(boundary)
            );
            assert!(
                boundary.secs() > start.secs(),
                "case {start_text}: the boundary must be in the future"
            );
        }
    }

    /// Even at the last date this service can express, the boundary is never earlier than
    /// the clock — so it can never read as already due.
    #[test]
    fn end_of_billing_period_never_precedes_the_clock() {
        for text in [
            "1970-01-01T00:00:00Z",
            "2026-06-15T08:00:00Z",
            "9999-11-30T00:00:00Z",
            "9999-12-31T23:59:59Z",
        ] {
            let start = parse_date_time(text).expect("the case is a date-time");
            let boundary = end_of_billing_period(start);
            assert!(
                boundary.secs() >= start.secs(),
                "case {text}: boundary {} precedes the clock {}",
                boundary.secs(),
                start.secs()
            );
        }
    }

    /// The date-time pair round-trips, so the tests that read a timestamp through it are
    /// reading the same calendar the wire serializer writes.
    #[test]
    fn date_time_round_trips() {
        for text in [
            "1970-01-01T00:00:00Z",
            "2000-02-29T12:00:00Z",
            "2026-12-31T23:59:59Z",
        ] {
            let parsed = parse_date_time(text).expect("the case is a date-time");
            assert_eq!(
                format_date_time(parsed).as_deref(),
                Some(text),
                "case {text}: did not round trip"
            );
        }
        for forged in ["", "not-a-date", "2026-02-30T00:00:00Z", "2026-13-01T00:00:00Z"] {
            assert!(
                parse_date_time(forged).is_none(),
                "case {forged:?}: must not parse as a date-time"
            );
        }
    }

    /// A minted ARN names this service, the caller's region and the canonical account,
    /// has the modelled 8-4-4-4-12 id shape, is accepted by the page-token decoder, and
    /// is distinct for every sequence number even at one instant.
    #[test]
    fn minted_arns_are_well_formed_and_unique() {
        let now = DateTime::from_secs(1_800_000_000);
        let mut seen = std::collections::BTreeSet::new();
        for sequence in 0..1_000_u64 {
            let arn = mint_subscription_arn("eu-west-1", sequence, now);
            assert!(
                arn.starts_with("arn:aws:pricingplanmanager:eu-west-1:"),
                "case {sequence}: got {arn}"
            );
            assert!(
                is_minted_subscription_arn(&arn),
                "case {sequence}: the token decoder must accept a minted ARN, got {arn}"
            );
            let id = arn
                .rsplit(":subscription/")
                .next()
                .expect("a minted ARN carries a subscription id");
            let groups: Vec<usize> = id.split('-').map(str::len).collect();
            assert_eq!(
                groups,
                vec![8, 4, 4, 4, 12],
                "case {sequence}: unexpected id shape in {id}"
            );
            assert!(
                seen.insert(arn.clone()),
                "case {sequence}: minted a duplicate ARN {arn}"
            );
        }
    }

    /// A page token round-trips, and anything this service did not mint is refused.
    #[test]
    fn page_tokens_round_trip_and_reject_forgeries() {
        let arn = mint_subscription_arn("us-east-1", 7, DateTime::from_secs(1_800_000_000));
        assert_eq!(
            decode_page_token(&encode_page_token(&arn)).as_deref(),
            Some(arn.as_str()),
            "a token must decode back to the ARN it was minted from"
        );
        for forged in [
            "",
            "not-hex",
            &hex::encode("zzz"),
            &hex::encode("arn:aws:s3:::bucket/key"),
            &hex::encode("arn:aws:pricingplanmanager:us-east-1:000000000000:other/1"),
        ] {
            assert_eq!(
                decode_page_token(forged),
                None,
                "case {forged:?}: a token this service did not mint must be refused"
            );
        }
    }
}
