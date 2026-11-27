// SPDX-License-Identifier: CC0-1.0

//! The customer-managed RUNTIME tree this control plane's second resource family is made
//! of, as this emulator persists it.
//!
//! `CreateRuntime` mints "a resource providing a centrally managed execution environment
//! for your Lambda functions", and that runtime "is the parent of the runtime version and
//! runtime alias child resources": a runtime version "defines the function execution
//! environment by referencing a container image" and is immutable but for its release
//! notes, and a runtime alias "is a named pointer to a runtime version". The records below
//! are the ONE copy of that tree every runtime handler reads and writes, plus the three
//! projections the model asks for (`RuntimeInfo`, `RuntimeVersionInfo`,
//! `RuntimeAliasInfo`).
//!
//! The lifecycle the three resources share is a SCHEDULED deletion: each `Delete…`
//! "schedules" its resource, which "enter[s] the `PENDING_DELETION` state and can be
//! restored during the scheduled deletion window", `RestoreRuntime` "cancels a pending
//! scheduled deletion, returning the resource to the `ACTIVE` state", and once the window
//! runs out the resource moves to `DELETING`, from which it "can no longer be restored".
//! Nothing sleeps: the instant each of those transitions falls due is stored on the record
//! and materialised by whichever later call reports it ([`Runtime::settle`]), so the
//! outcome is owed whether or not anybody calls.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use super::{
    invalid_parameter, state_in_words, PendingChange, PendingTransition,
    DELETION_COMPLETES_AFTER, WORK_COMPLETES_AFTER,
};
use crate::ops::types::{
    Compatibility, InvalidParameterValueException, ResourceConflictException,
    ResourceNotFoundException, RuntimeAliasInfo, RuntimeInfo, RuntimeReleaseLifecycle,
    RuntimeVersionInfo, ServiceQuotaExceededException,
};

/// `RuntimeState$ACTIVE` — "the runtime is active and available for use". The same spelling
/// is `RuntimeVersionState$ACTIVE` and `RuntimeAliasState$ACTIVE`, each documented the same
/// way, so one constant serves the three enums.
pub const STATE_ACTIVE: &str = "ACTIVE";
/// `RuntimeState$PENDING_DELETION` — "scheduled for deletion. It can be restored to ACTIVE
/// state until the date and time specified in `PendingDeletionAt`". Declared identically by
/// `RuntimeVersionState` and `RuntimeAliasState`.
pub const STATE_PENDING_DELETION: &str = "PENDING_DELETION";
/// `RuntimeState$DELETING` — "being deleted and can no longer be restored". Declared
/// identically by `RuntimeVersionState` and `RuntimeAliasState`.
pub const STATE_DELETING: &str = "DELETING";
/// `RuntimeState$FAILED` — "the runtime deletion failed to complete because the runtime's
/// versions were not removed in time. The runtime is no longer scheduled for deletion."
pub const STATE_FAILED: &str = "FAILED";
/// `RuntimeVersionState$CREATING` — "the runtime version is being created". Declared
/// identically by `RuntimeAliasState`.
pub const STATE_CREATING: &str = "CREATING";
/// `RuntimeAliasState$UPDATING` — "being updated to point to a new runtime version".
pub const STATE_UPDATING: &str = "UPDATING";

/// `RuntimeReleaseState$DEPRECATED` — "the runtime is deprecated".
pub const RELEASE_STATE_DEPRECATED: &str = "DEPRECATED";

/// How many runtimes the store holds before `CreateRuntime` refuses with the modelled
/// `ServiceQuotaExceededException` ("the request would exceed a service quota for your
/// account").
///
/// AWS publishes an adjustable per-account quota and this emulator models neither Service
/// Quotas nor an account's adjustments, so this is the emulator's OWN bound on the store
/// rather than a claim about any account's limit. It is far above any workflow an emulator
/// serves, and it is what stops the state growing without end.
pub const RUNTIME_QUOTA: usize = 1_000;

/// How many versions, and how many aliases, one runtime holds before `CreateRuntimeVersion`
/// / `CreateRuntimeAlias` refuse with that same modelled quota error. The emulator's own
/// bound, for the same reason as [`RUNTIME_QUOTA`].
pub const CHILDREN_PER_RUNTIME_QUOTA: usize = 100;

/// The largest page the runtime listings serve, and the page they serve when the request
/// names no `MaxResults`.
///
/// `MaxResults` is modelled `@range(min: 1, max: 100)` on all three listings and the lists
/// they fill (`RuntimeList`, `RuntimeVersionList`, `RuntimeAliasList`) declare no length
/// bound of their own, so the most one response can carry is what `MaxResults` admits.
pub const RUNTIME_PAGE_LIMIT: i32 = 100;

