// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-connectscheduling-amazonconnectscheduling service.
//!
//! What this service holds is a time off request LIFECYCLE and the balances the rules for it
//! are evaluated against, both scoped to one Connect instance and one user:
//!
//! * [`Instance::time_offs`] — every time off request the instance holds, keyed by the
//!   `TimeOffId` the create minted. A request is never removed: cancelling it moves it to
//!   [`TimeOffStatus::Cancelled`], which is a status `GetTimeOff` reports.
//! * [`Instance::balances`] — the time off balance of one (user, shift activity) pair, stored
//!   exactly as `PutTimeOffBalance` sent it so that call REPLACES a balance rather than
//!   adjusting it.
//! * [`Instance::client_tokens`] — the `TimeOffId` a `ClientToken` already minted, so a retry
//!   of a create answers the first create's id and writes nothing.
//!
//! The DECISION about a request lives here too ([`Instance::decide`]), because two operations
//! take it: `CreateTimeOff` for an `AUTOMATIC` request, and `ApproveTimeOff` for one that
//! waited. Keeping it in one place is what makes the two agree.
//!
//! This service models no operation that creates an instance, a user or a shift activity, so
//! none of those is a resource it can report missing: an instance's state comes into being
//! with the first write addressed to it.

use std::collections::{HashMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_state;

use crate::ops::types::{
    ConflictException, ResourceNotFoundException, ServiceQuotaExceededException,
    ValidationException, ValidationExceptionField,
};

/// How many time off requests one instance's store holds before a create is refused.
///
/// The emulator's own store bound rather than a published Connect quota: the store is in
/// memory, so it is bounded, and the modelled `ServiceQuotaExceededException` is how the
/// operation says a create would exceed a bound ([`quota_exceeded`]). It also bounds the
/// client tokens an instance remembers, since a token is recorded only with the record its
/// create stored, and a record is never removed.
pub const MAX_TIME_OFF_REQUESTS_PER_INSTANCE: usize = 500;

/// How many time off balances one instance's store holds.
///
/// The same bound for the other map a caller can grow: the (user, shift activity) pair a
/// balance is filed under is read from the request, so the count has to be capped rather
/// than left to the pairs a caller chooses. Past it the instance drops the balance written
/// longest ago ([`Instance::put_balance`]), which leaves that pair in the state it was in
/// before any balance was set for it — no balance, and so no balance rule to evaluate.
pub const MAX_TIME_OFF_BALANCES_PER_INSTANCE: usize = 500;

/// How many Connect instances this store files state under at once.
///
/// THIS is what bounds the whole store: the instance a record is filed under is read from
/// the request, so a count per instance would be a count per value a caller chose. Every
/// map held under an instance is itself capped, so the store holds at most this many
/// instances' worth of requests and balances. Past the bound the instance written longest
/// ago is dropped with everything under it, the emulator's own retention rather than a rule
/// of the service; it is set far above the number of Connect instances any use of the
/// emulator addresses.
pub const MAX_INSTANCES_HELD: usize = 64;

/// The resource kind a refusal about a time off request names in `ResourceType`.
const TIME_OFF_RESOURCE_TYPE: &str = "TimeOff";

/// The resource kind a refusal about the instance's store names in `ResourceType`.
const INSTANCE_RESOURCE_TYPE: &str = "Instance";

/// Per-service state for com-amazonaws-connectscheduling-amazonconnectscheduling. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsConnectschedulingAmazonconnectschedulingState {
    /// The time off state of every Connect instance addressed so far, keyed by the instance
    /// id ([`resource_id`]) so the same instance reached by id and by ARN is one entry.
    pub instances: HashMap<String, Instance>,
    /// The order [`Self::instances`] was written in, so the bound on it drops the instance
    /// written longest ago. Maintained by [`Self::instance_mut`], never directly.
    instance_order: VecDeque<String>,
}

