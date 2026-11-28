// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-abusesignal-abusereportapi service.
//!
//! One resource is persisted here: the ABUSE REPORT a `CreateAbuseReport` call submits.
//! `GetAbuseReportStatus` and `ListAbuseReportStatuses` both render what they answer out
//! of these records ([`AbuseReport::status_output`] and [`AbuseReport::list_item`]), so a
//! create, a read and a listing cannot disagree about a report id, a correlation id, a
//! status, a reason code or a timestamp.
//!
//! The reading of the XARF document a report carries — the digest two submissions are
//! told apart by, the XARF core fields the document must declare, and the submission
//! requirements the model publishes — lives in [`xarf`].

mod xarf;

pub use xarf::*;

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant, SystemTime};

use service_common::page_tokens::{scope_digest, PageTokenKey};

use crate::ops::types::{GetAbuseReportStatusOutput, ReportStatusListItem, REASON_CODE_VALUES};

/// The first status a submitted report carries, and the two it moves through:
/// `ReportSubmissionStatus` is `QUEUED | PROCESSING | COMPLETED`
/// (`smithy_query(query="shape",shape="ReportSubmissionStatus")`), and the model calls
/// `COMPLETED` "the only terminal status".
pub const STATUS_QUEUED: &str = "QUEUED";
/// See [`STATUS_QUEUED`].
pub const STATUS_PROCESSING: &str = "PROCESSING";
/// See [`STATUS_QUEUED`].
pub const STATUS_COMPLETED: &str = "COMPLETED";

/// The `description` a report in each status is reported with — "a human-readable
/// description of the current status", worded as the model itself words that status:
/// `QUEUED` is "the report has been received and queued for processing", `PROCESSING`
/// "the report has been received and is being processed", `COMPLETED` "downstream
/// resolution is finished".
pub const QUEUED_DESCRIPTION: &str = "The report has been received and queued for processing.";
/// See [`QUEUED_DESCRIPTION`].
pub const PROCESSING_DESCRIPTION: &str = "The report has been received and is being processed.";
/// See [`QUEUED_DESCRIPTION`].
pub const COMPLETED_DESCRIPTION: &str = "Downstream resolution is finished.";

/// What a handler answers when the shared store cannot be reached at all. The wording is
/// the model's own for `InternalServerException`: "An unexpected error occurred during the
/// processing of the request."
pub const STORE_UNAVAILABLE: &str =
    "An unexpected error occurred during the processing of the request.";

/// How this service words a request it refuses as not well-formed. The wording is the
/// model's own for `ValidationException`.
pub const VALIDATION_MESSAGE: &str =
    "The input fails to satisfy the constraints specified by the service, which indicates that \
     the request is not well-formed. Check fieldList for the specific members that failed \
     validation.";

/// How a report moves on: one span per step of the reporter-facing lifecycle, so a caller
/// that reads a report straight after submitting it still sees it `QUEUED`, one span later
/// `PROCESSING`, and one span after that `COMPLETED`.
///
/// Seconds rather than the days the model says a real resolution can take, because the
/// model also tells a poller what interval to expect progress on: "If you poll this
/// operation yourself, wait 1–5 seconds between calls." The instant each step falls due is
/// recorded on the record and compared against the emulator's clock
/// ([`service_common::clock`]); nothing sleeps.
pub const PROGRESSION_SPAN: Duration = Duration::from_secs(3);

/// How long a caller-supplied `clientToken` keeps answering with the report it created:
/// the model's "If you provide a `clientToken`, duplicate requests within 24 hours return
/// the existing report ID."
pub const CLIENT_TOKEN_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

/// How long the token the service itself derives for a request that carries no
/// `clientToken` keeps answering with the report it created: the model's "When you omit
/// this token, the service automatically generates one that is scoped to a 5-minute
/// window."
pub const GENERATED_TOKEN_WINDOW: Duration = Duration::from_secs(5 * 60);

/// `maxResults`' modelled ceiling for `ListAbuseReportStatuses` (range 1..50, and
/// `ReportStatusList` is `@length(0..50)`), which the model also gives as the page size of
/// a listing that names none: "Valid range: 1–50. The default is 50."
pub const MAX_RESULTS_CEILING: i32 = 50;