/// The waiting period a scheduled deletion gets when the request names none.
///
/// `RecoveryWindowInDays` is "optional. The waiting period, in days (7-30), before the
/// runtime is permanently deleted", and the model declares no default for it, so this is the
/// emulator's own choice: the longest window the parameter admits, which is the one that
/// discards least. A window the request DID name keeps the value it asked for
/// ([`recovery_window`]).
pub const DEFAULT_RECOVERY_WINDOW_IN_DAYS: i32 = 30;

/// Seconds in one of those days.
const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

/// How long a scheduled deletion waits before it executes: the `RecoveryWindowInDays` the
/// request asked for, or [`DEFAULT_RECOVERY_WINDOW_IN_DAYS`] when it named none.
///
/// The boundary validator refused anything outside the modelled `@range(min: 7, max: 30)`
/// before a handler ran, so the only case left is an absent value.
pub fn recovery_window(days: Option<i32>) -> Duration {
    let days = days.unwrap_or(DEFAULT_RECOVERY_WINDOW_IN_DAYS);
    Duration::from_secs(u64::try_from(days).unwrap_or(0).saturating_mul(SECONDS_PER_DAY))
}

/// `at` as a `RuntimeTimestamp`: "a timestamp, represented as a string".
///
/// The shape is a plain `string` with no `@timestampFormat` to read a rendering off, so the
/// rendering is the ISO-8601 / RFC 3339 instant in UTC that every AWS JSON API writes a
/// date-time string as — and the one the tree's single timestamp renderer
/// (`aws_smithy_types::DateTime`) produces for that format, so this service cannot disagree
/// with another about what an instant looks like. It is well inside the modelled
/// `@length(min: 1, max: 64)`.
pub fn timestamp(at: SystemTime) -> String {
    ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// How many characters a `RuntimeLifecycleDate` has: "a UTC calendar date, yyyy-mm-dd".
const LIFECYCLE_DATE_LENGTH: usize = 10;

/// Today's UTC calendar date, in the `yyyy-mm-dd` form `RuntimeLifecycleDate` declares.
///
/// Read off the same rendering [`timestamp`] produces, whose leading `yyyy-mm-dd` IS the UTC
/// calendar date of the instant; two dates in that form compare as strings exactly as they
/// compare as dates, which is what the lifecycle rules below are stated over.
pub fn today(now: SystemTime) -> String {
    let mut rendered = timestamp(now);
    rendered.truncate(LIFECYCLE_DATE_LENGTH);
    rendered
}

/// The resource part of a runtime's ARN, which `RuntimeArn` writes as
/// `runtime/<RuntimeName>`, optionally followed by `/version/<name>` or `/alias/<name>`.
const ARN_RESOURCE_TYPE: &str = "runtime";

/// What tells a `RuntimeIdentifier` that IS an ARN from one that is a bare `RuntimeName`:
/// the ARN form is `arn:…:<account>:runtime/<name>` and a `RuntimeName` is
/// `[a-z0-9][a-z0-9_.-]*[a-z0-9]`, which carries neither a colon nor a slash.
const ARN_RESOURCE_INFIX: &str = ":runtime/";

/// The unqualified ARN of the runtime named `name`, in `region`.
pub fn runtime_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("lambda", region, &format!("{ARN_RESOURCE_TYPE}/{name}"))
}

/// The ARN of version `version` of the runtime whose own ARN is `runtime_arn`:
/// `RuntimeArn`'s expression writes a child as `<runtime ARN>/(version|alias)/<name>`.
pub fn version_arn(runtime_arn: &str, version: &str) -> String {
    format!("{runtime_arn}/version/{version}")
}

/// The ARN of the alias `alias` of the runtime whose own ARN is `runtime_arn`.
pub fn alias_arn(runtime_arn: &str, alias: &str) -> String {
    format!("{runtime_arn}/alias/{alias}")
}

/// The runtime a `RuntimeIdentifier` names: "this can be either the runtime name or the
/// fully qualified runtime ARN", so an ARN contributes the resource part after
/// `:runtime/` and a bare value is the name already.
pub fn runtime_name_of(identifier: &str) -> &str {
    match identifier.split_once(ARN_RESOURCE_INFIX) {
        Some((_, name)) => name,
        None => identifier,
    }
}