impl ComAmazonawsConnectschedulingAmazonconnectschedulingState {
    /// The state of `instance_id`, created empty on the first write addressed to it.
    ///
    /// Creating one past [`MAX_INSTANCES_HELD`] drops the instance written longest ago, so a
    /// caller addressing instance after instance cannot grow the store without bound.
    pub fn instance_mut(&mut self, instance_id: &str) -> &mut Instance {
        let key = resource_id(instance_id).to_string();
        if !self.instances.contains_key(&key) {
            while self.instances.len() >= MAX_INSTANCES_HELD {
                match self.instance_order.pop_front() {
                    Some(oldest) => {
                        self.instances.remove(&oldest);
                    }
                    // The order queue and the map cannot disagree, but a queue that ran dry
                    // with the map still full would leave this loop spinning: empty the map
                    // instead, which is the bound this branch exists to keep.
                    None => {
                        self.instances.clear();
                        break;
                    }
                }
            }
            self.instance_order.push_back(key.clone());
        }
        self.instances.entry(key).or_default()
    }

    /// The state of `instance_id`, or `None` when nothing was ever written for it.
    pub fn instance(&self, instance_id: &str) -> Option<&Instance> {
        self.instances.get(resource_id(instance_id))
    }
}

/// One Connect instance's time off state. Every lookup is keyed by (instance, user), so one
/// user's requests and balances never answer for another's.
#[derive(Debug, Default)]
pub struct Instance {
    /// Every time off request of this instance, keyed by `TimeOffId`.
    pub time_offs: HashMap<String, TimeOffRequest>,
    /// The stored balance of one (user id, shift activity id) pair. Written through
    /// [`Self::put_balance`], which keeps the bound on it.
    pub balances: HashMap<(String, String), Balance>,
    /// The order [`Self::balances`] was written in, so the bound on it drops the balance
    /// written longest ago.
    balance_order: VecDeque<(String, String)>,
    /// The create each `ClientToken` already performed.
    pub client_tokens: HashMap<String, ClientTokenRecord>,
}

impl Instance {
    /// Store `balance` for one (user, shift activity) pair, REPLACING whatever was there:
    /// "the value replaces the existing balance rather than adjusting it".
    ///
    /// A pair the instance holds no balance for yet is taken in while there is room under
    /// [`MAX_TIME_OFF_BALANCES_PER_INSTANCE`]; past it the balance written longest ago is
    /// dropped.
    pub fn put_balance(&mut self, user_id: &str, shift_activity_id: &str, balance: Balance) {
        let key = (
            resource_id(user_id).to_string(),
            resource_id(shift_activity_id).to_string(),
        );
        if !self.balances.contains_key(&key) {
            while self.balances.len() >= MAX_TIME_OFF_BALANCES_PER_INSTANCE {
                match self.balance_order.pop_front() {
                    Some(oldest) => {
                        self.balances.remove(&oldest);
                    }
                    None => {
                        self.balances.clear();
                        break;
                    }
                }
            }
            self.balance_order.push_back(key.clone());
        }
        self.balances.insert(key, balance);
    }

    /// The request `time_off_id` names, but only when it belongs to `user_id`: "if the time
    /// off request doesn't belong to UserId, the operation returns ResourceNotFoundException",
    /// so a request of another user is not found rather than answered.
    pub fn time_off(&self, user_id: &str, time_off_id: &str) -> Option<&TimeOffRequest> {
        self.time_offs
            .get(time_off_id)
            .filter(|record| record.user_id == resource_id(user_id))
    }

    /// The stored balance of one (user, shift activity) pair, in minutes.
    pub fn balance_minutes(&self, user_id: &str, shift_activity_id: &str) -> Option<i64> {
        self.balances
            .get(&(
                resource_id(user_id).to_string(),
                resource_id(shift_activity_id).to_string(),
            ))
            .map(Balance::minutes)
    }