/// How many characters a minted `reportId` is written in. `ReportId` is
/// `@length(20..64)` with pattern `^[a-zA-Z0-9]{20,64}$`, and the alphabet
/// [`service_common::minted_base36`] draws from (digits and uppercase letters) is a subset
/// of what that pattern admits.
const REPORT_ID_CHARS: usize = 32;

/// What separates the two fields a pagination cursor carries (the sequence it names a
/// position in, and the report id it resumes after): a byte no `reportId` and no scope
/// digest can hold, so the split is unambiguous.
const CURSOR_SEPARATOR: char = '\u{1f}';

/// How many submitted reports this store holds.
///
/// A submission is a write a caller can repeat, and nothing AWS publishes caps how many
/// reports an account may submit, so refusing one past a limit would be this emulator's
/// invention. What is bounded instead is the STORE: the oldest submission is released when
/// a new one would take the store past this many ([`ComAmazonawsAbusesignalAbusereportapiState::insert`]),
/// so an emulator left running cannot grow without bound, and the walk a listing and a
/// clock pass over the store stays bounded by a constant.
pub const MAX_STORED_REPORTS: usize = 1_000;

/// Per-service state for com-amazonaws-abusesignal-abusereportapi. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAbusesignalAbusereportapiState {
    /// Every submitted report, keyed by its `reportId` — the store
    /// `GetAbuseReportStatus` reads and `ListAbuseReportStatuses` renders.
    pub reports: BTreeMap<String, AbuseReport>,
    /// The report ids in SUBMISSION order, which is the order the model says a listing
    /// reports them in ("ordered by submission time") and the order a `nextToken` resumes
    /// a page of.
    pub order: Vec<String>,
    /// The live idempotency records, keyed by [`idempotency_key`]'s rendering of the
    /// token a create was answered under.
    pub idempotency: HashMap<String, IdempotencyRecord>,
    /// The monotonic counter a minted `reportId` derives from, so two creates in one
    /// process cannot mint the same id.
    pub next_report_seq: u64,
    /// The secret THIS store's pagination tokens are authenticated under, so a cursor
    /// this service never issued names no page.
    pub page_token_key: PageTokenKey,
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAbusesignalAbusereportapiState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAbusesignalAbusereportapiState>)
}

/// A stored instant as the wire timestamp a `Timestamp` member of this model carries.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    // Seconds and sub-second nanos separately, so the instant the store holds survives
    // the conversion exactly rather than through a float.
    ::aws_smithy_types::DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        since_epoch.subsec_nanos(),
    )
}

/// One submitted abuse report, as stored.
#[derive(Debug, Clone)]
pub struct AbuseReport {
    /// The id this service minted for the report, which every answer about it carries.
    pub report_id: String,
    /// The `correlationId` the submission carried, if any — echoed back by the create,
    /// reported by the read and the listing, and the value the listing filters on.
    pub correlation_id: Option<String>,
    /// The XARF document the submission carried. Held so a repeat of an idempotency
    /// token can be told from a token reused with a different payload, and so a report's
    /// own content is what later answers are derived from.
    pub report: ::aws_smithy_types::Document,
    /// Where the report stands in the reporter-facing lifecycle: one of
    /// [`STATUS_QUEUED`], [`STATUS_PROCESSING`] and [`STATUS_COMPLETED`].
    pub status: String,
    /// The outcome a `COMPLETED` report carries, and `None` in every other status — the
    /// model: "This value is present only after the report reaches the terminal
    /// `COMPLETED` status; it is absent while the report is `QUEUED` or `PROCESSING`."
    pub reason_code: Option<String>,
    /// When the report was submitted, which `submittedAt` reports.
    pub submitted_at: SystemTime,
    /// The same instant on the emulator's own clock, which the lifecycle steps are
    /// measured from. Kept beside `submitted_at` because a due instant has to be compared
    /// on a monotonic clock while a reported timestamp has to be a wall-clock time.
    pub submitted_instant: Instant,
    /// When the report's status last changed, which `lastUpdatedAt` reports. The status
    /// was set when the report was submitted, so it starts equal to `submitted_at`.
    pub last_updated_at: SystemTime,
}