/// Whether `identifier` is a runtime ARN of an account other than the caller's.
///
/// An account id is part of an ARN, so an ARN naming another account addresses a resource
/// this caller has no access to — an authorization refusal
/// ([`crate::state::identifier::CROSS_ACCOUNT_DENIED`]) and not an existence answer, decided
/// before the store is read so the answer says nothing about what this account holds. The
/// ARN is read through the tree's ONE ARN parse, and a bare `RuntimeName` parses as no ARN
/// at all.
pub fn names_another_account(identifier: &str) -> bool {
    ::service_common::arn::parse_of(identifier, "lambda")
        .is_some_and(|parsed| parsed.account != ::engine::emulator_account_id())
}

/// Which of the three resources of the tree a refusal is about: what the service calls it in
/// a sentence, singular and plural.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeResource {
    /// The parent: `CreateRuntime`'s "customer-managed runtime".
    Runtime,
    /// `CreateRuntimeVersion`'s "runtime version".
    Version,
    /// `CreateRuntimeAlias`'s "runtime alias".
    Alias,
}

impl RuntimeResource {
    /// The resource's name as a sentence STARTS with it.
    pub fn sentence_start(self) -> &'static str {
        match self {
            Self::Runtime => "Runtime",
            Self::Version => "Runtime version",
            Self::Alias => "Runtime alias",
        }
    }

    /// The resource's name inside a sentence.
    pub fn singular(self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Version => "runtime version",
            Self::Alias => "runtime alias",
        }
    }

    /// More than one of them.
    pub fn plural(self) -> &'static str {
        match self {
            Self::Runtime => "runtimes",
            Self::Version => "runtime versions",
            Self::Alias => "runtime aliases",
        }
    }
}

/// The `ResourceNotFoundException` payload for an identifier that names no resource of this
/// account and Region: the resource the request addressed, stated as the ARN that names it.
///
/// Every resource of this tree HAS an ARN the service can build from what the request
/// supplied — a runtime from its name, a child from its runtime and its own name — so the
/// refusal names the resource rather than echoing whichever of the two identifier forms the
/// caller chose, in the `<resource> not found: <arn>` form this service states a missing
/// resource in ([`crate::state::NOT_FOUND_PREFIX`] is the same sentence for its connectors).
pub fn not_found(resource: RuntimeResource, arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        // `Type` is "the exception type", a value no documented rule gives for this class, so
        // it is left unset rather than invented — as it is for every not-found this service
        // answers.
        r#type: None,
        message: Some(format!("{} not found: {arn}", resource.sentence_start())),
    }
}

/// What a `Name` already held is refused with: the runtime `Name` is unique within the
/// account and Region, and a child's `Name` within its runtime, so the sentence names the
/// colliding name in its quoted slot and the scope it collides in.
pub fn duplicate_name(
    resource: RuntimeResource,
    name: &str,
    runtime: Option<&str>,
) -> ResourceConflictException {
    let scope = match runtime {
        Some(runtime) => format!("for runtime '{runtime}'"),
        None => "in this account".to_string(),
    };
    super::connector_conflict(format!(
        "A {} with name '{name}' already exists {scope}",
        resource.singular()
    ))
}

/// What a request a resource's current state does not permit is refused with — which is what
/// `ResourceConflictException` is modelled for ("for example, attempting to modify a
/// resource that is not in a state that permits the operation").
///
/// The sentence is BUILT FROM THE RESOURCE: it names the resource and the state it is in,
/// spelled as this service spells a state's name in prose
/// (`crate::state::state_in_words`), then states the requirement — the same sentence shape
/// this service already states a connector's state gate in.
pub fn refused_in_state(
    resource: RuntimeResource,
    name: &str,
    state: &str,
    action: &str,
    required: &str,
) -> ResourceConflictException {
    super::connector_conflict(format!(
        "{} {name} is in state {} and cannot be {action}. Only {required} {} can be {action}.",
        resource.sentence_start(),
        state_in_words(state),
        resource.plural(),
    ))
}

/// What a delete of a runtime that still has live children is refused with: "runtime
/// versions and aliases must be deleted or scheduled for deletion before deleting the
/// runtime", so a runtime that still holds one conflicts with the request.
pub fn runtime_still_has_children(name: &str) -> ResourceConflictException {
    super::connector_conflict(format!(
        "Runtime {name} cannot be deleted. Runtime versions and aliases must be deleted or \
         scheduled for deletion before deleting the runtime."
    ))
}