    /// Whether `candidate`'s window overlaps another request of the same user that is
    /// `APPROVED`.
    ///
    /// The window is half-open — `StartAt` is inclusive and `EndAt` exclusive — so two
    /// requests that merely touch do not overlap. Only an `APPROVED` request holds a window:
    /// a waiting request "doesn't reserve its window", and a rejected or cancelled one has
    /// released it.
    fn overlaps_approved(&self, candidate: &TimeOffRequest) -> bool {
        self.time_offs.values().any(|other| {
            other.time_off_id != candidate.time_off_id
                && other.user_id == candidate.user_id
                && other.status == TimeOffStatus::Approved
                && other.start_at_millis < candidate.end_at_millis
                && candidate.start_at_millis < other.end_at_millis
        })
    }

    /// The minutes of this user's `APPROVED` time off that already draw on the balance for
    /// `shift_activity_id`, ignoring the request being decided.
    fn committed_minutes(&self, candidate: &TimeOffRequest) -> i64 {
        self.time_offs
            .values()
            .filter(|other| {
                other.time_off_id != candidate.time_off_id
                    && other.user_id == candidate.user_id
                    && other.shift_activity_id == candidate.shift_activity_id
                    && other.status == TimeOffStatus::Approved
            })
            .map(TimeOffRequest::requested_minutes)
            .fold(0i64, i64::saturating_add)
    }

    /// Evaluate the configured rules for `candidate` and say which status it ends in.
    ///
    /// The ONE decision both `CreateTimeOff` (for an `AUTOMATIC` request) and
    /// `ApproveTimeOff` take, in the order the service documents it:
    ///
    /// 1. an overlap with another `APPROVED` request of the same user ends the request
    ///    `REJECTED` with `CONFLICTING_TIME_OFF_REQUEST`, and `OVERRIDE` never waives it;
    /// 2. a stored balance the request does not fit in is a failed rule:
    ///    `INSUFFICIENT_USER_TIME_OFF_BALANCE`, recorded as an `ERROR` that rejects the
    ///    request under `REJECT` and as a `WARNING` that approves it under `OVERRIDE`;
    /// 3. a request whose whole window is in the past records a failed rule as a `WARNING`
    ///    and is not rejected, whatever the `RuleFailureAction`;
    /// 4. anything else ends `APPROVED` with no validation results.
    ///
    /// A user with no stored balance for the activity has no balance rule to evaluate: this
    /// service holds balances only where `PutTimeOffBalance` set them.
    pub fn decide(
        &self,
        candidate: &TimeOffRequest,
        rule_failure_action: RuleFailureAction,
        now_millis: i64,
    ) -> Decision {
        if self.overlaps_approved(candidate) {
            return Decision {
                status: TimeOffStatus::Rejected,
                validation_results: vec![ValidationOutcome {
                    severity: ValidationSeverity::Error,
                    code: ValidationCode::ConflictingTimeOffRequest,
                }],
            };
        }

        if let Some(balance) =
            self.balance_minutes(&candidate.user_id, &candidate.shift_activity_id)
        {
            let requested = candidate
                .requested_minutes()
                .saturating_add(self.committed_minutes(candidate));
            if requested > balance {
                let whole_window_in_the_past = candidate.end_at_millis <= now_millis;
                let waived = whole_window_in_the_past
                    || rule_failure_action == RuleFailureAction::Override;
                return Decision {
                    status: match waived {
                        true => TimeOffStatus::Approved,
                        false => TimeOffStatus::Rejected,
                    },
                    validation_results: vec![ValidationOutcome {
                        severity: match waived {
                            true => ValidationSeverity::Warning,
                            false => ValidationSeverity::Error,
                        },
                        code: ValidationCode::InsufficientUserTimeOffBalance,
                    }],
                };
            }
        }

        Decision {
            status: TimeOffStatus::Approved,
            validation_results: Vec::new(),
        }
    }
}

/// What a rule evaluation decided: the status the request ends in, and the warnings or
/// failures produced while processing it.
#[derive(Debug, Clone)]
pub struct Decision {
    pub status: TimeOffStatus,
    pub validation_results: Vec<ValidationOutcome>,
}