impl AbuseReport {
    /// A report submitted now: `QUEUED`, with no outcome yet.
    pub fn new(
        report_id: String,
        correlation_id: Option<String>,
        report: ::aws_smithy_types::Document,
        submitted_at: SystemTime,
        submitted_instant: Instant,
    ) -> Self {
        Self {
            report_id,
            correlation_id,
            report,
            status: STATUS_QUEUED.to_string(),
            reason_code: None,
            submitted_at,
            submitted_instant,
            last_updated_at: submitted_at,
        }
    }

    /// Materialise every lifecycle step the clock already owes this report by `now`.
    ///
    /// Both steps are checked in order, so a clock that crossed both spans since the last
    /// call lands on `COMPLETED` rather than one step short of it. A report that is
    /// already `COMPLETED` is terminal and nothing here moves it.
    pub fn advance(&mut self, now: Instant) {
        if self.status == STATUS_QUEUED && self.step_is_due(now, 1) {
            self.enter(STATUS_PROCESSING, PROGRESSION_SPAN);
        }
        if self.status == STATUS_PROCESSING && self.step_is_due(now, 2) {
            self.enter(STATUS_COMPLETED, PROGRESSION_SPAN * 2);
            self.reason_code = Some(resolution_reason_code(&self.report_id).to_string());
        }
    }

    /// Whether `step` spans have passed since the report was submitted.
    fn step_is_due(&self, now: Instant, step: u32) -> bool {
        self.submitted_instant
            .checked_add(PROGRESSION_SPAN * step)
            .is_some_and(|due| now >= due)
    }

    /// Record `status`, reached `since_submission` after the report was submitted.
    ///
    /// `lastUpdatedAt` is the instant the step FELL DUE rather than the instant some
    /// later call happened to materialise it, so what a read reports does not depend on
    /// when it was read.
    fn enter(&mut self, status: &str, since_submission: Duration) {
        self.status = status.to_string();
        self.last_updated_at = self
            .submitted_at
            .checked_add(since_submission)
            .unwrap_or(self.submitted_at);
    }

    /// The `description` of the status this report stands in.
    pub fn description(&self) -> &'static str {
        match self.status.as_str() {
            STATUS_PROCESSING => PROCESSING_DESCRIPTION,
            STATUS_COMPLETED => COMPLETED_DESCRIPTION,
            _ => QUEUED_DESCRIPTION,
        }
    }

    /// This record as `GetAbuseReportStatus`'s answer.
    pub fn status_output(&self) -> GetAbuseReportStatusOutput {
        GetAbuseReportStatusOutput {
            report_id: self.report_id.clone(),
            correlation_id: self.correlation_id.clone(),
            status: self.status.clone(),
            description: self.description().to_string(),
            reason_code: self.reason_code.clone(),
            submitted_at: timestamp(self.submitted_at),
            last_updated_at: Some(timestamp(self.last_updated_at)),
        }
    }

    /// This record as one entry of `ListAbuseReportStatuses`'s page.
    pub fn list_item(&self) -> ReportStatusListItem {
        ReportStatusListItem {
            report_id: self.report_id.clone(),
            correlation_id: self.correlation_id.clone(),
            status: self.status.clone(),
            description: self.description().to_string(),
            reason_code: self.reason_code.clone(),
        }
    }
}

/// What one idempotency token has already been answered with.
#[derive(Debug, Clone)]
pub struct IdempotencyRecord {
    /// The report the first call under this token created.
    pub report_id: String,
    /// The digest of that call's payload, so a token reused with a DIFFERENT payload is
    /// told from a repeat of the same one.
    pub payload_fingerprint: String,
    /// When this record stops answering: the instant the token's window closes.
    pub expires_at: Instant,
}

impl IdempotencyRecord {
    /// Whether this record still answers at `now`.
    fn is_live(&self, now: Instant) -> bool {
        now < self.expires_at
    }
}

/// What a create should do with the idempotency token it was called under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdempotencyOutcome {
    /// No live record holds this token: create a report.
    Fresh,
    /// A live record holds this token under the SAME payload: answer with the report it
    /// already created.
    Replay(String),
    /// A live record holds this token under a DIFFERENT payload, which the model calls a
    /// conflict.
    Conflict,
}