/// What a request past one of the store's bounds is refused with: the modelled
/// `ServiceQuotaExceededException`, naming the quota it would exceed and the scope that
/// holds it.
pub fn quota_exceeded(
    resource: RuntimeResource,
    runtime: Option<&str>,
    quota: usize,
) -> ServiceQuotaExceededException {
    let message = match runtime {
        Some(runtime) => format!(
            "Runtime {runtime} has reached the maximum of {quota} {} allowed",
            resource.plural()
        ),
        None => format!(
            "This account has reached the maximum of {quota} {} allowed in this Region",
            resource.plural()
        ),
    };
    ServiceQuotaExceededException {
        r#type: None,
        message: Some(message),
    }
}

/// What `RestoreRuntime` refuses a request naming both of its optional scope parameters
/// with: they are "an optional, mutually exclusive `RuntimeVersion` or `RuntimeAlias` query
/// parameter", so a request carrying both names no one target to restore.
pub fn restore_scope_is_ambiguous() -> InvalidParameterValueException {
    invalid_parameter(
        "RuntimeVersion and RuntimeAlias are mutually exclusive: specify at most one"
            .to_string(),
    )
}

/// Whether the `ReleaseLifecycle` a request carries breaks the rule
/// `RuntimeReleaseLifecycle$ReleaseState` states, and the refusal if it does.
///
/// That rule, as the member states it: "`DEPRECATED` requires a `DeprecationDate` on or
/// before today (UTC); `PREVIEW` and `GA` require a later one, or none." A lifecycle that
/// declares no `ReleaseState` declares nothing for the rule to hold of — the member is
/// "optional, with no default applied" — so it passes whatever date it carries.
pub fn unusable_lifecycle(
    lifecycle: Option<&RuntimeReleaseLifecycle>,
    today: &str,
) -> Option<InvalidParameterValueException> {
    let lifecycle = lifecycle?;
    let release_state = lifecycle.release_state.as_deref()?;
    let deprecation_date = lifecycle.deprecation_date.as_deref();
    let reached = deprecation_date.is_some_and(|date| date <= today);
    if release_state == RELEASE_STATE_DEPRECATED {
        return (!reached).then(|| {
            invalid_parameter(format!(
                "ReleaseLifecycle release state {RELEASE_STATE_DEPRECATED} requires a \
                 DeprecationDate on or before today"
            ))
        });
    }
    reached.then(|| {
        invalid_parameter(format!(
            "ReleaseLifecycle release state {release_state} requires a DeprecationDate later \
             than today"
        ))
    })
}

/// The `ReleaseLifecycle` a read REPORTS for the one stored: the same lifecycle, except that
/// "reads report `DEPRECATED` once `DeprecationDate` is reached".
///
/// A projection and never a write: the state the clock moved the release through is computed
/// from the stored date at the instant of the read, so a read changes nothing a later caller
/// sees and the answer is the same whether or not anybody looked earlier.
pub fn reported_lifecycle(
    lifecycle: Option<&RuntimeReleaseLifecycle>,
    today: &str,
) -> Option<RuntimeReleaseLifecycle> {
    let mut reported = lifecycle?.clone();
    if reported
        .deprecation_date
        .as_deref()
        .is_some_and(|date| date <= today)
    {
        reported.release_state = Some(RELEASE_STATE_DEPRECATED.to_string());
    }
    Some(reported)
}

/// One runtime version: "a runtime version defines the function execution environment by
/// referencing a container image. Runtime versions are immutable."
#[derive(Debug, Clone)]
pub struct RuntimeVersion {
    pub name: String,
    /// `<runtime ARN>/version/<Name>`.
    pub arn: String,
    /// The `ImageUri` the create referenced, which is what the version resolves to
    /// ([`RuntimeVersion::resolved_uri`]).
    pub image_uri: String,
    pub release_notes: Option<String>,
    /// One of `RuntimeVersionState`'s values.
    pub state: String,
    /// The state this version takes once the work in flight on it falls due.
    pub pending: Option<PendingTransition>,
    /// When a scheduled deletion of this version executes — reported while, and only while,
    /// it is `PENDING_DELETION`.
    pub pending_deletion_at: Option<SystemTime>,
    pub created: SystemTime,
    pub last_modified: SystemTime,
}

impl RuntimeVersion {
    /// "The resolved image URI of the runtime version."
    ///
    /// This emulator reaches no registry — it opens no outbound connection — so there is no
    /// tag-to-digest resolution to report: the reference the create supplied IS what this
    /// version resolves to, and it is stable across every read of the record.
    pub fn resolved_uri(&self) -> String {
        self.image_uri.clone()
    }

    /// The compatibility matrix of this version.
    ///
    /// `Compatibility` reports which integration models, architectures and features the
    /// version's IMAGE supports. Nothing in this emulator inspects an image, so no measured
    /// matrix exists and the optional member is left unset rather than invented.
    pub fn compatibility(&self) -> Option<Compatibility> {
        None
    }