/// A create a `ClientToken` already performed: the id it minted, and a fingerprint of the
/// request body it was sent with, so reusing the token with a DIFFERENT body is refused
/// instead of answering the first create's id.
#[derive(Debug, Clone)]
pub struct ClientTokenRecord {
    pub time_off_id: String,
    pub fingerprint: String,
}

/// One time off request, as `CreateTimeOff` stored it and the lifecycle calls changed it.
#[derive(Debug, Clone)]
pub struct TimeOffRequest {
    pub time_off_id: String,
    /// The user the request is for, as an id ([`resource_id`]): `GetTimeOff` answers `UserId`,
    /// the id form, whichever form the create addressed.
    pub user_id: String,
    /// The shift activity of the request, as an id, for the same reason as `user_id`.
    pub shift_activity_id: String,
    /// The start of the window, inclusive, "as sent to CreateTimeOff", to the millisecond.
    pub start_at_millis: i64,
    /// The end of the window, exclusive, as sent.
    pub end_at_millis: i64,
    pub status: TimeOffStatus,
    pub approval_type: ApprovalType,
    /// What the create asked for on a failed rule, which is what decides an `AUTOMATIC`
    /// request. `ApproveTimeOff` carries its own, so a waiting request is decided by the
    /// approval's value.
    pub rule_failure_action: RuleFailureAction,
    /// The description of the request: the create sets it, and a cancellation replaces or
    /// removes it. A description sent with an approval or a rejection is not returned by
    /// `GetTimeOff`, so it is not recorded here.
    pub description: Option<String>,
    pub validation_results: Vec<ValidationOutcome>,
    /// The identity that created the request, immutable for its life.
    pub created_by: String,
    pub created_at_millis: i64,
    /// The identity that made the most recent approve, reject or cancel call. Absent until
    /// one of those is taken — including after the service itself decides an `AUTOMATIC`
    /// request, which changes `UpdatedAt` without naming an identity.
    pub updated_by: Option<String>,
    pub updated_at_millis: i64,
}

impl TimeOffRequest {
    /// The length of the requested window in minutes, rounded up, so any non-empty window
    /// draws at least a minute of the balance it is checked against.
    pub fn requested_minutes(&self) -> i64 {
        let millis = self.end_at_millis.saturating_sub(self.start_at_millis).max(0);
        millis.saturating_add(59_999) / 60_000
    }

    /// Record a rule evaluation's outcome on the request, and that it changed at
    /// `now_millis`.
    ///
    /// `actor` is the identity of the call that asked for the change, and `None` where the
    /// service decided the request itself.
    pub fn settle(&mut self, decision: Decision, now_millis: i64, actor: Option<String>) {
        self.status = decision.status;
        self.validation_results = decision.validation_results;
        self.updated_at_millis = now_millis;
        if actor.is_some() {
            self.updated_by = actor;
        }
    }
}

/// The statuses a time off request can be in (`TimeOffStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeOffStatus {
    Pending,
    Approved,
    Rejected,
    Cancelled,
    AwaitingApproval,
}

impl TimeOffStatus {
    /// The value `GetTimeOff` reports for this status.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Approved => "APPROVED",
            Self::Rejected => "REJECTED",
            Self::Cancelled => "CANCELLED",
            Self::AwaitingApproval => "AWAITING_APPROVAL",
        }
    }
}

/// Who approves a request (`ApprovalType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalType {
    /// The service evaluates the rules and decides the request.
    Automatic,
    /// The request waits in `AWAITING_APPROVAL` for an approve or reject call.
    AwaitApproval,
}

impl ApprovalType {
    /// Read the modelled wire value. The boundary refuses any other value before a handler
    /// runs, so `None` is a value the model does not declare.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "AUTOMATIC" => Some(Self::Automatic),
            "AWAIT_APPROVAL" => Some(Self::AwaitApproval),
            _ => None,
        }
    }
}