/// The store key an idempotency token is held under.
///
/// A caller's `clientToken` and a token the service derived from the payload live in
/// separate namespaces, so a caller cannot send a `clientToken` that collides with a
/// derived one.
pub fn idempotency_key(client_token: Option<&str>, payload_fingerprint: &str) -> String {
    match client_token {
        Some(token) => format!("clientToken{CURSOR_SEPARATOR}{token}"),
        None => format!("generated{CURSOR_SEPARATOR}{payload_fingerprint}"),
    }
}

/// How long a token of the kind `client_token` names answers for: see
/// [`CLIENT_TOKEN_WINDOW`] and [`GENERATED_TOKEN_WINDOW`].
pub fn idempotency_window(client_token: Option<&str>) -> Duration {
    if client_token.is_some() {
        CLIENT_TOKEN_WINDOW
    } else {
        GENERATED_TOKEN_WINDOW
    }
}

impl ComAmazonawsAbusesignalAbusereportapiState {
    /// Materialise every lifecycle step the clock owes any stored report by `now`.
    pub fn advance_all(&mut self, now: Instant) {
        for report in self.reports.values_mut() {
            report.advance(now);
        }
    }

    /// What a create called under `key` should do: see [`IdempotencyOutcome`].
    ///
    /// A record whose window has closed answers nothing, so the call goes on to create a
    /// report, which is what the documented window means.
    pub fn idempotency_outcome(
        &self,
        key: &str,
        payload_fingerprint: &str,
        now: Instant,
    ) -> IdempotencyOutcome {
        match self.idempotency.get(key).filter(|held| held.is_live(now)) {
            Some(held) if held.payload_fingerprint == payload_fingerprint => {
                IdempotencyOutcome::Replay(held.report_id.clone())
            }
            Some(_) => IdempotencyOutcome::Conflict,
            None => IdempotencyOutcome::Fresh,
        }
    }

    /// Hold `key` against the report it created until its window closes.
    ///
    /// Records whose windows have already closed are dropped as the new one is taken, so
    /// the index holds only tokens that still answer.
    pub fn record_idempotency(
        &mut self,
        key: String,
        payload_fingerprint: String,
        report_id: String,
        now: Instant,
        window: Duration,
    ) {
        self.idempotency.retain(|_, held| held.is_live(now));
        self.idempotency.insert(
            key,
            IdempotencyRecord {
                report_id,
                payload_fingerprint,
                expires_at: now.checked_add(window).unwrap_or(now),
            },
        );
    }