    /// Materialise what the clock owes this version; `true` when the record is due to be
    /// forgotten, which only the runtime that holds it can do.
    fn settle(&mut self, now: SystemTime) -> bool {
        loop {
            let Some(pending) = self.pending.clone() else {
                return false;
            };
            if now < pending.due {
                return false;
            }
            match pending.change {
                PendingChange::State(state) => {
                    self.state = state;
                    self.pending = completes_after_deleting(&self.state, pending.due);
                }
                PendingChange::Removal => return true,
            }
        }
    }

    /// The `RuntimeVersionInfo` projection `ListRuntimeVersions` returns.
    pub fn to_info(&self) -> RuntimeVersionInfo {
        RuntimeVersionInfo {
            name: self.name.clone(),
            arn: self.arn.clone(),
            resolved_uri: self.resolved_uri(),
            state: self.state.clone(),
            pending_deletion_at: pending_deletion_text(&self.state, self.pending_deletion_at),
            release_notes: self.release_notes.clone(),
            compatibility: self.compatibility(),
            created: timestamp(self.created),
            last_modified: timestamp(self.last_modified),
        }
    }
}

/// One runtime alias: "a named pointer to a runtime version. Functions consume the
/// customer-managed runtime through the runtime alias."
#[derive(Debug, Clone)]
pub struct RuntimeAlias {
    pub name: String,
    /// `<runtime ARN>/alias/<Name>`.
    pub arn: String,
    /// The `Name` of the runtime version this alias points at.
    pub runtime_version: String,
    pub description: Option<String>,
    pub release_lifecycle: Option<RuntimeReleaseLifecycle>,
    /// One of `RuntimeAliasState`'s values.
    pub state: String,
    pub pending: Option<PendingTransition>,
    pub pending_deletion_at: Option<SystemTime>,
    pub created: SystemTime,
    pub last_modified: SystemTime,
}

impl RuntimeAlias {
    /// Materialise what the clock owes this alias; `true` when the record is due to be
    /// forgotten.
    fn settle(&mut self, now: SystemTime) -> bool {
        loop {
            let Some(pending) = self.pending.clone() else {
                return false;
            };
            if now < pending.due {
                return false;
            }
            match pending.change {
                PendingChange::State(state) => {
                    self.state = state;
                    self.pending = completes_after_deleting(&self.state, pending.due);
                }
                PendingChange::Removal => return true,
            }
        }
    }

    /// The ARN of the runtime version this alias points at, under the runtime whose own ARN
    /// is `runtime_arn`.
    pub fn runtime_version_arn(&self, runtime_arn: &str) -> String {
        version_arn(runtime_arn, &self.runtime_version)
    }

    /// The `RuntimeAliasInfo` projection `ListRuntimeAliases` returns, for the runtime whose
    /// own ARN is `runtime_arn` and the version this alias points at.
    pub fn to_info(
        &self,
        runtime_arn: &str,
        version: Option<&RuntimeVersion>,
        today: &str,
    ) -> RuntimeAliasInfo {
        RuntimeAliasInfo {
            name: self.name.clone(),
            arn: self.arn.clone(),
            runtime_version: self.runtime_version.clone(),
            runtime_version_arn: self.runtime_version_arn(runtime_arn),
            state: self.state.clone(),
            pending_deletion_at: pending_deletion_text(&self.state, self.pending_deletion_at),
            description: self.description.clone(),
            // "The compatibility matrix of the runtime version the alias points to": the
            // version's own, which is whatever that record reports.
            compatibility: version.and_then(RuntimeVersion::compatibility),
            release_lifecycle: reported_lifecycle(self.release_lifecycle.as_ref(), today),
            created: timestamp(self.created),
            last_modified: timestamp(self.last_modified),
        }
    }
}

/// One customer-managed runtime, with the versions and aliases it parents.
#[derive(Debug, Clone)]
pub struct Runtime {
    pub name: String,
    /// "The unique identifier of the runtime", minted by
    /// [`crate::state::ComAmazonawsLambdacoreLambdacoreapiserviceState::mint_runtime_id`].
    pub id: String,
    /// `arn:aws:lambda:<region>:<account>:runtime/<Name>`.
    pub arn: String,
    pub description: Option<String>,
    pub license_info: Option<String>,
    pub release_lifecycle: Option<RuntimeReleaseLifecycle>,
    /// One of `RuntimeState`'s values.
    pub state: String,
    pub pending: Option<PendingTransition>,
    pub pending_deletion_at: Option<SystemTime>,
    pub tags: BTreeMap<String, String>,
    pub created: SystemTime,
    pub last_modified: SystemTime,
    /// Every persisted version, ordered by name — the stable total order the versions
    /// listing's `NextToken` indexes into.
    pub versions: BTreeMap<String, RuntimeVersion>,
    /// Every persisted alias, in that same kind of order.
    pub aliases: BTreeMap<String, RuntimeAlias>,
}