/// What happens when a configured rule fails (`RuleFailureAction`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleFailureAction {
    /// Record an `ERROR` and reject the request. The documented default.
    Reject,
    /// Record a `WARNING` and approve the request past the failed rule.
    Override,
}

impl RuleFailureAction {
    /// Read the modelled wire value, defaulting an absent one to `REJECT`, "the default".
    pub fn from_wire(value: Option<&str>) -> Self {
        match value {
            Some("OVERRIDE") => Self::Override,
            _ => Self::Reject,
        }
    }
}

/// One warning or failure produced while processing a request
/// (`TimeOffValidationResult`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValidationOutcome {
    pub severity: ValidationSeverity,
    pub code: ValidationCode,
}

/// How much a breach of the configured rules weighs (`TimeOffValidationSeverity`): a
/// `WARNING` is not enough to reject the request, an `ERROR` is a reason it was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationSeverity {
    Warning,
    Error,
}

impl ValidationSeverity {
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Warning => "WARNING",
            Self::Error => "ERROR",
        }
    }
}

/// The rules this slice evaluates, from `TimeOffValidationCode`. The group and staffing-group
/// allowances the same enum carries are not evaluated here: this service holds no group
/// allowance records to evaluate them against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationCode {
    InsufficientUserTimeOffBalance,
    ConflictingTimeOffRequest,
}

impl ValidationCode {
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::InsufficientUserTimeOffBalance => "INSUFFICIENT_USER_TIME_OFF_BALANCE",
            Self::ConflictingTimeOffRequest => "CONFLICTING_TIME_OFF_REQUEST",
        }
    }
}

/// A time off balance, stored exactly as `PutTimeOffBalance` sent it (`Duration`): that call
/// replaces the balance rather than adjusting it, so the value and the unit it was sent in
/// are what a later read of the balance has to see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Balance {
    pub value: i32,
    pub unit: DurationUnit,
}

impl Balance {
    /// The balance in minutes, which is the unit a request's window is measured against.
    pub fn minutes(&self) -> i64 {
        i64::from(self.value).saturating_mul(self.unit.minutes())
    }
}

/// The units a `Duration` can be sent in (`DurationUnit`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurationUnit {
    Minutes,
    Hours,
    Days,
}

impl DurationUnit {
    /// Read the modelled wire value. The boundary refuses any other value before a handler
    /// runs, so `None` is a value the model does not declare.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "MINUTES" => Some(Self::Minutes),
            "HOURS" => Some(Self::Hours),
            "DAYS" => Some(Self::Days),
            _ => None,
        }
    }

    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Minutes => "MINUTES",
            Self::Hours => "HOURS",
            Self::Days => "DAYS",
        }
    }

    /// How many minutes one unit is: an hour is 60 minutes and a day is 24 of those hours.
    pub fn minutes(self) -> i64 {
        match self {
            Self::Minutes => 1,
            Self::Hours => 60,
            Self::Days => 24 * 60,
        }
    }
}

/// The canonical form of an identifier member that admits either an id or an ARN
/// (`InstanceIdOrArn`, `UserIdOrArn`, `ShiftActivityIdOrArn`): the id the ARN ends with, or
/// the value itself when it already is an id.
///
/// One form for the store's keys, so a caller that addresses an instance, a user or an
/// activity by ARN reaches the state another caller wrote by id — and `GetTimeOff`, whose
/// `UserId` and `ShiftActivityId` are the id shapes rather than the "or ARN" ones, answers
/// the id.
pub fn resource_id(value: &str) -> &str {
    value.split('/').next_back().unwrap_or(value)
}

/// The current instant as epoch milliseconds, the resolution this service stores and reports
/// its timestamps at.
pub fn now_millis() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX)
}

/// The sentence this service's `ResourceNotFoundException` carries, as the model states it
/// for that error.
const RESOURCE_NOT_FOUND_MESSAGE: &str = "The specified resource was not found.";