    /// Mint a `reportId` no stored report holds.
    ///
    /// Derived from the payload digest, the store's own monotonic counter and the
    /// submission instant, so two creates — of the same payload or of different ones —
    /// cannot collide, and the loop re-mints rather than overwriting if one ever did.
    pub fn mint_report_id(&mut self, payload_fingerprint: &str, at: SystemTime) -> String {
        let nanos = at
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        loop {
            self.next_report_seq = self.next_report_seq.wrapping_add(1);
            let seed = format!(
                "{payload_fingerprint}{CURSOR_SEPARATOR}{}{CURSOR_SEPARATOR}{nanos}",
                self.next_report_seq
            );
            let candidate = ::service_common::minted_base36(&seed, REPORT_ID_CHARS);
            if !self.reports.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// Store `report` as the newest submission, releasing the oldest ones while the store
    /// would otherwise hold more than [`MAX_STORED_REPORTS`].
    ///
    /// An idempotency record naming a released report goes with it: a token that answered
    /// with a report id must never answer with one no read resolves.
    pub fn insert(&mut self, report: AbuseReport) {
        self.order.push(report.report_id.clone());
        self.reports.insert(report.report_id.clone(), report);
        while self.order.len() > MAX_STORED_REPORTS {
            let released = self.order.remove(0);
            self.reports.remove(&released);
            self.idempotency
                .retain(|_, held| held.report_id != released);
        }
    }

    /// One page of the stored reports, in submission order, filtered to
    /// `correlation_id` when the caller named one and resuming after the report a
    /// previous page ended on.
    ///
    /// The second member is the `nextToken` for the page after this one, and is present
    /// only while reports MATCHING THE SAME FILTER remain — a page that exhausted them
    /// ends the listing.
    pub fn page_reports(
        &self,
        correlation_id: Option<&str>,
        resume_after: Option<&str>,
        page_size: usize,
    ) -> (Vec<ReportStatusListItem>, Option<String>) {
        let mut items = Vec::new();
        let mut next_token = None;
        let mut skipping = resume_after.is_some();
        let mut last_included: Option<&str> = None;
        for report_id in &self.order {
            if skipping {
                skipping = Some(report_id.as_str()) != resume_after;
                continue;
            }
            let Some(record) = self.reports.get(report_id) else {
                continue;
            };
            if correlation_id
                .is_some_and(|wanted| record.correlation_id.as_deref() != Some(wanted))
            {
                continue;
            }
            if items.len() >= page_size {
                // At least one more report matches, so hand back a cursor that resumes
                // exactly where this page ended.
                next_token = last_included.map(|cursor| self.page_token_for(correlation_id, cursor));
                break;
            }
            items.push(record.list_item());
            last_included = Some(report_id);
        }
        (items, next_token)
    }

    /// The opaque `nextToken` that resumes a listing of `correlation_id` after `cursor`.
    ///
    /// Hex of the authenticated bytes rather than base64: the token travels on a query
    /// string (`@httpQuery("nextToken")`), where base64's `+` is another character's
    /// encoding, and hex keeps every issued token inside `NextToken`'s modelled length.
    fn page_token_for(&self, correlation_id: Option<&str>, cursor: &str) -> String {
        ::hex::encode(
            self.page_token_key
                .mint(&format!("{}{CURSOR_SEPARATOR}{cursor}", list_scope(correlation_id))),
        )
    }

    /// The report a `nextToken` resumes after, or `None` for any token THIS listing did
    /// not issue: one that is not hex, one this store's secret does not authenticate, one
    /// minted for a listing under a different filter, and one naming a report the store
    /// does not hold.
    pub fn resume_cursor(&self, token: &str, correlation_id: Option<&str>) -> Option<String> {
        let cursor = self
            .page_token_key
            .cursor_of(&::hex::decode(token).ok()?)?;
        let (scope, report_id) = cursor.split_once(CURSOR_SEPARATOR)?;
        (scope == list_scope(correlation_id) && self.reports.contains_key(report_id))
            .then(|| report_id.to_string())
    }
}

/// The sequence a pagination cursor names a position in: the reports of this account
/// under the filter in force, as the one digest a cursor carries it by
/// ([`service_common::page_tokens::scope_digest`]).
fn list_scope(correlation_id: Option<&str>) -> String {
    match correlation_id {
        Some(filter) => scope_digest(&format!("reports?correlationId={filter}")),
        None => scope_digest("reports"),
    }
}

/// How many entries one page of a listing holds: the caller's `maxResults`, or
/// [`MAX_RESULTS_CEILING`] when the caller names none, clamped into the modelled range.
///
/// The boundary walk already refuses a `maxResults` outside that range before a handler
/// runs, so the clamp is defensive: it keeps the page size a bound on the work one
/// request can ask for whatever reaches it.
pub fn page_size(requested: Option<i32>) -> usize {
    requested
        .unwrap_or(MAX_RESULTS_CEILING)
        .clamp(1, MAX_RESULTS_CEILING) as usize
}

/// The outcome a report carries once it reaches `COMPLETED`.
///
/// This emulator investigates nothing, so the one thing it can answer with is a value
/// DERIVED from the report's own identity: a stable function of the `reportId` over the
/// `ReasonCode` vocabulary the model publishes ([`REASON_CODE_VALUES`]). The same report
/// therefore reports the same outcome on every read — a reader never sees it change —
/// while two reports need not share one. The mixing is the workspace's one stable
/// derivation ([`service_common::stable_hex_id`], SHA-256 over its parts).
fn resolution_reason_code(report_id: &str) -> &'static str {
    let digest = ::service_common::stable_hex_id(&["resolution", report_id]);
    let selector = usize::from_str_radix(digest.get(..8).unwrap_or("0"), 16).unwrap_or(0);
    REASON_CODE_VALUES[selector % REASON_CODE_VALUES.len()]
}