impl Runtime {
    /// Materialise every change the clock owes this runtime and its children; `true` when
    /// the record is due to be forgotten, which only the owner of the map can do.
    pub fn settle(&mut self, now: SystemTime) -> bool {
        let forgotten: Vec<String> = self
            .versions
            .iter_mut()
            .filter_map(|(name, version)| version.settle(now).then(|| name.clone()))
            .collect();
        for name in forgotten {
            self.versions.remove(&name);
        }
        let forgotten: Vec<String> = self
            .aliases
            .iter_mut()
            .filter_map(|(name, alias)| alias.settle(now).then(|| name.clone()))
            .collect();
        for name in forgotten {
            self.aliases.remove(&name);
        }

        loop {
            let Some(pending) = self.pending.clone() else {
                return false;
            };
            if now < pending.due {
                return false;
            }
            match pending.change {
                PendingChange::State(state) => {
                    // "The runtime deletion failed to complete because the runtime's
                    // versions were not removed in time. The runtime is no longer scheduled
                    // for deletion." A scheduled deletion that comes due while children
                    // remain is exactly that, so the runtime lands in FAILED and keeps no
                    // schedule.
                    if state == STATE_DELETING && self.has_children() {
                        self.state = STATE_FAILED.to_string();
                        self.pending = None;
                        self.pending_deletion_at = None;
                        return false;
                    }
                    self.state = state;
                    self.pending = completes_after_deleting(&self.state, pending.due);
                }
                PendingChange::Removal => return true,
            }
        }
    }

    /// Whether this runtime still holds a version or an alias.
    pub fn has_children(&self) -> bool {
        !self.versions.is_empty() || !self.aliases.is_empty()
    }

    /// Whether every child of this runtime is already deleted or scheduled for deletion,
    /// which is what `DeleteRuntime` requires before it will schedule the runtime itself.
    pub fn children_are_released(&self) -> bool {
        self.versions
            .values()
            .all(|version| is_released(&version.state))
            && self.aliases.values().all(|alias| is_released(&alias.state))
    }

    /// The `RuntimeInfo` projection `ListRuntimes` returns.
    pub fn to_info(&self, today: &str) -> RuntimeInfo {
        RuntimeInfo {
            arn: self.arn.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            license_info: self.license_info.clone(),
            release_lifecycle: reported_lifecycle(self.release_lifecycle.as_ref(), today),
            state: Some(self.state.clone()),
            pending_deletion_at: pending_deletion_text(&self.state, self.pending_deletion_at),
            created: timestamp(self.created),
            last_modified: timestamp(self.last_modified),
        }
    }
}

/// Whether a resource in `state` has been released by its owner: it is being deleted, or is
/// scheduled to be.
pub fn is_released(state: &str) -> bool {
    state == STATE_PENDING_DELETION || state == STATE_DELETING
}

/// `PendingDeletionAt` as an answer reports it: "present only when `State` is
/// `PENDING_DELETION`. The date and time at which the … will be permanently deleted."
fn pending_deletion_text(state: &str, at: Option<SystemTime>) -> Option<String> {
    (state == STATE_PENDING_DELETION)
        .then_some(at)
        .flatten()
        .map(timestamp)
}

/// What the clock owes a resource that has just reached `state`.
///
/// Only one state owes anything further: a resource that is `DELETING` "can no longer be
/// restored", so what remains is for the record to be forgotten, at
/// [`crate::state::DELETION_COMPLETES_AFTER`] past the instant the deletion began executing
/// (`began`) rather than past whenever somebody happened to look.
fn completes_after_deleting(state: &str, began: SystemTime) -> Option<PendingTransition> {
    (state == STATE_DELETING).then(|| PendingTransition {
        due: began
            .checked_add(DELETION_COMPLETES_AFTER)
            .unwrap_or(began),
        change: PendingChange::Removal,
        reason: None,
    })
}

/// The transition a scheduled deletion owes: the resource moves to `DELETING` once its
/// recovery window runs out, from where it can no longer be restored.
pub fn scheduled_deletion(executes_at: SystemTime) -> PendingTransition {
    PendingTransition {
        due: executes_at,
        change: PendingChange::State(STATE_DELETING.to_string()),
        reason: None,
    }
}