/// The sentence this service's `ConflictException` carries, as the model states it.
const CONFLICT_MESSAGE: &str =
    "The request could not be processed because of a conflict with the current state of the \
     resource.";

/// The sentence this service's `ValidationException` carries, as the model states it. What
/// exactly was wrong is named per member in `FieldList`.
const VALIDATION_MESSAGE: &str =
    "The input fails to satisfy the constraints specified by an AWS service.";

/// The sentence this service's `ServiceQuotaExceededException` carries, as the model states
/// it.
const QUOTA_EXCEEDED_MESSAGE: &str = "The request would cause a service quota to be exceeded.";

/// The modelled 404 for a `TimeOffId` that names no request of the user addressed.
pub fn time_off_not_found(time_off_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: RESOURCE_NOT_FOUND_MESSAGE.to_string(),
        resource_id: Some(time_off_id.to_string()),
        resource_type: Some(TIME_OFF_RESOURCE_TYPE.to_string()),
    }
}

/// The modelled 409 for a request whose current status does not admit the call.
pub fn status_conflict(time_off_id: &str) -> ConflictException {
    ConflictException {
        message: CONFLICT_MESSAGE.to_string(),
        resource_id: Some(time_off_id.to_string()),
        resource_type: Some(TIME_OFF_RESOURCE_TYPE.to_string()),
    }
}

/// The modelled 400 for a member whose value breaks a rule the model states in prose rather
/// than in a constraint the boundary can refuse.
///
/// `member` is the member's name as the model spells it, and `reason` the rule it broke; the
/// pair is what a caller fixes the request from.
pub fn invalid_member(member: &str, reason: &str) -> ValidationException {
    ValidationException {
        message: VALIDATION_MESSAGE.to_string(),
        reason: Some("FIELD_VALIDATION_FAILED".to_string()),
        field_list: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message: reason.to_string(),
        }]),
    }
}

/// The modelled 402 for a create that would take an instance past
/// [`MAX_TIME_OFF_REQUESTS_PER_INSTANCE`].
pub fn quota_exceeded(instance_id: &str) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: QUOTA_EXCEEDED_MESSAGE.to_string(),
        quota_code: None,
        service_code: None,
        resource_id: Some(resource_id(instance_id).to_string()),
        resource_type: Some(INSTANCE_RESOURCE_TYPE.to_string()),
    }
}

/// The message a modelled server error carries when this service's store is not reachable on
/// the engine the operation was constructed from.
pub const STORE_UNAVAILABLE_MESSAGE: &str =
    "the Connect Customer scheduling store is unavailable";

/// What a call of this service did, as the one closed set [`audit`] records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditOutcome {
    /// A time off balance was stored, replacing whatever was there.
    BalanceStored,
    /// A time off request was created and stored.
    TimeOffCreated,
    /// A create was the retry of one this instance already performed, so it stored nothing.
    TimeOffReplayed,
    /// A stored request ended `APPROVED`.
    TimeOffApproved,
    /// A stored request ended `REJECTED`.
    TimeOffRejected,
    /// A stored request ended `CANCELLED`.
    TimeOffCancelled,
    /// The call succeeded without changing the stored request.
    Unchanged,
    /// The call was refused with one of the operation's modelled errors.
    Refused,
}

impl AuditOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BalanceStored => "balance-stored",
            Self::TimeOffCreated => "time-off-created",
            Self::TimeOffReplayed => "time-off-replayed",
            Self::TimeOffApproved => "time-off-approved",
            Self::TimeOffRejected => "time-off-rejected",
            Self::TimeOffCancelled => "time-off-cancelled",
            Self::Unchanged => "unchanged",
            Self::Refused => "refused",
        }
    }
}