/// The transition the asynchronous half of a create or an update owes: the resource reaches
/// `ACTIVE` once the work the call started finishes, [`WORK_COMPLETES_AFTER`] past it.
///
/// `RuntimeVersionState$CREATING` is "being created" and `RuntimeAliasState$CREATING` "not
/// yet available for use", with `UPDATING` for an alias "being updated to point to a new
/// runtime version": each is a state the service ANSWERS reads with while the work is in
/// flight, so the window has to be long enough for the calls a client makes right after the
/// one that started it to land inside it. No documented duration says how long AWS takes, so
/// the span is this emulator's own — the same one its connectors provision over.
pub fn becomes_active(started: SystemTime) -> PendingTransition {
    PendingTransition {
        due: started
            .checked_add(WORK_COMPLETES_AFTER)
            .unwrap_or(started),
        change: PendingChange::State(STATE_ACTIVE.to_string()),
        reason: None,
    }
}

#[cfg(test)]
mod tests {
    //! The rules this module settles, each over a SET of inputs.

    use super::*;

    /// A lifecycle carrying `release_state` and `deprecation_date`.
    fn lifecycle(release_state: Option<&str>, deprecation_date: Option<&str>) -> RuntimeReleaseLifecycle {
        RuntimeReleaseLifecycle {
            release_state: release_state.map(str::to_string),
            deprecation_date: deprecation_date.map(str::to_string),
            function_create_block_date: None,
            function_update_block_date: None,
        }
    }

    /// RULE: `DEPRECATED` requires a `DeprecationDate` on or before today (UTC); `PREVIEW`
    /// and `GA` require a later one, or none.
    ///
    /// The rows span the rule rather than one example: every `RuntimeReleaseState` value the
    /// model declares (`smithy_query(query="shape",shape="RuntimeReleaseState")` ->
    /// `PREVIEW | GA | DEPRECATED`), against a past date, today itself (the boundary the rule
    /// words as "on or before"), a later date and no date at all.
    #[test]
    fn a_release_state_is_refused_against_the_deprecation_date_it_needs() {
        const TODAY: &str = "2026-10-01";
        let rows = [
            ("DEPRECATED, a past date", "DEPRECATED", Some("2020-01-01"), true),
            ("DEPRECATED, today itself", "DEPRECATED", Some(TODAY), true),
            ("DEPRECATED, a later date", "DEPRECATED", Some("2099-01-01"), false),
            ("DEPRECATED, no date", "DEPRECATED", None, false),
            ("GA, a past date", "GA", Some("2020-01-01"), false),
            ("GA, today itself", "GA", Some(TODAY), false),
            ("GA, a later date", "GA", Some("2099-01-01"), true),
            ("GA, no date", "GA", None, true),
            ("PREVIEW, a past date", "PREVIEW", Some("2020-01-01"), false),
            ("PREVIEW, today itself", "PREVIEW", Some(TODAY), false),
            ("PREVIEW, a later date", "PREVIEW", Some("2099-01-01"), true),
            ("PREVIEW, no date", "PREVIEW", None, true),
        ];
        for (case, release_state, date, accepted) in rows {
            let declared = lifecycle(Some(release_state), date);
            let refusal = unusable_lifecycle(Some(&declared), TODAY);
            assert_eq!(
                refusal.is_none(),
                accepted,
                "case {case:?}: expected accepted={accepted}, got {:?}",
                refusal.map(|refused| refused.message)
            );
        }
        // A lifecycle that declares no release state declares nothing for the rule to hold
        // of, whatever date it carries, and neither does an absent lifecycle.
        for (case, declared) in [
            ("no release state, a past date", Some(lifecycle(None, Some("2020-01-01")))),
            ("no release state, a later date", Some(lifecycle(None, Some("2099-01-01")))),
            ("no lifecycle at all", None),
        ] {
            let refusal = unusable_lifecycle(declared.as_ref(), TODAY);
            assert!(
                refusal.is_none(),
                "case {case:?}: expected the request to be accepted, got {:?}",
                refusal.map(|refused| refused.message)
            );
        }
    }

    /// RULE: reads report `DEPRECATED` once `DeprecationDate` is reached, and report the
    /// stored release state until then.
    #[test]
    fn a_read_reports_deprecated_once_the_deprecation_date_is_reached() {
        const TODAY: &str = "2026-10-01";
        let rows = [
            ("a date already past", Some("GA"), Some("2020-01-01"), Some("DEPRECATED")),
            ("today itself", Some("GA"), Some(TODAY), Some("DEPRECATED")),
            ("a later date", Some("GA"), Some("2099-01-01"), Some("GA")),
            ("a later date, preview", Some("PREVIEW"), Some("2099-01-01"), Some("PREVIEW")),
            ("no date at all", Some("GA"), None, Some("GA")),
            ("no release state, a past date", None, Some("2020-01-01"), Some("DEPRECATED")),
            ("no release state, no date", None, None, None),
        ];
        for (case, release_state, date, expected) in rows {
            let stored = lifecycle(release_state, date);
            let reported = reported_lifecycle(Some(&stored), TODAY)
                .unwrap_or_else(|| panic!("case {case:?}: a stored lifecycle is reported"));
            assert_eq!(
                reported.release_state.as_deref(),
                expected,
                "case {case:?}: expected the reported release state {expected:?}, got {:?}",
                reported.release_state
            );
            assert_eq!(
                reported.deprecation_date.as_deref(),
                date,
                "case {case:?}: the reported deprecation date is the stored one"
            );
        }
        assert!(
            reported_lifecycle(None, TODAY).is_none(),
            "a runtime that stores no lifecycle reports none"
        );
    }

    /// RULE: a `RuntimeIdentifier` is "either the runtime name or the fully qualified runtime
    /// ARN", and an ARN of another account names no resource this caller may reach.
    #[test]
    fn a_runtime_identifier_resolves_to_the_runtime_it_names() {
        let own = ::engine::emulator_account_id();
        let rows: Vec<(&str, String, &str, bool)> = vec![
            ("a bare name", "python3.14".to_string(), "python3.14", false),
            (
                "this account's runtime ARN",
                format!("arn:aws:lambda:us-west-2:{own}:runtime/python3.14"),
                "python3.14",
                false,
            ),
            (
                "another partition, this account",
                format!("arn:aws-us-gov:lambda:us-gov-west-1:{own}:runtime/gov.runtime"),
                "gov.runtime",
                false,
            ),
            (
                "another account's runtime ARN",
                "arn:aws:lambda:us-west-2:111122223333:runtime/theirs".to_string(),
                "theirs",
                true,
            ),
        ];
        for (case, identifier, name, cross_account) in rows {
            assert_eq!(
                runtime_name_of(&identifier),
                name,
                "case {case:?}: expected the identifier to name {name:?}"
            );
            assert_eq!(
                names_another_account(&identifier),
                cross_account,
                "case {case:?}: expected cross_account={cross_account}"
            );
        }
    }

    /// RULE: a scheduled deletion waits the `RecoveryWindowInDays` the request asked for, and
    /// the longest window the parameter admits when it asked for none.
    ///
    /// The rows are each published bound of the modelled `@range(min: 7, max: 30)` and one
    /// value inside it, plus the absent case.
    #[test]
    fn a_recovery_window_is_the_one_the_request_asked_for() {
        for (case, days, expected_days) in [
            ("the shortest window", Some(7), 7),
            ("a window inside the range", Some(14), 14),
            ("the longest window", Some(30), 30),
            ("no window named", None, DEFAULT_RECOVERY_WINDOW_IN_DAYS),
        ] {
            let window = recovery_window(days);
            let expected =
                Duration::from_secs(u64::try_from(expected_days).unwrap() * SECONDS_PER_DAY);
            assert_eq!(
                window, expected,
                "case {case:?}: expected {expected:?}, got {window:?}"
            );
        }
    }

    /// RULE: `PendingDeletionAt` is present only while the resource is `PENDING_DELETION`.
    #[test]
    fn pending_deletion_at_is_reported_only_while_the_deletion_is_pending() {
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        for (case, state, reported) in [
            ("scheduled for deletion", STATE_PENDING_DELETION, true),
            ("already executing", STATE_DELETING, false),
            ("active", STATE_ACTIVE, false),
            ("creating", STATE_CREATING, false),
            ("failed", STATE_FAILED, false),
        ] {
            let text = pending_deletion_text(state, Some(at));
            assert_eq!(
                text.is_some(),
                reported,
                "case {case:?}: expected reported={reported}, got {text:?}"
            );
            if reported {
                assert_eq!(
                    text.as_deref(),
                    Some(timestamp(at).as_str()),
                    "case {case:?}: the reported instant is the stored one"
                );
            }
        }
        assert!(
            pending_deletion_text(STATE_PENDING_DELETION, None).is_none(),
            "a record with no scheduled instant reports none"
        );
    }
}