/// The outcome to record for a request that ended in `status`.
pub fn audit_outcome_of(status: TimeOffStatus) -> AuditOutcome {
    match status {
        TimeOffStatus::Approved => AuditOutcome::TimeOffApproved,
        TimeOffStatus::Rejected => AuditOutcome::TimeOffRejected,
        TimeOffStatus::Cancelled => AuditOutcome::TimeOffCancelled,
        TimeOffStatus::Pending | TimeOffStatus::AwaitingApproval => AuditOutcome::TimeOffCreated,
    }
}

/// Emit the structured record of one state-changing call of this service.
///
/// Every operation that stores, changes or refuses something calls this on the branch it
/// returns from, so a reader of the emulator's log can attribute each call — above all the
/// state-changing ones, whose effect on the store is otherwise invisible afterwards. The
/// record carries the operation, the identity the request PRESENTED (the SigV4 access key id,
/// which is an identifier and not a secret), the request target it named, the resource it
/// addressed and the outcome; the subscriber supplies the timestamp.
///
/// Metadata only: no description, no balance and no client token, so a caller's own data
/// never reaches the log. Every interpolated value is request-derived, so each is bounded and
/// stripped of the control characters that would otherwise let one request forge a second log
/// line ([`audit_field`]).
pub fn audit(
    operation: &str,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    resource: &str,
    outcome: AuditOutcome,
) {
    let parts = ctx.parts();
    ::tracing::info!(
        target: "emulator::audit",
        service = ctx.identity().signing_name(),
        operation,
        akid = %audit_field(&::engine::extract_akid(&parts.headers)),
        request_target = %audit_field(&format!("{} {}", parts.method, parts.uri.path())),
        resource = %audit_field(resource),
        outcome = outcome.as_str(),
        "connect scheduling api call"
    );
}

/// Longest request-derived value one audit field carries.
///
/// An access key id, a request path and a resource identifier are all caller-controlled, so
/// an unbounded echo would let one request emit an arbitrarily long log line.
const MAX_AUDIT_FIELD: usize = 128;

/// A request-derived value as it may appear in an audit record: control characters (above all
/// CR and LF, which would forge a second record) replaced, and the whole value bounded by
/// [`MAX_AUDIT_FIELD`] on a character boundary.
fn audit_field(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { '.' } else { c })
        .take(MAX_AUDIT_FIELD)
        .collect()
}

/// The resource an audit record names when the call addresses a (user, shift activity) pair
/// rather than one stored request — a balance, or a create before its id is minted — in the
/// id form the store keys it by.
pub fn audit_pair_resource(user_id: &str, shift_activity_id: &str) -> String {
    format!(
        "{}/{}",
        resource_id(user_id),
        resource_id(shift_activity_id)
    )
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsConnectschedulingAmazonconnectschedulingState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsConnectschedulingAmazonconnectschedulingState>)
}

/// WHO a request speaks for, as `sts:GetCallerIdentity` answers it for the same credential:
/// the identity `CreatedBy` and `UpdatedBy` name on a time off request.
///
/// Asked of the store that owns it — the session registry in the SAME shared engine — rather
/// than spelled out of the request, so a client that compares a request's `CreatedBy` with
/// its own identity can match. An engine carrying no session registry holds no session, so
/// the only identity a request can present there is the account's own published one.
///
/// Read with NO store of this service locked: resolving a caller materialises the session
/// expiries the clock has already made due, a write owed at the same instant whether or not
/// anybody called. A credential whose session that resolution finds expired verifies for
/// nobody, so the identity it would have spoken for is the account's own published one.
pub fn caller_identity_arn<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> String {
    let access_key_id = ::service_common::request_access_key_id(ctx.parts());
    let identity = match sts_state::service_state(engine) {
        Some(registry) => sts_state::resolve_caller(
            &mut registry.lock(),
            &access_key_id,
            SystemTime::now(),
            ctx.credential_store(),
        ),
        None => Ok(sts_state::ambient_identity()),
    };
    identity
        .unwrap_or_else(|_expired| sts_state::ambient_identity())
        .arn
}
