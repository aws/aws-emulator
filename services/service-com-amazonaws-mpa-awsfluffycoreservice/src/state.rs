// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-mpa-awsfluffycoreservice service.
//!
//! The approval team is this service's primary resource, and this module is the one
//! place it is stored: `CreateApprovalTeam` mints a record here, and
//! `GetApprovalTeam` / `ListApprovalTeams` / `UpdateApprovalTeam` /
//! `StartActiveApprovalTeamDeletion` all compute their answers from it.
//!
//! # Policies attached to a resource
//!
//! A team's `Policies` references are also the POLICIES ATTACHED to that team as a
//! resource, so the resource-policy reads (`GetResourcePolicy`) answer from the same
//! records rather than from a second copy, and resolve each reference through the
//! catalog below so that every member they report is the policy record's own:
//! [`ComAmazonawsMpaAwsfluffycoreserviceState::attached_resource_policy`] turns one
//! attached `QualifiedPolicyArn` into the [`ResourcePolicy`] those reads report
//! ([`parse_policy_version_arn`] reading the ARN back, `$DEFAULT` resolving to the
//! policy's current default version), and
//! [`ComAmazonawsMpaAwsfluffycoreserviceState::resource_policies`] /
//! [`ComAmazonawsMpaAwsfluffycoreserviceState::resource_policy`] resolve them for a
//! resource ARN.
//!
//! # The clock, and who materialises it
//!
//! Two changes are owed by the passage of time alone, so they are recorded as DUE
//! INSTANTS on the stored record and materialised by [`ComAmazonawsMpaAwsfluffycoreserviceState::materialize_due`],
//! which every handler calls before it reads or writes. Nothing sleeps.
//!
//! Each one is stamped with the instant it was DUE rather than with the instant it was
//! materialised, and two of them that bear on the same record are applied in the order
//! those instants fall, so the store a handler reads is the store a caller who polled at
//! every instant would have seen — and never a function of when the next request arrived.
//!
//! * An invited approver's response ([`INVITATION_RESPONSE_DELAY`]). A team whose every
//!   approver has accepted their invitation is `ACTIVE`; until then it is `PENDING`
//!   with status code `PENDING_ACTIVATION`, which is what AWS reports for a team whose
//!   approvers have not all answered.
//! * The approval of an active team's DELETION ([`DELETION_APPROVAL_DELAY`]) and, after
//!   it, the end of the pending window that approval starts
//!   ([`DEFAULT_PENDING_WINDOW_DAYS`]). Until the approval the team reports
//!   `DELETE_PENDING_APPROVAL`; once the window elapses the team is gone, so
//!   `GetApprovalTeam` answers 404 and `ListApprovalTeams` omits it. A deletion the team
//!   never approved removes nothing.
//! * The activation of a new identity source ([`IDENTITY_SOURCE_ACTIVATION_DELAY`]). A
//!   source is `CREATING` until it comes due and `ACTIVE` afterwards, which is what
//!   `IdentitySourceStatus`' non-terminal `CREATING` token stands for. The approval
//!   portal of the IAM Identity Center instance the source is associated with is
//!   recorded when that activation is materialised ([`IdentitySource::activate`],
//!   [`approval_portal_url`]), so a source reports a portal exactly once its
//!   association is live.
//! * The removal of an identity source whose deletion `DeleteIdentitySource` started
//!   ([`IDENTITY_SOURCE_DELETION_DELAY`]). The source is reported `DELETING` — the
//!   model's other non-terminal `IdentitySourceStatus` token — until the removal comes
//!   due, and is gone afterwards, so `ListIdentitySources` omits it. A removal is
//!   carried out only while the rule that permits it still holds at the instant it comes
//!   due: a source a stored team's approvers authenticate through is IN USE, so it stays
//!   `DELETING` with the removal still owed until no stored team names it any more.
//! * The activation of a new session ([`SESSION_ACTIVATION_DELAY`]) and, after it, that
//!   session's expiration ([`SESSION_EXPIRATION`]): a session is `CREATING` until its
//!   activation comes due, `PENDING` while it waits on its approvers, and `FAILED` with
//!   status code `EXPIRED` once its expiration has passed with no answer.
//! * The approval of a change an active team was asked for ([`UPDATE_APPROVAL_DELAY`])
//!   and, after it, that change's activation ([`UPDATE_ACTIVATION_DELAY`]): the team
//!   reports `UPDATE_PENDING_APPROVAL`, then `UPDATE_PENDING_ACTIVATION`, and the
//!   requested configuration becomes the live one when the activation comes due — which
//!   is the action that change's session approved being carried out, so from there the
//!   session reports its execution as `EXECUTED` ([`Session::record_execution`]).
//!
//! # Sessions
//!
//! A session is an approval request put to ONE team's approvers.
//! `StartApprovalTeamBaseline` starts a baseline session, `UpdateApprovalTeam` puts the
//! change it was asked for to the team that has to approve it and
//! `StartActiveApprovalTeamDeletion` puts the deletion of an active team to it
//! ([`SessionKind`]), and `GetSession` reports any of them out of its one record
//! ([`ComAmazonawsMpaAwsfluffycoreserviceState::session`]). No operation of this model
//! records an approver's vote, so a baseline session waits in `PENDING` until either its
//! expiration comes due or `CancelSession` ends it ([`Session::cancel`]); the answers
//! this store records are the ones the CLOCK owes, the approval of a requested team update
//! ([`UPDATE_APPROVAL_DELAY`]) and of a requested deletion
//! ([`DELETION_APPROVAL_DELAY`]). A team's
//! outstanding baseline session is also what `GetApprovalTeam` reports for each approver
//! ([`ComAmazonawsMpaAwsfluffycoreserviceState::pending_baseline_session_arn`]), so the
//! two reads cannot disagree.
//!
//! Beside its `Status`, a session reports the ACTION it approves
//! ([`SessionKind::action_name`]) and how far that action has got: `PENDING` while it is
//! still waiting to run and `EXECUTED` once this store has carried it out
//! ([`Session::record_execution`]).
//!
//! # An update an active team has to approve
//!
//! A change to the configuration an ACTIVE team's approvers approved is not applied where
//! it is asked for: `UpdateApprovalTeam` records it as a REQUESTED version
//! ([`PendingTeamUpdate`], [`ApprovalTeam::request_update`]) and puts it to the team in an
//! update session, so the team keeps its description, strategy, approvers, version id and
//! last update time until that change is approved and activated
//! ([`ApprovalTeam::activate_pending_update`]). A team whose configuration has never been
//! activated takes the change in place ([`ApprovalTeam::bump_version`]): it has no
//! approved configuration to protect. The handshake protects the CONFIGURATION, so a
//! request that asks for no configuration member to change is served while a version
//! waits: it asks the team's approvers nothing.
//!
//! # A deletion an active team has to approve
//!
//! The deletion of an ACTIVE team is the same handshake: `StartActiveApprovalTeamDeletion`
//! records it as a REQUESTED deletion ([`PendingTeamDeletion`],
//! [`ApprovalTeam::request_deletion`]) and puts it to the team in a deletion session, so
//! the team is removed only once that session carries the team's approval
//! ([`ApprovalTeam::record_deletion_approval`]) and the pending window measured from that
//! approval has elapsed. A session that ended without one is a deletion the team refused,
//! so the team stays ([`ApprovalTeam::record_deletion_failure`]).
//!
//! # Approver MFA devices
//!
//! Each approver carries the MFA devices they authenticate with
//! ([`Approver::mfa_methods`]), which is what `GetApprovalTeam` reports per approver.
//! A device is `OUT_OF_SYNC` until it is read back off the identity source
//! ([`Approver::synchronize_mfa_devices`]), and `UpdateApprovalTeam`'s
//! [`SYNCHRONIZE_MFA_DEVICES_ACTION`] is what asks for that, for every approver record
//! the team holds ([`ApprovalTeam::synchronize_mfa_devices`]). No read synchronizes one.
//!
//! # Identity sources
//!
//! The identity source is this service's other caller-created resource:
//! `CreateIdentitySource` mints an [`IdentitySource`] record here and
//! `ListIdentitySources` pages over them in creation order
//! ([`ComAmazonawsMpaAwsfluffycoreserviceState::identity_sources_in_creation_order`]),
//! so a listed entry is always read off the stored record.
//!
//! `DeleteIdentitySource` is what takes one out again: it puts the stored record into
//! `DELETING` ([`IdentitySource::begin_deletion`]) and the clock removes it
//! ([`ComAmazonawsMpaAwsfluffycoreserviceState::remove_identity_source`]), so the
//! record a later read reports — or omits — is this one store's. A source an approver
//! record of a stored team authenticates through is IN USE and is not removed at all
//! ([`ComAmazonawsMpaAwsfluffycoreserviceState::team_arn_using_identity_source`]): the
//! rule is the STORE's, so both paths that act on it read that one resolver — the
//! request that asks for the removal, which is refused while it holds, and the clock
//! pass that carries the removal out ([`ComAmazonawsMpaAwsfluffycoreserviceState::materialize_due`]),
//! which re-reads it at the instant the removal comes due and keeps the record while it
//! still holds.
//!
//! # The policy catalog
//!
//! Policies are AWS's, not the caller's: no operation in this model creates, updates or
//! deletes one, so the catalog is SEEDED when this store is first touched
//! ([`Default`]) and `ListPolicies` reports whatever it then holds. See
//! [`seeded_policy_catalog`].
//!
//! A published entry's identity and dates are the CATALOG's own, not this store's: the
//! display name `Name` reports is the policy's ([`CatalogPolicy::name`], a `PolicyName`
//! the ARN's name segment cannot be read back out of), each version carries the document
//! the catalog publishes for it ([`CatalogPolicyVersion::document`]), and the instants it
//! reports are the ones recorded for that version rather than the instant this store came
//! into being — so two stores answer a catalog read alike.
//!
//! Each catalog entry carries its VERSIONS ([`CatalogPolicy::versions`]), one of which is
//! the entry's default; `ListPolicyVersions` reports them for the policy its `PolicyArn`
//! names and `GetPolicyVersion` reports the one its `PolicyVersionArn` names, resolved
//! through [`ComAmazonawsMpaAwsfluffycoreserviceState::policy_version`]. A version is as
//! immutable as the policy that owns it — nothing in this model mints or retires one — so
//! no reader of the catalog writes to it.
//!
//! # Paging a listing
//!
//! Every paginated listing of this service pages the same way, so the rule lives here
//! rather than once per handler: [`list_page_size`] is the width one `MaxResults` asks
//! for, `cut_page` is the one place a page is CUT — at most one page of entries, and
//! the next token naming the last entry reported exactly while entries remain
//! ([`ListingPage`]) — and the two resume rules are stated over it once each:
//!
//! * [`page_after_position`], for the listings that page over a POSITION
//!   (`ListApprovalTeams`, `ListIdentitySources`, `ListSessions` over a record's stored
//!   position [`SeqOrdered`], `ListResourcePolicies` over an attachment's place in the
//!   resource's attachment list): the page is the records ordered after the position the
//!   request's token names.
//! * [`page_after_key`], for the listings that page over a KEY the store itself orders
//!   by (`ListPolicies` over the catalog's policy ARNs, `ListPolicyVersions` over one
//!   policy's version ids): the page is the map's entries ordered after the key the
//!   request's token names.
//!
//! A token is ONE listing's, so it carries the listing that issued it — the operation,
//! and the resource it pages over where it pages over one ([`Listing`],
//! [`listing_token`]) — and is only resumable there: a token this listing cannot read, a
//! token another listing issued, and a token naming a record this listing does not hold
//! are all refused ([`token_key`], [`page_after_position`], [`page_after_key`]) rather
//! than answered with a page. A listing hands a key out by RENDERING the value it names,
//! so one value has exactly one spelling in a token of this service's and a key is read
//! back only in that spelling (`canonical_key`): a key that merely parses to a value
//! this listing holds is a key no page of it ever handed out.
//!
//! Whether a token was handed out is decided against every record the listing pages over,
//! never against the narrower set a request's own narrowing admits
//! ([`page_after_position_where`], [`page_after_seq_where`], which a narrowed listing like
//! `ListSessions` pages with): the listing holding the record its token names is the whole
//! test, so a token stays resumable while that record is the resource's, whatever the
//! request admits and however the record has changed since.
//!
//! # Authorization
//!
//! This module also owns this service's authorization rules, so every handler states
//! them the same way and decides them BEFORE it resolves a resource:
//!
//! * [`authorize_resource_arn`] — an operation that identifies its resource by ARN is
//!   refused with `AccessDeniedException` (403) when that ARN is outside the caller's
//!   own account, whatever is stored under it.
//! * [`ComAmazonawsMpaAwsfluffycoreserviceState::visible_session`] — an operation
//!   addressed by a SESSION ARN is refused with `AccessDeniedException` (403) when that
//!   ARN resolves to no session the caller can see, because this service does not
//!   disclose whether such a session exists.
//! * [`ComAmazonawsMpaAwsfluffycoreserviceState::is_management_account`] /
//!   [`not_management_account`] — an operation an organization restricts to its
//!   management account is refused the same way for any other caller.
//!
//! # How this service words a rejection
//!
//! A rejection's wording is part of this service's contract, so the ones MPA states with
//! a FIXED sentence are built here and nowhere else, and every handler raises them
//! through these constructors instead of formatting prose of its own:
//!
//! * [`resource_not_found`] — the one message every ARN-addressed resource lookup in
//!   this service answers with ([`RESOURCE_NOT_FOUND_MESSAGE`]), for the resources whose
//!   existence it does report.
//! * [`access_denied`] / [`ACCESS_DENIED_MESSAGE`] — the one message a lookup whose
//!   resource's existence this service does NOT disclose is refused with.
//! * [`tagged_resource_not_found`] — the tag family's not-found, whose message IS the
//!   `ResourceArn` the request sent.
//! * [`invalid_arn`] / [`INVALID_ARN_MESSAGE`] — the one message a member that should
//!   carry an Amazon Resource Name and does not is refused with, raised by a handler for
//!   a member the model gives no ARN `@pattern` and by `crate::service`'s refusal seam
//!   ([`is_malformed_arn_violation`]) for the members it does.
//! * [`invalid_identity_source_arn`] / [`INVALID_IDENTITY_SOURCE_ARN_MESSAGE`] — the one
//!   message the two operations ADDRESSED BY an identity source's ARN refuse a value that
//!   addresses none with ([`addresses_identity_source`]); it names the member it rejected.
//! * [`reserved_tag_key`] — the one message every path that WRITES tags refuses a key in
//!   the [`RESERVED_TAG_KEY_PREFIX`] namespace with ([`names_reserved_tag_key`]), raised
//!   through the one ordered tag-write gate [`tag_write_breach`] below, which is also
//!   what decides which of the two tag rules a request breaking both is refused for.
//! * [`foreign_next_token`] / [`FOREIGN_NEXT_TOKEN_MESSAGE`] — the one message every
//!   paginated listing refuses a `NextToken` it did not issue with.
//! * [`unattachable_policy`] / [`policy_version_not_attachable`] — the two messages the
//!   one attach gate refuses a policy reference with
//!   ([`ComAmazonawsMpaAwsfluffycoreserviceState::unattachable_policy_reference`]): the
//!   first for a reference the catalog cannot resolve, the second for one it resolves to
//!   a version whose [`PolicyStatus`] no longer allows attaching it.
//! * [`repeated_identity`] — the one message either write path that takes an approver
//!   list refuses a repeated identity with.
//! * [`malformed_approver_identity_source`] — the malformed-ARN refusal either write path
//!   that takes an approver list answers a `PrimaryIdentitySourceArn` that is not an
//!   identity source's ARN with.
//!
//! Whatever the sentence, a refusal is a modelled response shape: every `Message` this
//! service declares is `@length(max: 1000)` while the members a refusal quotes back run
//! to 2048 characters on their own, so every message built by interpolating a request
//! value is held to that bound by [`bounded_message`].

use std::collections::BTreeMap;
use std::ops::Bound;
use std::time::{Duration, SystemTime};

use crate::ops::types::{AccessDeniedException, ResourceNotFoundException, ValidationException};

/// Region stamped into a minted ARN when the request carries no SigV4 credential
/// scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The IAM action namespace every MPA action carries, from this service's
/// `arnNamespace` / `sigv4Name` (`smithy_query(query="service_info")`: both `mpa`).
/// It is what an authorization refusal names the attempted action with
/// (`mpa:GetApprovalTeam`).
pub const SERVICE_ACTION_PREFIX: &str = "mpa";

/// Longest `Message` this service emits on a modelled error, from the
/// `@length(max: 1000)` every error's `Message` member declares — `ValidationException`,
/// `ConflictException`, `AccessDeniedException`, `ResourceNotFoundException`,
/// `InvalidParameterException`, `ServiceQuotaExceededException`, `TooManyTagsException`,
/// `ThrottlingException` and `InternalServerException` alike. It is the bound
/// [`bounded_message`] holds every refusal to.
pub const MAX_EXCEPTION_MESSAGE: usize = 1000;

/// The `UpdateAction` token that asks for every approver's MFA devices to be
/// synchronized with the identity source — the one value `UpdateAction` declares
/// (`smithy_query(query="shape",shape="UpdateAction")`), and so the whole catalog
/// `UpdateApprovalTeam` has to apply. The generated catalog
/// (`crate::ops::types::shared_types::UPDATE_ACTION_VALUES`) is the authority this is
/// read off, and a test asserts the two agree.
pub const SYNCHRONIZE_MFA_DEVICES_ACTION: &str = "SYNCHRONIZE_MFA_DEVICES";

/// How long after an approver is invited the emulator reports their invitation
/// response (and therefore how long a new team stays `PENDING`).
///
/// CHOSEN, not documented: a real approver answers an invitation whenever they
/// answer it. It sits inside the 250–750 ms window this project fixes for a
/// clock-owed transition, so a caller that reads the team back immediately still
/// observes the transient `PENDING` state and the errors that go with it.
pub const INVITATION_RESPONSE_DELAY: Duration = Duration::from_millis(500);

/// The instant an approver invited at `now` is owed their invitation response at, or
/// `None` when `now` names no instant this service could record that response at.
///
/// The ONE place that arithmetic lives: [`Approver::invited`] records it, and a write path
/// that records something else (an update session) before it builds an approver set checks
/// it ahead of that write, so the two cannot disagree about which requests can be served.
/// A caller that answers `None` with the modelled server error its operation declares
/// words it with [`UNCOMPUTABLE_INVITATION_RESPONSE`].
#[must_use]
pub fn invitation_response_due(now: SystemTime) -> Option<SystemTime> {
    now.checked_add(INVITATION_RESPONSE_DELAY)
}

/// The sentence every write path that invites approvers answers its modelled server error
/// with when [`invitation_response_due`] names no instant, so the two paths cannot word one
/// and the same internal failure differently.
pub const UNCOMPUTABLE_INVITATION_RESPONSE: &str =
    "the invited approvers' response instant could not be computed";

/// How long after an update is requested the team reports its approvers' approval of
/// that update (and therefore how long the team waits in `UPDATE_PENDING_APPROVAL`).
///
/// CHOSEN, not documented: a real approver answers an approval request whenever they
/// answer it, and this model carries no operation that records a vote — the approvers'
/// answer is modelled by the clock, exactly as their answer to an invitation is
/// ([`INVITATION_RESPONSE_DELAY`]). It sits inside the 250–750 ms window this project
/// fixes for a clock-owed transition, so a caller that reads the team back immediately
/// still observes the requested change waiting on its approval.
pub const UPDATE_APPROVAL_DELAY: Duration = Duration::from_millis(500);

/// How long after an update is approved the configuration it carries becomes the team's
/// live one (and therefore how long the team waits in `UPDATE_PENDING_ACTIVATION`).
///
/// CHOSEN, not documented: AWS publishes no duration for an approved team version to be
/// activated. It sits inside the 250–750 ms window this project fixes for a clock-owed
/// transition, so a caller that polls the team straight after the approval still observes
/// the transient `UPDATE_PENDING_ACTIVATION` state the model declares a token for.
pub const UPDATE_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// How long after the deletion of an ACTIVE team is requested the team reports its
/// approvers' approval of that deletion (and therefore how long the team waits in
/// `DELETE_PENDING_APPROVAL`).
///
/// CHOSEN, not documented: a real approver answers an approval request whenever they
/// answer it, and this model carries no operation that records a vote — the approvers'
/// answer is modelled by the clock, exactly as their answer to a requested update is
/// ([`UPDATE_APPROVAL_DELAY`]). It sits inside the 250–750 ms window this project fixes
/// for a clock-owed transition, so a caller that reads the team back immediately still
/// observes the deletion waiting on its approval.
///
/// It is NOT the pending window: that window is [`DEFAULT_PENDING_WINDOW_DAYS`] long by
/// default and starts where this delay ends, at the approval.
pub const DELETION_APPROVAL_DELAY: Duration = Duration::from_millis(500);

/// How long after `CreateIdentitySource` the emulator reports the new source as
/// `ACTIVE` (and therefore how long it stays `CREATING`).
///
/// CHOSEN, not documented: AWS publishes no duration for the association with an IAM
/// Identity Center instance to settle. It sits inside the 250–750 ms window this project
/// fixes for a clock-owed transition, so a caller that lists the source immediately
/// still observes the transient `CREATING` status the model declares.
pub const IDENTITY_SOURCE_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// How long after `DeleteIdentitySource` the emulator stops reporting the source at all
/// (and therefore how long it stays `DELETING`).
///
/// CHOSEN, not documented: AWS publishes no duration for an identity source's removal to
/// settle. It sits inside the 250–750 ms window this project fixes for a clock-owed
/// transition, so a caller that lists the source straight after the delete call still
/// observes the transient `DELETING` status the model declares.
pub const IDENTITY_SOURCE_DELETION_DELAY: Duration = Duration::from_millis(500);

/// How long after a session is started the emulator reports it as `PENDING` (and
/// therefore how long it stays `CREATING`).
///
/// CHOSEN, not documented: AWS publishes no duration for a session to be set up. It sits
/// inside the 250–750 ms window this project fixes for a clock-owed transition, so a
/// caller that reads the session back immediately still observes the transient
/// `CREATING` status the model declares.
pub const SESSION_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// How long a session waits for its approvers before it expires, measured from the
/// instant it was initiated.
///
/// CHOSEN, not documented: this model carries no session timeout and no request member
/// names one, so this is NOT a claim about the real service's deadline — it is the span
/// `ExpirationTime` reports and the instant at which the clock owes the session's
/// `FAILED` / `EXPIRED` outcome. It is deliberately NOT in the 250–750 ms band the
/// project fixes for a provisioning transition: this span is a DEADLINE for a human
/// answer, not a resource settling, and a sub-second one would expire every session
/// before any approver could see it.
pub const SESSION_EXPIRATION: Duration = Duration::from_secs(24 * 60 * 60);

/// Largest number of sessions the store will hold.
///
/// A ceiling, not a fidelity claim, and the same bound (and the same reason) as
/// [`MAX_APPROVAL_TEAMS`]: nothing authenticates a caller of this emulator, so without
/// one a client looping on `StartApprovalTeamBaseline` grows this process's heap and the
/// per-request [`ComAmazonawsMpaAwsfluffycoreserviceState::materialize_due`] walk without
/// bound. `StartApprovalTeamBaseline` declares no quota error, so it reports the ceiling
/// with the `ValidationException` (400) it does declare.
pub const MAX_SESSIONS: usize = 1_000;

/// Service principal of this service, as a session reports the service the approved
/// action belongs to (`RequesterServicePrincipal`, `length(1..100)`).
///
/// Built from this service's own endpoint prefix (`smithy_query(query="service_info")`:
/// `arnNamespace`/`sigv4Name` `mpa`, endpoint `mpa.{Region}.…`), in the
/// `<prefix>.amazonaws.com` form every AWS service principal takes.
pub const SERVICE_PRINCIPAL: &str = "mpa.amazonaws.com";

/// Days between the team's APPROVAL of a delete request and the team's removal when
/// `StartActiveApprovalTeamDeletion` omits `PendingWindowDays`.
///
/// `PendingWindowDays` is documented as "Number of days between when the team
/// approves the delete request and when the team is deleted"; AWS applies a 7-day
/// window when the request does not name one.
pub const DEFAULT_PENDING_WINDOW_DAYS: i32 = 7;

/// Seconds in one day, the unit `PendingWindowDays` counts in.
const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

/// Largest pending deletion window this emulator will record, in days (~100 years).
///
/// `PendingWindowDays` carries no modelled `range`, and AWS's own maximum is not in the
/// model, so this is NOT a claim about what the real service accepts: it is the bound
/// past which the window's end would stop being a timestamp this service can report, the
/// response member being an `IsoTimestamp` (`@timestampFormat("date-time")`). A request
/// past it is refused rather than answered with a completion instant that cannot be
/// rendered.
pub const MAX_PENDING_WINDOW_DAYS: i32 = 36_500;

/// Largest number of approval teams the store will hold.
///
/// A ceiling, not a fidelity claim: nothing authenticates a caller of this emulator, so
/// without one a client looping on `CreateApprovalTeam` grows this process's heap (and
/// the per-request [`ComAmazonawsMpaAwsfluffycoreserviceState::materialize_due`] walk)
/// without bound. `CreateApprovalTeam` reports it through the
/// `ServiceQuotaExceededException` (402) the model already declares for it, and
/// `name_index` / `idempotency_tokens` are bounded with it because
/// [`ComAmazonawsMpaAwsfluffycoreserviceState::remove_team`] frees a team's entries in
/// both when the team goes.
pub const MAX_APPROVAL_TEAMS: usize = 1_000;

/// Largest number of identity sources the store will hold.
///
/// From `ListIdentitySources`' own output: `IdentitySources` is `length(max: 20)` and
/// `MaxResults` is `range(1..20)`, so twenty is the most identity sources this service
/// can report at all. It is also what keeps a client looping on `CreateIdentitySource`
/// from growing this process's heap without bound; the model declares
/// `ServiceQuotaExceededException` (402) for exactly this condition.
pub const MAX_IDENTITY_SOURCES: usize = 20;

/// Largest page a paginated list operation of this service returns, from `MaxResults`'
/// modelled `range(min: 1, max: 20)` / `default: 20` — the same shape every one of them
/// binds (`ListApprovalTeams`, `ListPolicies`, …).
pub const MAX_LIST_PAGE: i32 = 20;

/// Width of one page, from the `MaxResults` the request carries.
///
/// `MaxResults` is modelled `range(1..20)` with `default: 20`, so the boundary validator
/// has already closed it; the clamp only keeps the slice width sane. Every paginated
/// listing of this service takes its page width from here, so none of them can disagree
/// about what a given `MaxResults` means.
#[must_use]
pub fn list_page_size(max_results: i32) -> usize {
    usize::try_from(max_results.clamp(1, MAX_LIST_PAGE)).unwrap_or(1)
}

/// A record a listing pages over in SEQUENCE order: the stable position this store gave
/// the record when it was written, which is the position its listing's `NextToken`
/// names.
pub trait SeqOrdered {
    /// This record's position in the order its listing reports.
    fn seq(&self) -> u64;
}

impl SeqOrdered for ApprovalTeam {
    fn seq(&self) -> u64 {
        self.seq
    }
}

impl SeqOrdered for IdentitySource {
    fn seq(&self) -> u64 {
        self.seq
    }
}

impl SeqOrdered for Session {
    fn seq(&self) -> u64 {
        self.seq
    }
}

/// One page of a paginated listing of this service: the records it reports, and the
/// token the NEXT page resumes after — absent once nothing is left, which is how a
/// client knows the listing is over.
#[derive(Debug)]
pub struct ListingPage<'a, T> {
    /// The records this page reports, in the order the listing held them.
    pub records: Vec<&'a T>,
    /// `NextToken`: this listing's token for the last record on this page, when records
    /// remain.
    pub next_token: Option<String>,
}

/// Which listing a pagination token belongs to — the operation, and the resource that
/// operation pages over where it pages over one.
///
/// A resume key is one listing's: the same creation-order position names a different
/// record in each listing, and in a listing of one resource's records a different record
/// for each resource. So the listing is part of the token and a token is resumable only
/// where it was issued ([`listing_token`], [`token_key`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Listing<'a> {
    /// `ListApprovalTeams`, over the stored teams in creation order.
    ApprovalTeams,
    /// `ListIdentitySources`, over the stored identity sources in creation order.
    IdentitySources,
    /// `ListSessions`, over ONE team's sessions in initiation order.
    Sessions {
        /// The team whose sessions are paged.
        approval_team_arn: &'a str,
    },
    /// `ListResourcePolicies`, over ONE resource's attached policies in attachment
    /// order.
    ResourcePolicies {
        /// The resource whose attached policies are paged.
        resource_arn: &'a str,
    },
    /// `ListPolicies`, over the policy catalog in ARN order.
    Policies,
    /// `ListPolicyVersions`, over ONE catalog policy's versions in version-id order.
    PolicyVersions {
        /// The policy whose versions are paged.
        policy_arn: &'a str,
    },
}

impl Listing<'_> {
    /// How this listing's own tokens name it.
    fn scope(self) -> String {
        match self {
            Self::ApprovalTeams => "approval-teams".to_string(),
            Self::IdentitySources => "identity-sources".to_string(),
            Self::Sessions { approval_team_arn } => format!("sessions/{approval_team_arn}"),
            Self::ResourcePolicies { resource_arn } => {
                format!("resource-policies/{resource_arn}")
            }
            Self::Policies => "policies".to_string(),
            Self::PolicyVersions { policy_arn } => format!("policy-versions/{policy_arn}"),
        }
    }
}

/// What separates a token's scope from the key it names. It is the FIRST such character
/// of a token that ends the scope, and the key keeps every later one, so a key that is
/// itself an ARN travels whole.
const TOKEN_SEPARATOR: char = ':';

/// The `NextToken` `listing` hands out to resume after the record `key` names — the
/// position of a record in a listing that pages over positions, the ordering key the
/// catalog itself holds in one that pages over a catalog key.
///
/// A `NextToken` is a value the client passes back UNREAD — "use this value in the
/// `NextToken` request parameter in a next call", nothing more — so what it is made of is
/// this service's own business: the listing it belongs to and the key it names, which is
/// what lets [`token_key`] tell a token of this listing's from every other. A scope is an
/// operation name or an ARN and a key is a position or an ARN, so a token stays far
/// inside `Token`'s `@length(max: 4096)`, and it carries no character that is special in
/// a query string or a JSON string, which is where the listings bind `NextToken`.
///
/// The form proves nothing by itself: what makes a token resumable is that the listing
/// reading it HOLDS the record its key names, which [`page_after_position`] and
/// [`page_after_key`] decide.
#[must_use]
pub fn listing_token(listing: Listing<'_>, key: &str) -> String {
    format!("{}{TOKEN_SEPARATOR}{key}", listing.scope())
}

/// The key `listing` resumes AFTER, read out of the request's `NextToken` — `None` when
/// the request carries no token, which starts the listing at the beginning.
///
/// This is the ONE place the token grammar of this service's listings lives: a token is
/// read only by the listing that issued it ([`listing_token`]), so anything else — a
/// value in no grammar of this service's, or a token another listing (or the same
/// listing of another resource) handed out — is refused with [`foreign_next_token`] and
/// the caller lifts that into its own operation's `ValidationException` (400). That the
/// key was also HANDED OUT by this listing is decided against the records themselves:
/// [`page_after_position`] for a position, [`page_after_key`] for a key the store orders
/// by.
pub fn token_key(
    listing: Listing<'_>,
    next_token: Option<&str>,
) -> Result<Option<String>, ValidationException> {
    let Some(token) = next_token.filter(|token| !token.is_empty()) else {
        return Ok(None);
    };
    // The scope is matched as a WHOLE: a key can carry an ARN, whose own separators are
    // no part of this grammar.
    let key = token
        .strip_prefix(&listing_token(listing, ""))
        .ok_or_else(foreign_next_token)?;
    Ok(Some(key.to_string()))
}

/// The `NextToken` a listing that pages over POSITIONS hands out to resume after
/// `position`: [`listing_token`] over the position.
#[must_use]
pub fn position_token(listing: Listing<'_>, position: u64) -> String {
    listing_token(listing, &position.to_string())
}

/// The position `listing` resumes AFTER, read out of the request's `NextToken` — `None`
/// when the request carries no token: [`token_key`] read as a position, in the one
/// spelling [`position_token`] renders a position in (`canonical_key`).
pub fn token_position(
    listing: Listing<'_>,
    next_token: Option<&str>,
) -> Result<Option<u64>, ValidationException> {
    match token_key(listing, next_token)? {
        None => Ok(None),
        Some(key) => canonical_key::<u64>(&key).map(Some),
    }
}

/// The value `key` names, read back only in the spelling this service MINTS for it —
/// [`foreign_next_token`] for any other text.
///
/// A listing hands a key out by RENDERING the value it names: a position through `u64`'s
/// `Display` ([`position_token`]), a catalog key through the map key's own
/// ([`page_after_key`]). So one value has exactly one spelling in a token of this
/// service's, and reading a key back has to reproduce the text a page handed out. Rust's
/// integer `FromStr` also accepts a leading sign and any amount of zero padding, and those
/// spellings render differently — they belong to no token this listing ever issued, so
/// they are refused here with the rest of the grammar rather than carried on as the value
/// they happen to parse as, which a held record would then answer with a page.
fn canonical_key<K>(key: &str) -> Result<K, ValidationException>
where
    K: ::std::str::FromStr + ::std::fmt::Display,
{
    let value = key.parse::<K>().map_err(|_| foreign_next_token())?;
    if value.to_string() != key {
        return Err(foreign_next_token());
    }
    Ok(value)
}

/// The ordering key `listing` resumes AFTER, read out of the request's `NextToken` in the
/// type the store's map is keyed by — `None` when the request carries no token: [`token_key`]
/// parsed as that key, the counterpart of [`token_position`] for a listing that pages over a
/// key the store orders by.
///
/// This is the GRAMMAR half of the token rule, and it needs none of the records: a token
/// naming another listing (or the same listing of another resource), and one whose key is
/// not a key of this map's type at all — or is not the spelling this listing renders such
/// a key as (`canonical_key`) — is refused with [`foreign_next_token`] here. That the
/// key was also HANDED OUT is decided against the records themselves, by
/// [`page_after_key`]. So a listing scoped to one named resource can read its token with
/// the rest of the request's shape, before it decides whether that resource exists.
pub fn token_map_key<K>(
    listing: Listing<'_>,
    next_token: Option<&str>,
) -> Result<Option<K>, ValidationException>
where
    K: ::std::str::FromStr + ::std::fmt::Display,
{
    match token_key(listing, next_token)? {
        None => Ok(None),
        Some(key) => canonical_key::<K>(&key).map(Some),
    }
}

/// One page cut out of `ordered`, and the token for the page after it.
///
/// This is the ONE place a page of this service is CUT, so the slice width and the
/// next-token condition cannot drift between the listings — whatever a listing pages
/// over and however it renders its token: the page is at most [`list_page_size`] entries
/// of `ordered`, and the token (`token_of` over the LAST entry reported) is handed out
/// exactly while entries remain, which is how a client knows the listing is over.
///
/// One entry past the page is read and no more: enough to know whether entries remain,
/// without materialising the whole tail of a listing that has no fixed size.
fn cut_page<I, U, F>(ordered: I, max_results: i32, token_of: F) -> (Vec<U>, Option<String>)
where
    I: IntoIterator<Item = U>,
    F: FnOnce(&U) -> String,
{
    let page_size = list_page_size(max_results);
    let mut reported: Vec<U> = ordered.into_iter().take(page_size + 1).collect();
    let more_remain = reported.len() > page_size;
    reported.truncate(page_size);
    let next_token = reported.last().filter(|_| more_remain).map(token_of);
    (reported, next_token)
}

/// One page of `ordered`, resuming after the position `after` names.
///
/// This is the ONE place the resume rule of a position listing lives, and the page is
/// cut by `cut_page` like every other: `ordered` is the records in the order the
/// listing reports them, each with its position; the page is the records ordered after
/// `after`, at most [`list_page_size`] of them; and the token for the page after names
/// the last record reported — absent once the page reached the end of `ordered`.
///
/// A position is RESUMABLE only where this listing handed it out, and this listing hands
/// out the position of a record it reported, so `after` has to be the position of a
/// record `ordered` still holds. Any other position — one past the end, one before the
/// first, one of a record that has since gone — is a position no page of this listing
/// handed out, so it is refused with [`foreign_next_token`] rather than answered with a
/// page the client would read as the end of the listing. Holding the record is the whole
/// test, so the position of the LAST record — which no page hands out, since a token is
/// issued only while records remain — is accepted and answers an empty final page.
pub fn page_after_position<'a, T, I>(
    listing: Listing<'_>,
    ordered: I,
    after: Option<u64>,
    max_results: i32,
) -> Result<ListingPage<'a, T>, ValidationException>
where
    T: 'a,
    I: IntoIterator<Item = (u64, &'a T)>,
{
    page_after_position_where(listing, ordered, |_| true, after, max_results)
}

/// One page of the records of `ordered` that `reported` admits, resuming after the
/// position `after` names: [`page_after_position`] for a listing whose REQUEST narrows
/// what it reports.
///
/// `ordered` is every record this listing pages over, each with its position, and
/// `reported` is the question the request asks of a record — so the page is the admitted
/// records ordered after `after`, and the token for the page after names the last one
/// reported.
///
/// A token's provenance is decided against `ordered`, never against the narrower set
/// `reported` admits: a position was handed out if this listing HOLDS the record it names,
/// and the request's own narrowing is no part of that. So a token stays resumable for as
/// long as its record is one of the listing's, whatever the narrowing now admits and
/// however the record has changed since — a client paging a narrowed listing reaches its
/// next page rather than being told the token it was just handed was never issued.
pub fn page_after_position_where<'a, T, I, P>(
    listing: Listing<'_>,
    ordered: I,
    mut reported: P,
    after: Option<u64>,
    max_results: i32,
) -> Result<ListingPage<'a, T>, ValidationException>
where
    T: 'a,
    I: IntoIterator<Item = (u64, &'a T)>,
    P: FnMut(&'a T) -> bool,
{
    let mut handed_out = after.is_none();
    let mut records: Vec<(u64, &'a T)> = Vec::new();
    for (position, record) in ordered {
        match after {
            Some(after) if position <= after => handed_out |= position == after,
            _ => {
                if reported(record) {
                    records.push((position, record));
                }
            }
        }
    }
    if !handed_out {
        return Err(foreign_next_token());
    }
    let (records, next_token) = cut_page(records, max_results, |(position, _)| {
        position_token(listing, *position)
    });
    Ok(ListingPage {
        records: records.into_iter().map(|(_, record)| record).collect(),
        next_token,
    })
}

/// One page of `ordered`, resuming after the key `after` names.
///
/// This is the ONE place the resume rule of a listing that pages over a key the STORE
/// orders by lives, and the page is cut by `cut_page` like every other: `ordered` is
/// the records keyed by that ordering key (a policy ARN for `ListPolicies`, a version id
/// for `ListPolicyVersions`); the page is the entries ordered after `after`, at most
/// [`list_page_size`] of them; and the token for the page after names the key of the
/// last entry reported — absent once the page reached the end of the map.
///
/// A key is RESUMABLE only where this listing handed it out, and this listing hands out
/// the key of an entry it reported, so `after` has to be a key `ordered` still holds:
/// any other key is refused with [`foreign_next_token`] rather than answered with a page
/// the client would read as the end of the listing. The key of the LAST entry — which no
/// page hands out, since a token is issued only while entries remain — is accepted and
/// answers an empty final page, exactly as a position is in [`page_after_position`].
pub fn page_after_key<'a, K, T>(
    listing: Listing<'_>,
    ordered: &'a BTreeMap<K, T>,
    after: Option<&K>,
    max_results: i32,
) -> Result<ListingPage<'a, T>, ValidationException>
where
    K: Ord + ::std::fmt::Display,
{
    if after.is_some_and(|key| !ordered.contains_key(key)) {
        return Err(foreign_next_token());
    }
    let start = match after {
        None => Bound::Unbounded,
        Some(key) => Bound::Excluded(key),
    };
    let (records, next_token) = cut_page(
        ordered.range((start, Bound::Unbounded)),
        max_results,
        |(key, _)| listing_token(listing, &key.to_string()),
    );
    Ok(ListingPage {
        records: records.into_iter().map(|(_, record)| record).collect(),
        next_token,
    })
}

/// One page of `ordered`, resuming after the position `after` names, for a listing whose
/// records carry their own position ([`SeqOrdered`]): [`page_after_position`] over the
/// stored positions.
pub fn page_after_seq<'a, T, I>(
    listing: Listing<'_>,
    ordered: I,
    after: Option<u64>,
    max_results: i32,
) -> Result<ListingPage<'a, T>, ValidationException>
where
    T: SeqOrdered + 'a,
    I: IntoIterator<Item = &'a T>,
{
    page_after_seq_where(listing, ordered, |_| true, after, max_results)
}

/// One page of the records of `ordered` that `reported` admits, resuming after the
/// position `after` names, for a listing whose records carry their own position
/// ([`SeqOrdered`]): [`page_after_position_where`] over the stored positions.
///
/// The whole of `ordered` is what the token's provenance is decided against, so a
/// listing narrowed by its request hands `ordered` every record it pages over and asks
/// `reported` which of them this request wants — rather than narrowing `ordered` itself,
/// which would make the provenance of its own tokens depend on what the request admits.
pub fn page_after_seq_where<'a, T, I, P>(
    listing: Listing<'_>,
    ordered: I,
    reported: P,
    after: Option<u64>,
    max_results: i32,
) -> Result<ListingPage<'a, T>, ValidationException>
where
    T: SeqOrdered + 'a,
    I: IntoIterator<Item = &'a T>,
    P: FnMut(&'a T) -> bool,
{
    page_after_position_where(
        listing,
        ordered.into_iter().map(|record| (record.seq(), record)),
        reported,
        after,
        max_results,
    )
}

/// One page of `ordered`, the page width and the resume position read off the request's
/// `MaxResults` and `NextToken`: [`token_position`] then [`page_after_seq`], the whole
/// seq-token paging rule in one call for a listing that decides nothing between reading
/// the token and cutting the page.
pub fn page_by_seq_token<'a, T, I>(
    listing: Listing<'_>,
    ordered: I,
    next_token: Option<&str>,
    max_results: i32,
) -> Result<ListingPage<'a, T>, ValidationException>
where
    T: SeqOrdered + 'a,
    I: IntoIterator<Item = &'a T>,
{
    let after = token_position(listing, next_token)?;
    page_after_seq(listing, ordered, after, max_results)
}

/// One page of `ordered`, the page width and the resume key read off the request's
/// `MaxResults` and `NextToken`: [`token_map_key`] then [`page_after_key`], the whole
/// key-token paging rule in one call for a listing that pages over a key the store
/// orders by and decides nothing between reading the token and cutting the page.
///
/// The key is read back in the type the map is keyed by, so a token whose key is not a
/// key of this map at all — one that does not parse, or one the map does not hold — is
/// refused with [`foreign_next_token`], and a key handed out always reads back as the
/// key it was rendered from.
pub fn page_by_key_token<'a, K, T>(
    listing: Listing<'_>,
    ordered: &'a BTreeMap<K, T>,
    next_token: Option<&str>,
    max_results: i32,
) -> Result<ListingPage<'a, T>, ValidationException>
where
    K: Ord + ::std::fmt::Display + ::std::str::FromStr,
{
    let after = token_map_key::<K>(listing, next_token)?;
    page_after_key(listing, ordered, after.as_ref(), max_results)
}

/// Largest number of tags one resource holds.
///
/// AWS's tagging limits allow 50 tags per resource, and this service's own model
/// corroborates that number: `UntagResource`'s `TagKeys` is `length(min: 0, max: 50)`
/// (`smithy_query(query="shape",shape="TagKeyList")`), so a single untag call can name
/// every key a resource is able to carry. `TagResource` reports a request that would
/// take a resource past it with the `TooManyTagsException` (400) the model declares for
/// exactly that condition.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// Number of tags a resource holding `existing` would hold once `incoming` keys are
/// merged into it.
///
/// Tagging is a per-key upsert, so a key the resource already carries replaces its
/// value and adds nothing to the count; only keys it does not carry yet do.
///
/// The two borrows are independent: `existing` is only read, and only the keys
/// `incoming` adds are held, so `incoming` may be borrowed from anywhere.
#[must_use]
pub fn merged_tag_count<'a, I>(existing: &BTreeMap<String, String>, incoming: I) -> usize
where
    I: IntoIterator<Item = &'a str>,
{
    let added: ::std::collections::BTreeSet<&str> = incoming
        .into_iter()
        .filter(|key| !existing.contains_key(*key))
        .collect();
    existing.len() + added.len()
}

/// Prefix a tag KEY carries when it belongs to Amazon Web Services rather than to the
/// caller.
///
/// AWS's tagging rules reserve this key namespace for AWS itself: a caller can neither
/// create nor change a tag whose key starts with it, so no request this service serves
/// may write one — see [`names_reserved_tag_key`]. The comparison ignores case, so the
/// namespace cannot be entered by spelling the prefix differently.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` lies in the reserved namespace [`RESERVED_TAG_KEY_PREFIX`] names.
#[must_use]
pub fn is_reserved_tag_key(key: &str) -> bool {
    let prefix = RESERVED_TAG_KEY_PREFIX.as_bytes();
    key.as_bytes()
        .get(..prefix.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
}

/// Whether any of `keys` lies in the reserved namespace, which is what every path that
/// WRITES tags refuses a request for ([`reserved_tag_key`]).
#[must_use]
pub fn names_reserved_tag_key<'a, I>(keys: I) -> bool
where
    I: IntoIterator<Item = &'a str>,
{
    keys.into_iter().any(is_reserved_tag_key)
}

/// The refusal a request that would write a tag key in the reserved namespace earns.
///
/// The message names the member and the constraint it breaks and no value the request
/// carried: a `TagKey` is `@sensitive`, so the key itself is not quoted back.
#[must_use]
pub fn reserved_tag_key() -> ValidationException {
    ValidationException {
        message: format!(
            "Tags names a key in the '{RESERVED_TAG_KEY_PREFIX}' namespace, which is \
             reserved for Amazon Web Services. Choose a key outside it, and try again."
        ),
    }
}

/// Which of the two rules a path that WRITES tags holds a request to the request breaks.
///
/// Every writing path raises its own modelled error for each of them — a create has only
/// `ValidationException` to answer with, while `TagResource` declares the
/// `TooManyTagsException` the model gives the ceiling condition — so the breach is reported
/// as a VALUE here and worded by the path that raised it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagWriteBreach {
    /// `Tags` names a key in the namespace [`RESERVED_TAG_KEY_PREFIX`] reserves, refused
    /// with [`reserved_tag_key`].
    ReservedKey,
    /// The resource would be left holding this many tags, more than
    /// [`MAX_TAGS_PER_RESOURCE`].
    PastResourceCeiling(usize),
}

/// The breach a tag write earns, or `None` when the request breaks neither rule.
///
/// The ONE place the two tag rules are decided, and therefore the one place their ORDER
/// is stated: a request is held to the constraints on its own INPUT before the resource
/// it names is held to a limit on the state that input would leave behind. The reserved
/// namespace is a property of the keys the request carries, and `Tags` carries no modelled
/// length constraint at all, so the ceiling is the per-resource LIMIT
/// `TooManyTagsException` is declared for ("the request exceeds the maximum number of tags
/// allowed for this resource") rather than a constraint on the map. So the namespace is
/// decided FIRST and the ceiling second, and a map that breaks both earns the namespace
/// refusal on every path that writes tags instead of one refusal per path.
///
/// `resulting_tag_count` is how many tags the resource would hold once the request was
/// written: the request's own `Tags` on a create, which writes a resource that does not
/// exist yet, and [`merged_tag_count`] on a path that merges into tags a resource already
/// carries.
#[must_use]
pub fn tag_write_breach<'a, I>(keys: I, resulting_tag_count: usize) -> Option<TagWriteBreach>
where
    I: IntoIterator<Item = &'a str>,
{
    if names_reserved_tag_key(keys) {
        return Some(TagWriteBreach::ReservedKey);
    }
    (resulting_tag_count > MAX_TAGS_PER_RESOURCE)
        .then_some(TagWriteBreach::PastResourceCeiling(resulting_tag_count))
}

/// The refusal a CREATE request's `Tags` earns, or `None` when it breaks neither tag rule.
///
/// Create is the path that writes a resource's tags before the resource exists, so the
/// count it holds against the ceiling is the request's own `Tags` — and because either
/// refusal is decided before anything is stored, no resource is ever stored past the
/// ceiling or already carrying a key a caller may not write. A create declares neither the
/// `TooManyTagsException` `TagResource` reports the ceiling with nor a second validation
/// error, so both breaches are the `ValidationException` (400) a create does declare,
/// which each create lifts into its own error type — decided in the order
/// [`tag_write_breach`] fixes for every writing path.
#[must_use]
pub fn create_tags_refusal(
    tags: Option<&::std::collections::HashMap<String, String>>,
) -> Option<ValidationException> {
    let keys = tags
        .into_iter()
        .flat_map(|tags| tags.keys().map(String::as_str));
    match tag_write_breach(keys, tags.map_or(0, ::std::collections::HashMap::len))? {
        TagWriteBreach::ReservedKey => Some(reserved_tag_key()),
        TagWriteBreach::PastResourceCeiling(tag_count) => {
            Some(tags_past_resource_ceiling(tag_count))
        }
    }
}

/// The refusal a create request whose `Tags` would leave the resource holding `tag_count`
/// tags, more than [`MAX_TAGS_PER_RESOURCE`], earns.
///
/// The message names the member, how many tags the request carried and the ceiling, and no
/// value the request carried: a `TagKey` and a `TagValue` are `@sensitive`.
#[must_use]
pub fn tags_past_resource_ceiling(tag_count: usize) -> ValidationException {
    ValidationException {
        message: format!(
            "Tags names {tag_count} tags, more than the maximum of \
             {MAX_TAGS_PER_RESOURCE} tags allowed per resource."
        ),
    }
}

/// The refusal a `Policies` reference that names no policy version this service holds
/// earns.
///
/// The reference is a `QualifiedPolicyArn`, which carries no `@sensitive` trait, so the
/// refusal names the ARN that resolved to nothing — held, like every refusal that quotes
/// a request value back, to the bound its own `Message` member declares
/// ([`bounded_message`]): a `QualifiedPolicyArn` is `@length(max: 1224)`, longer on its
/// own than the message it goes into.
#[must_use]
pub fn unattachable_policy(policy_arn: &str) -> ValidationException {
    ValidationException {
        message: bounded_message(format!(
            "Policies names a policy version that does not exist: {policy_arn}. Attach a \
             policy version ListPolicies and ListPolicyVersions report."
        )),
    }
}

/// The refusal a `Policies` reference that resolves to a policy version whose status no
/// longer allows attaching it earns.
///
/// The version EXISTS — the catalog reads keep reporting it — so the refusal names the
/// status that stops it being attached, read off the catalog record, and the status an
/// attachable version carries. Like every message that quotes a request value back, it is
/// held to the bound its own `Message` member declares ([`bounded_message`]).
#[must_use]
pub fn policy_version_not_attachable(
    policy_arn: &str,
    status: PolicyStatus,
) -> ValidationException {
    ValidationException {
        message: bounded_message(format!(
            "Policies names a policy version whose status is {}: {policy_arn}. Attach a \
             policy version ListPolicyVersions reports as {}.",
            status.as_wire(),
            PolicyStatus::Attachable.as_wire()
        )),
    }
}

/// The pending deletion window `days` names, or `None` when `days` is not a day count
/// this service can turn into a duration.
#[must_use]
pub fn pending_window(days: i32) -> Option<Duration> {
    u64::try_from(days)
        .ok()?
        .checked_mul(SECONDS_PER_DAY)
        .map(Duration::from_secs)
}

/// The instant a pending deletion window of `days` started at `start` ends, or `None`
/// when `days` names no instant this service can record and report.
///
/// Every step is checked, so no request can make this arithmetic panic the handler.
#[must_use]
pub fn deletion_completion(start: SystemTime, days: i32) -> Option<SystemTime> {
    if !(1..=MAX_PENDING_WINDOW_DAYS).contains(&days) {
        return None;
    }
    start.checked_add(pending_window(days)?)
}

/// The two instants a delete request schedules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeletionSchedule {
    /// Instant the team's answer to the deletion session comes due
    /// ([`DELETION_APPROVAL_DELAY`]).
    pub approval_due: SystemTime,
    /// Instant the removal the approved deletion owes becomes due: the pending window
    /// measured from the APPROVAL, which is where `PendingWindowDays` counts from.
    pub completion_time: SystemTime,
}

/// What a delete request made at `start` schedules, or `None` when `days` names no window
/// this service can record and report.
///
/// The pending window runs from the team's APPROVAL of the request, not from the request,
/// so the completion instant is [`DELETION_APPROVAL_DELAY`] (the span after which that
/// approval is owed, and the instant it is stamped with) plus `days` days. Every step is
/// checked, so no request can make this arithmetic panic a handler.
#[must_use]
pub fn deletion_schedule(start: SystemTime, days: i32) -> Option<DeletionSchedule> {
    let approval_due = start.checked_add(DELETION_APPROVAL_DELAY)?;
    Some(DeletionSchedule {
        approval_due,
        completion_time: deletion_completion(approval_due, days)?,
    })
}

/// Status of an approval team, in the tokens `ApprovalTeamStatus` declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamStatus {
    Active,
    Inactive,
    Deleting,
    Pending,
}

impl TeamStatus {
    /// The wire token, as `ApprovalTeamStatus` spells it.
    #[must_use]
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::Inactive => "INACTIVE",
            Self::Deleting => "DELETING",
            Self::Pending => "PENDING",
        }
    }
}

/// Status code of an approval team, in the tokens `ApprovalTeamStatusCode` declares.
/// Only the codes this slice of the lifecycle can reach are modelled here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamStatusCode {
    PendingActivation,
    DeletePendingApproval,
    DeleteFailedApproval,
    UpdatePendingApproval,
    UpdatePendingActivation,
    UpdateFailedApproval,
}

impl TeamStatusCode {
    /// The wire token, as `ApprovalTeamStatusCode` spells it.
    #[must_use]
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::PendingActivation => "PENDING_ACTIVATION",
            Self::DeletePendingApproval => "DELETE_PENDING_APPROVAL",
            Self::DeleteFailedApproval => "DELETE_FAILED_APPROVAL",
            Self::UpdatePendingApproval => "UPDATE_PENDING_APPROVAL",
            Self::UpdatePendingActivation => "UPDATE_PENDING_ACTIVATION",
            Self::UpdateFailedApproval => "UPDATE_FAILED_APPROVAL",
        }
    }
}

/// Status of an approver's identity, in the tokens `IdentityStatus` declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApproverIdentityStatus {
    Pending,
    Accepted,
    Rejected,
    Invalid,
}

impl ApproverIdentityStatus {
    /// The wire token, as `IdentityStatus` spells it.
    #[must_use]
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Accepted => "ACCEPTED",
            Self::Rejected => "REJECTED",
            Self::Invalid => "INVALID",
        }
    }
}

/// An approver's last activity, in the tokens `ApproverLastActivity` declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApproverLastActivity {
    Baselined,
    RespondedToInvitation,
    Voted,
}

impl ApproverLastActivity {
    /// The wire token, as `ApproverLastActivity` spells it.
    #[must_use]
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Baselined => "BASELINED",
            Self::RespondedToInvitation => "RESPONDED_TO_INVITATION",
            Self::Voted => "VOTED",
        }
    }
}

/// How a team grants approval. Mirrors the `ApprovalStrategy` union, which today
/// declares the single `MofN` variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoredApprovalStrategy {
    MofN { min_approvals_required: i32 },
}

impl StoredApprovalStrategy {
    /// Minimum approvals (M) the strategy requires.
    #[must_use]
    pub fn min_approvals_required(self) -> i32 {
        match self {
            Self::MofN {
                min_approvals_required,
            } => min_approvals_required,
        }
    }
}

/// Type of MFA device an approver authenticates with, in the tokens `MfaType`
/// declares (`smithy_query(query="shape",shape="MfaType")`: `EMAIL_OTP`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MfaType {
    EmailOtp,
}

impl MfaType {
    /// The wire token, as `MfaType` spells it.
    #[must_use]
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::EmailOtp => "EMAIL_OTP",
        }
    }
}

/// Whether an approver's MFA device is in sync with the identity source that manages
/// their authentication, in the tokens `MfaSyncStatus` declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MfaSyncStatus {
    InSync,
    OutOfSync,
}

impl MfaSyncStatus {
    /// The wire token, as `MfaSyncStatus` spells it.
    #[must_use]
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::InSync => "IN_SYNC",
            Self::OutOfSync => "OUT_OF_SYNC",
        }
    }
}

/// One MFA device of one approver: the configuration the wire `MfaMethod` reports, and
/// the synchronization state [`Approver::synchronize_mfa_devices`] moves. Named for the
/// STORED record, as [`StoredApprovalStrategy`] is, so it is never mistaken for the
/// generated wire type of the same shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoredMfaMethod {
    pub mfa_type: MfaType,
    pub sync_status: MfaSyncStatus,
}

/// The MFA configuration an approver starts with: the one device type this model
/// declares, not yet read back from the identity source.
///
/// A device is `OUT_OF_SYNC` until a synchronization reads it back off the identity
/// source, which is what the `SYNCHRONIZE_MFA_DEVICES` update action asks for; nothing
/// else in this model synchronizes one. The list stays within `MfaMethods`' modelled
/// `@length(max: 5)`. Private, so [`Approver::invited`] stays the one place an
/// approver's MFA state is seeded.
fn initial_mfa_methods() -> Vec<StoredMfaMethod> {
    vec![StoredMfaMethod {
        mfa_type: MfaType::EmailOtp,
        sync_status: MfaSyncStatus::OutOfSync,
    }]
}

/// One approver on a team.
#[derive(Debug, Clone)]
pub struct Approver {
    /// Service-minted id for this participant, stable for as long as the approver
    /// stays on the team.
    pub approver_id: String,
    pub primary_identity_id: String,
    pub primary_identity_source_arn: String,
    pub primary_identity_status: ApproverIdentityStatus,
    pub response_time: Option<SystemTime>,
    pub last_activity: Option<ApproverLastActivity>,
    pub last_activity_time: Option<SystemTime>,
    /// Instant at which this approver's invitation response becomes due. `None`
    /// once it has been materialised.
    pub invitation_response_due: Option<SystemTime>,
    /// MFA devices this approver authenticates with, each with its synchronization
    /// state, as `GetApprovalTeamResponseApprover$MfaMethods` reports them.
    pub mfa_methods: Vec<StoredMfaMethod>,
}

impl Approver {
    /// A freshly invited approver: `PENDING` until the clock owes its response, or `None`
    /// when `now` names no instant this service could record that response at.
    ///
    /// The instant is computed with CHECKED arithmetic, like every other clock-owed
    /// instant this module records ([`PendingTeamUpdate::requested`],
    /// [`Session::for_team`], [`deletion_schedule`]): the calling handler answers a
    /// `None` with the server error it declares, so no approver is ever stored waiting on
    /// a response owed at no instant at all and no request can panic part-way through an
    /// approver set.
    #[must_use]
    pub fn invited(
        approver_id: String,
        primary_identity_id: String,
        primary_identity_source_arn: String,
        now: SystemTime,
    ) -> Option<Self> {
        Some(Self {
            approver_id,
            primary_identity_id,
            primary_identity_source_arn,
            primary_identity_status: ApproverIdentityStatus::Pending,
            response_time: None,
            last_activity: None,
            last_activity_time: None,
            invitation_response_due: Some(invitation_response_due(now)?),
            mfa_methods: initial_mfa_methods(),
        })
    }

    /// Synchronize this approver's MFA devices with the identity source: every device
    /// is read back, so each one is in sync afterwards.
    pub fn synchronize_mfa_devices(&mut self) {
        for method in &mut self.mfa_methods {
            method.sync_status = MfaSyncStatus::InSync;
        }
    }
}

/// A change to a team's configuration that its approvers have been asked to approve: the
/// VERSION the team will carry once the change is approved and activated, which is what
/// `GetApprovalTeam` reports in `PendingUpdate` beside the live configuration.
///
/// The requested configuration lives HERE, not on the team, so a reader of the team sees
/// the configuration the approvers already approved until this one takes its place
/// ([`ApprovalTeam::activate_pending_update`]).
#[derive(Debug, Clone)]
pub struct PendingTeamUpdate {
    /// Wire version id of the requested version: the decimal rendering of
    /// [`PendingTeamUpdate::version`]. Never one a live version of this team has carried.
    pub version_id: String,
    pub version: u64,
    pub description: String,
    pub approval_strategy: StoredApprovalStrategy,
    /// The approver set the requested change produces: an approver the change keeps
    /// carries the id it already had, one it adds was invited when the change was
    /// requested.
    pub approvers: Vec<Approver>,
    /// Status of the requested VERSION, as `PendingUpdate$Status` reports it: `PENDING`
    /// while it waits, `INACTIVE` once it is a version the team will never move to.
    pub status: TeamStatus,
    pub status_code: Option<TeamStatusCode>,
    pub status_message: Option<String>,
    pub update_initiation_time: SystemTime,
    /// The approval session this change was put to the team's approvers in, as
    /// `GetApprovalTeamResponse$UpdateSessionArn` names it.
    pub update_session_arn: String,
    /// Instant at which the approvers' answer to that session comes due
    /// ([`UPDATE_APPROVAL_DELAY`]). `None` once it has been materialised — and only
    /// then: `UpdateApprovalTeam` refuses a request whose approval instant it cannot
    /// compute before it writes anything, so a recorded change always starts with one.
    pub approval_due: Option<SystemTime>,
    /// Instant at which the approved change's activation comes due
    /// ([`UPDATE_ACTIVATION_DELAY`]). `None` until the change is approved.
    pub activation_due: Option<SystemTime>,
}

impl PendingTeamUpdate {
    /// A freshly requested change: `PENDING` with `UPDATE_PENDING_APPROVAL` until the
    /// team's answer to `update_session_arn` comes due.
    #[must_use]
    pub fn requested(
        version: u64,
        description: String,
        approval_strategy: StoredApprovalStrategy,
        approvers: Vec<Approver>,
        update_session_arn: String,
        now: SystemTime,
    ) -> Self {
        Self {
            version_id: version.to_string(),
            version,
            description,
            approval_strategy,
            approvers,
            status: TeamStatus::Pending,
            status_code: Some(TeamStatusCode::UpdatePendingApproval),
            status_message: None,
            update_initiation_time: now,
            update_session_arn,
            approval_due: now.checked_add(UPDATE_APPROVAL_DELAY),
            activation_due: None,
        }
    }

    /// Whether this change is still on its way to becoming the team's live
    /// configuration — waiting on the approvers, or on the activation they approved.
    #[must_use]
    pub fn is_outstanding(&self) -> bool {
        self.status == TeamStatus::Pending
    }

    /// Total number of approvers (N) the requested change produces, as
    /// `PendingUpdate$NumberOfApprovers` reports it.
    #[must_use]
    pub fn number_of_approvers(&self) -> i32 {
        i32::try_from(self.approvers.len()).unwrap_or(i32::MAX)
    }
}

/// The deletion of an ACTIVE team that its approvers have been asked to approve: the
/// request `StartActiveApprovalTeamDeletion` recorded, and the two instants it answers
/// with.
///
/// A team is removed only once this record carries the team's APPROVAL
/// ([`PendingTeamDeletion::approval_time`]) and the pending window measured from it has
/// elapsed ([`PendingTeamDeletion::removal_due`]), so a deletion nobody approved removes
/// nothing.
#[derive(Debug, Clone)]
pub struct PendingTeamDeletion {
    /// `PendingWindowDays` as the request named it (or the default): the number of days
    /// between the team's approval of the request and the team's removal.
    pub pending_window_days: i32,
    /// The approval session the request was put to the team's approvers in.
    pub session_arn: String,
    /// Instant the deletion process was initiated, as `DeletionStartTime` reports it.
    pub start_time: SystemTime,
    /// Instant the approvers' answer to that session comes due
    /// ([`DELETION_APPROVAL_DELAY`]). `None` once it has been materialised.
    pub approval_due: Option<SystemTime>,
    /// Instant the team APPROVED the deletion, and so the instant the pending window
    /// starts. `None` until the approval is recorded.
    pub approval_time: Option<SystemTime>,
    /// Instant the deletion is scheduled to complete, as `DeletionCompletionTime` reports
    /// it: the approval instant plus the pending window. It is fixed when the request is
    /// recorded and never moves, because the approval the clock owes is stamped with the
    /// instant it was DUE.
    pub completion_time: SystemTime,
}

impl PendingTeamDeletion {
    /// A freshly requested deletion: waiting on the team's approval, with the window that
    /// approval starts already scheduled ([`deletion_schedule`]).
    #[must_use]
    pub fn requested(
        pending_window_days: i32,
        session_arn: String,
        start: SystemTime,
        schedule: DeletionSchedule,
    ) -> Self {
        Self {
            pending_window_days,
            session_arn,
            start_time: start,
            approval_due: Some(schedule.approval_due),
            approval_time: None,
            completion_time: schedule.completion_time,
        }
    }

    /// Instant the team's removal is due, or `None` while the deletion carries no
    /// approval: the window `PendingWindowDays` names runs from the APPROVAL, so an
    /// unapproved deletion owes no removal at any instant.
    #[must_use]
    pub fn removal_due(&self) -> Option<SystemTime> {
        self.approval_time.map(|_| self.completion_time)
    }
}

/// A stored approval team: everything the lifecycle operations report.
#[derive(Debug, Clone)]
pub struct ApprovalTeam {
    pub arn: String,
    pub name: String,
    pub description: String,
    pub approval_strategy: StoredApprovalStrategy,
    pub approvers: Vec<Approver>,
    /// The `PolicyArn` of each attached policy, in the order the request listed them.
    pub policies: Vec<String>,
    /// Wire version id: the decimal rendering of [`ApprovalTeam::version`].
    pub version_id: String,
    /// Version the team's LIVE configuration carries.
    pub version: u64,
    /// Highest version number this team has ever minted, live or requested. A version id
    /// is never reused, so a requested change that was not approved does not hand its id
    /// to the next one ([`ApprovalTeam::mint_version`]).
    pub version_seq: u64,
    pub status: TeamStatus,
    pub status_code: Option<TeamStatusCode>,
    pub status_message: Option<String>,
    pub creation_time: SystemTime,
    pub last_update_time: Option<SystemTime>,
    /// The deletion the team's approvers were asked to approve, if any — the record
    /// `StartActiveApprovalTeamDeletion` writes, and the only thing that can make the
    /// clock remove this team.
    pub pending_deletion: Option<PendingTeamDeletion>,
    /// The change the team's approvers were last asked to approve, if any. It is the
    /// requested version `GetApprovalTeam` reports in `PendingUpdate`, and it is cleared
    /// once that version becomes the team's live one.
    pub pending_update: Option<PendingTeamUpdate>,
    pub tags: BTreeMap<String, String>,
    /// Creation order, the stable ordering `ListApprovalTeams` pages over.
    pub seq: u64,
}

impl ApprovalTeam {
    /// A fresh, never-reused version number for this team.
    pub fn mint_version(&mut self) -> u64 {
        self.version_seq = self.version_seq.max(self.version) + 1;
        self.version_seq
    }

    /// Record a new version of the team IN PLACE: bump the version id and the update
    /// time.
    ///
    /// This is the change a team whose configuration no approver has approved yet takes —
    /// a team still waiting on its activation, or one no longer active — and the change an
    /// ACTIVE team takes where the request asks for no configuration member to change at
    /// all (an update action alone), which its approvers have nothing to approve. Either
    /// way the caller is answered with the version recorded here. A change to the
    /// configuration an ACTIVE team's approvers approved goes through
    /// [`ApprovalTeam::request_update`] instead, which leaves the live version id and
    /// update time alone: the version that change mints is the one the caller is answered
    /// with, and it becomes live only at its activation
    /// ([`ApprovalTeam::activate_pending_update`]).
    pub fn bump_version(&mut self, now: SystemTime) {
        self.version = self.mint_version();
        self.version_id = self.version.to_string();
        self.last_update_time = Some(now);
    }

    /// EVERY approver record this team holds: its LIVE approvers, and the approvers of
    /// the change it has been asked to approve ([`ApprovalTeam::pending_update`]).
    ///
    /// A team keeps two approver sets, and both are real: the requested change's set is
    /// invited when the change is requested, is reported member-for-member in
    /// `PendingUpdate$Approvers`, and becomes the live set when the change is activated
    /// ([`ApprovalTeam::activate_pending_update`]). So a decision about the identities or
    /// the identity sources a team depends on reads this, not one of the two sets: the
    /// one that is live now is not the only one this store holds.
    pub fn approver_records(&self) -> impl Iterator<Item = &Approver> {
        self.approvers.iter().chain(
            self.pending_update
                .iter()
                .flat_map(|update| update.approvers.iter()),
        )
    }

    /// The same two sets, for update — what a change the clock owes an approver record
    /// is applied through ([`ComAmazonawsMpaAwsfluffycoreserviceState::materialize_due`]).
    pub fn approver_records_mut(&mut self) -> impl Iterator<Item = &mut Approver> {
        let requested = self
            .pending_update
            .as_mut()
            .map(|update| update.approvers.as_mut_slice())
            .unwrap_or_default();
        self.approvers.iter_mut().chain(requested)
    }

    /// Put a requested change to the team's approvers: the team keeps the configuration
    /// and the version it has, and reports that it is waiting on their approval.
    pub fn request_update(&mut self, requested: PendingTeamUpdate) {
        self.status = TeamStatus::Pending;
        self.status_code = requested.status_code;
        self.pending_update = Some(requested);
    }

    /// The requested change that is still on its way to becoming this team's live
    /// configuration, if there is one.
    #[must_use]
    pub fn outstanding_update(&self) -> Option<&PendingTeamUpdate> {
        self.pending_update
            .as_ref()
            .filter(|update| update.is_outstanding())
    }

    /// Record the approvers' approval of the requested change: from here the change waits
    /// only on its activation ([`UPDATE_ACTIVATION_DELAY`]), and each approver who
    /// answered has voting as their last activity.
    pub fn record_update_approval(&mut self, at: SystemTime, answered: &[String]) {
        let Some(update) = self.pending_update.as_mut() else {
            return;
        };
        update.approval_due = None;
        update.status_code = Some(TeamStatusCode::UpdatePendingActivation);
        update.activation_due = at.checked_add(UPDATE_ACTIVATION_DELAY);
        self.status_code = Some(TeamStatusCode::UpdatePendingActivation);
        for approver in &mut self.approvers {
            if answered.iter().any(|id| id == &approver.approver_id) {
                approver.last_activity = Some(ApproverLastActivity::Voted);
                approver.last_activity_time = Some(at);
            }
        }
    }

    /// Record that the requested change was NOT approved: the session it was put in ended
    /// without the team's approval, so the team keeps the configuration and the version it
    /// has, and the requested version is one it will never move to.
    pub fn record_update_failure(&mut self) {
        let Some(update) = self.pending_update.as_mut() else {
            return;
        };
        update.approval_due = None;
        update.activation_due = None;
        update.status = TeamStatus::Inactive;
        update.status_code = Some(TeamStatusCode::UpdateFailedApproval);
        self.status = TeamStatus::Active;
        self.status_code = Some(TeamStatusCode::UpdateFailedApproval);
    }

    /// The approved change becomes the team's LIVE configuration: the version it carried
    /// is now the team's, and the team is active again with nothing pending.
    ///
    /// An approver the change keeps is carried over with the record the LIVE team holds,
    /// so an answer the clock materialised while the change waited is not lost; one the
    /// change adds joins with the record minted when it was requested.
    pub fn activate_pending_update(&mut self, at: SystemTime) {
        let Some(update) = self.pending_update.take() else {
            return;
        };
        let mut live: BTreeMap<String, Approver> = self
            .approvers
            .drain(..)
            .map(|approver| (approver.approver_id.clone(), approver))
            .collect();
        self.approvers = update
            .approvers
            .into_iter()
            .map(|requested| {
                live.remove(&requested.approver_id)
                    .unwrap_or(requested)
            })
            .collect();
        self.description = update.description;
        self.approval_strategy = update.approval_strategy;
        self.version = update.version;
        self.version_seq = self.version_seq.max(update.version);
        self.version_id = update.version_id;
        self.last_update_time = Some(at);
        self.status = TeamStatus::Active;
        self.status_code = None;
    }

    /// Total number of approvers (N), as `NumberOfApprovers` reports it.
    #[must_use]
    pub fn number_of_approvers(&self) -> i32 {
        i32::try_from(self.approvers.len()).unwrap_or(i32::MAX)
    }

    /// Drop the requested version this team reports, which is what a caller asking for an
    /// inactive version or a draft to be deleted asks for. The team keeps its live
    /// configuration, and stops reporting the outcome of a change whose version is gone.
    pub fn remove_pending_update(&mut self) {
        self.pending_update = None;
        if self.status_code == Some(TeamStatusCode::UpdateFailedApproval) {
            self.status_code = None;
        }
    }

    /// Synchronize the MFA devices of EVERY approver record on the team — the effect the
    /// `SYNCHRONIZE_MFA_DEVICES` update action asks for.
    ///
    /// A device belongs to the approver's IDENTITY, not to one version of the team, so the
    /// records of both sets the team holds ([`ApprovalTeam::approver_records_mut`]) are
    /// synchronized: an approver the team carries live and in the version it has been
    /// asked to approve cannot be reported in sync under one and out of sync under the
    /// other.
    pub fn synchronize_mfa_devices(&mut self) {
        for approver in self.approver_records_mut() {
            approver.synchronize_mfa_devices();
        }
    }

    /// Put a requested DELETION to the team's approvers: the team reports `DELETING` with
    /// `DELETE_PENDING_APPROVAL` and keeps everything it has, because a deletion the team
    /// has not approved yet removes nothing.
    pub fn request_deletion(&mut self, requested: PendingTeamDeletion) {
        self.status = TeamStatus::Deleting;
        self.status_code = Some(TeamStatusCode::DeletePendingApproval);
        self.pending_deletion = Some(requested);
    }

    /// The deletion that is still waiting on the team's approval, if there is one.
    #[must_use]
    pub fn unapproved_deletion(&self) -> Option<&PendingTeamDeletion> {
        self.pending_deletion
            .as_ref()
            .filter(|deletion| deletion.approval_time.is_none())
    }

    /// Record the approvers' approval of the requested deletion: from here the pending
    /// window runs, so the removal is owed at its end, and each approver who answered has
    /// voting as their last activity.
    ///
    /// Nothing is pending the team's approval afterwards, so the team reports `DELETING`
    /// with no status code: `ApprovalTeamStatusCode` carries no token for an approved
    /// deletion inside its window.
    pub fn record_deletion_approval(&mut self, at: SystemTime, answered: &[String]) {
        let Some(deletion) = self.pending_deletion.as_mut() else {
            return;
        };
        deletion.approval_due = None;
        deletion.approval_time = Some(at);
        self.status_code = None;
        for approver in &mut self.approvers {
            if answered.iter().any(|id| id == &approver.approver_id) {
                approver.last_activity = Some(ApproverLastActivity::Voted);
                approver.last_activity_time = Some(at);
            }
        }
    }

    /// Record that the requested deletion was NOT approved: the session it was put in
    /// ended without the team's approval, so the team is not deleted and is active again,
    /// reporting the failed approval.
    pub fn record_deletion_failure(&mut self) {
        if self.pending_deletion.take().is_none() {
            return;
        }
        self.status = TeamStatus::Active;
        self.status_code = Some(TeamStatusCode::DeleteFailedApproval);
    }
}

/// The type of resource that provides an identity source's identities, in the tokens
/// `IdentitySourceType` declares (`smithy_query(query="shape",shape="IdentitySourceType")`:
/// `IAM_IDENTITY_CENTER`). It is read off the `IdentitySourceParameters` a create request
/// carries, whose only member today is `IamIdentityCenter`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentitySourceType {
    IamIdentityCenter,
}

impl IdentitySourceType {
    /// The wire token, as `IdentitySourceType` spells it.
    #[must_use]
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::IamIdentityCenter => "IAM_IDENTITY_CENTER",
        }
    }
}

/// Status of an identity source, in the tokens `IdentitySourceStatus` declares.
///
/// `CREATING` and `ACTIVE` are the two a freshly created source moves through, and
/// `DELETING` the one `DeleteIdentitySource` records until the removal it started comes
/// due ([`ComAmazonawsMpaAwsfluffycoreserviceState::materialize_due`] owes every one of
/// those transitions). `ERROR` is for a failed association with the IAM Identity Center
/// instance, which nothing in this emulator produces — no operation of this model reports
/// an association failure — so nothing constructs it today; it is carried here so the
/// enum is the model's whole token catalog rather than the subset one operation reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentitySourceStatus {
    Creating,
    Active,
    Deleting,
    Error,
}

impl IdentitySourceStatus {
    /// The wire token, as `IdentitySourceStatus` spells it.
    #[must_use]
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Active => "ACTIVE",
            Self::Deleting => "DELETING",
            Self::Error => "ERROR",
        }
    }
}

/// A stored identity source: everything the identity-source operations report.
#[derive(Debug, Clone)]
pub struct IdentitySource {
    pub arn: String,
    pub source_type: IdentitySourceType,
    /// `IamIdentityCenter$InstanceArn`: the IAM Identity Center instance that provides
    /// this source's identities, as the create request named it.
    pub instance_arn: String,
    /// `IamIdentityCenter$Region`: the region of that instance, as the create request
    /// named it. It is the INSTANCE's region, which need not be the region the identity
    /// source's own ARN carries.
    pub instance_region: String,
    /// `IamIdentityCenter$ApprovalPortalUrl`: the address of the approval portal that
    /// belongs to this source's IAM Identity Center instance ([`approval_portal_url`]).
    ///
    /// The portal exists once the association with the instance does, so it is recorded
    /// when this source's activation comes due ([`IdentitySource::activate`]) and is
    /// `None` for as long as the source is still `CREATING`. Reading it off the record
    /// rather than recomputing it per request is what makes every read of one source
    /// report the same URL.
    pub approval_portal_url: Option<String>,
    pub status: IdentitySourceStatus,
    pub creation_time: SystemTime,
    /// Instant at which this source's activation (`CREATING` -> `ACTIVE`) becomes due.
    /// `None` once it has been materialised.
    pub activation_due: Option<SystemTime>,
    /// Instant at which this source's removal becomes due, set when
    /// `DeleteIdentitySource` starts it. `None` on a source whose deletion nobody has
    /// asked for.
    pub deletion_due: Option<SystemTime>,
    pub tags: BTreeMap<String, String>,
    /// Creation order, the stable ordering `ListIdentitySources` pages over.
    pub seq: u64,
}

impl IdentitySource {
    /// A freshly created identity source: `CREATING` until the clock owes its activation,
    /// or `None` when `now` names no instant this service could record that activation at.
    ///
    /// The instant is computed with CHECKED arithmetic, like every other clock-owed
    /// instant this module records: the calling handler answers a `None` with the server
    /// error it declares, so no source is ever stored waiting on an activation owed at no
    /// instant at all and no request can panic the handler here.
    #[must_use]
    pub fn creating(
        arn: String,
        source_type: IdentitySourceType,
        instance_arn: String,
        instance_region: String,
        tags: BTreeMap<String, String>,
        now: SystemTime,
        seq: u64,
    ) -> Option<Self> {
        Some(Self {
            arn,
            source_type,
            instance_arn,
            instance_region,
            // The association with the instance is not established yet, so neither is
            // its approval portal: `activate` records the portal's address when the
            // activation this create owes comes due.
            approval_portal_url: None,
            status: IdentitySourceStatus::Creating,
            creation_time: now,
            activation_due: Some(now.checked_add(IDENTITY_SOURCE_ACTIVATION_DELAY)?),
            deletion_due: None,
            tags,
            seq,
        })
    }

    /// Materialise the activation the clock owes this source: it becomes `ACTIVE`, and
    /// the approval portal of the IAM Identity Center instance it is associated with is
    /// recorded on the record ([`approval_portal_url`]), so every later read reports the
    /// same portal for this source.
    pub fn activate(&mut self) {
        self.status = IdentitySourceStatus::Active;
        self.activation_due = None;
        self.approval_portal_url =
            Some(approval_portal_url(&self.instance_arn, &self.instance_region));
    }

    /// Put this source into `DELETING` and record the instant its removal comes due
    /// ([`IDENTITY_SOURCE_DELETION_DELAY`]), which
    /// [`ComAmazonawsMpaAwsfluffycoreserviceState::materialize_due`] materialises for
    /// every later reader — which is also where the IN-USE rule is read again, so a
    /// source a stored team's approvers authenticate through when the removal comes due
    /// stays `DELETING` rather than being removed
    /// ([`ComAmazonawsMpaAwsfluffycoreserviceState::team_arn_using_identity_source`]).
    ///
    /// A source whose deletion has started is no longer waiting to become `ACTIVE`, so
    /// its activation is dropped: the clock owes exactly one transition from here, the
    /// removal. Idempotent — a repeat delete keeps the due instant the first one
    /// recorded, so asking twice does not postpone the removal.
    ///
    /// Answers whether this source is now `DELETING` with its removal scheduled. The
    /// instant is computed with CHECKED arithmetic, like every other clock-owed instant
    /// this module records, and BEFORE the record is touched: `false` says only that `now`
    /// names no instant the removal could be recorded at, and leaves the source exactly as
    /// it was for the calling handler to answer with the server error it declares.
    #[must_use]
    pub fn begin_deletion(&mut self, now: SystemTime) -> bool {
        if self.status == IdentitySourceStatus::Deleting {
            return true;
        }
        let Some(deletion_due) = now.checked_add(IDENTITY_SOURCE_DELETION_DELAY) else {
            return false;
        };
        self.status = IdentitySourceStatus::Deleting;
        self.activation_due = None;
        self.deletion_due = Some(deletion_due);
        true
    }
}

/// Status of an approval session, in the tokens `SessionStatus` declares
/// (`smithy_query(query="shape",shape="SessionStatus")`: `CREATING | PENDING | APPROVED |
/// CANCELLED | FAILED`).
///
/// `CREATING` is the non-terminal status a freshly started session is reported in and
/// `PENDING` the one it waits for its approvers in
/// ([`ComAmazonawsMpaAwsfluffycoreserviceState::materialize_due`] owes that transition).
/// `FAILED` is what a session that was never answered reaches when its expiration comes
/// due, `CANCELLED` is what `CancelSession` records ([`Session::cancel`]) and `APPROVED`
/// is what a session the clock owes an answer to reaches once the approvers give it
/// ([`Session::approve`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    Creating,
    Pending,
    Approved,
    Cancelled,
    Failed,
}

impl SessionStatus {
    /// The wire token, as `SessionStatus` spells it.
    #[must_use]
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Pending => "PENDING",
            Self::Approved => "APPROVED",
            Self::Cancelled => "CANCELLED",
            Self::Failed => "FAILED",
        }
    }

    /// Whether a session in this status is still waiting on its approvers, and so is
    /// neither completed nor cancelled.
    #[must_use]
    pub fn is_outstanding(self) -> bool {
        matches!(self, Self::Creating | Self::Pending)
    }
}

/// Status code of an approval session, in the tokens `SessionStatusCode` declares
/// (`REJECTED | EXPIRED | CONFIGURATION_CHANGED | ALL_APPROVERS_IN_SESSION`).
///
/// `EXPIRED` is the one a session this service records reaches on its own: it is what the
/// clock owes a session nobody answered before [`Session::expiration_time`]. The model
/// documents no prose for any of the four, so the rest are carried as the model's token
/// catalog and constructed by nothing until an operation records the condition each
/// stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatusCode {
    Rejected,
    Expired,
    ConfigurationChanged,
    AllApproversInSession,
}

impl SessionStatusCode {
    /// The wire token, as `SessionStatusCode` spells it.
    #[must_use]
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Rejected => "REJECTED",
            Self::Expired => "EXPIRED",
            Self::ConfigurationChanged => "CONFIGURATION_CHANGED",
            Self::AllApproversInSession => "ALL_APPROVERS_IN_SESSION",
        }
    }
}

/// Status of the protected operation a session guards, in the tokens
/// `SessionExecutionStatus` declares (`PENDING | EXECUTED | FAILED`).
///
/// `PENDING` is what a session reports for as long as the action it approves has not been
/// carried out, and `EXECUTED` what it reports once it has been
/// ([`Session::record_execution`]): every session this service records carries
/// `AUTO_COMPLETION_UPON_APPROVAL`, under which an approved action is executed on the
/// approval, so a session whose action the clock has carried out is a session whose
/// execution is done. `FAILED` stands for an execution that was attempted and did not
/// complete, which nothing here produces: an action this service never carried out is one
/// that is still waiting, not one that ran and failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionExecutionStatus {
    Pending,
    Executed,
    Failed,
}

impl SessionExecutionStatus {
    /// The wire token, as `SessionExecutionStatus` spells it.
    #[must_use]
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Executed => "EXECUTED",
            Self::Failed => "FAILED",
        }
    }
}

/// One approver's answer in a session, in the tokens `SessionResponse` declares
/// (`APPROVED | REJECTED | NO_RESPONSE`).
///
/// `NO_RESPONSE` is where every approver starts. No operation of this model records a
/// vote, so the answers this emulator records are the ones the CLOCK owes: the approval
/// an approver gives the change `UpdateApprovalTeam` put to them
/// ([`UPDATE_APPROVAL_DELAY`]) and the one they give the deletion
/// `StartActiveApprovalTeamDeletion` put to them ([`DELETION_APPROVAL_DELAY`],
/// [`Session::approve`]), the same way the clock owes their answer to an invitation.
/// `REJECTED` stands for a refusal nothing here produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionResponse {
    Approved,
    Rejected,
    NoResponse,
}

impl SessionResponse {
    /// The wire token, as `SessionResponse` spells it.
    #[must_use]
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Approved => "APPROVED",
            Self::Rejected => "REJECTED",
            Self::NoResponse => "NO_RESPONSE",
        }
    }
}

/// How the protected operation completes once a session is approved, in the tokens
/// `ActionCompletionStrategy` declares — today the single `AUTO_COMPLETION_UPON_APPROVAL`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionCompletionStrategy {
    AutoCompletionUponApproval,
}

impl ActionCompletionStrategy {
    /// The wire token, as `ActionCompletionStrategy` spells it.
    #[must_use]
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::AutoCompletionUponApproval => "AUTO_COMPLETION_UPON_APPROVAL",
        }
    }
}

/// An extra requirement a session places on its approvers, in the tokens
/// `AdditionalSecurityRequirement` declares — today the single
/// `APPROVER_VERIFICATION_REQUIRED`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdditionalSecurityRequirement {
    ApproverVerificationRequired,
}

impl AdditionalSecurityRequirement {
    /// The wire token, as `AdditionalSecurityRequirement` spells it.
    #[must_use]
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::ApproverVerificationRequired => "APPROVER_VERIFICATION_REQUIRED",
        }
    }
}

/// What a session was started FOR, which is what fixes the action it reports.
///
/// Three operations of this model put a request to a team's approvers:
/// `StartApprovalTeamBaseline` starts a baseline session, `UpdateApprovalTeam` puts the
/// change it was asked for to the team that has to approve it, and
/// `StartActiveApprovalTeamDeletion` puts the deletion of an active team to it — the
/// model documents that both an update and a deletion of an active team require the
/// team's approval; a session a protected operation of another service starts is not
/// reachable from this API at all. The kind is what [`SessionKind::action_name`] reports
/// as `ActionName` and what tells a team's outstanding BASELINE session from any other
/// session of the same team
/// ([`ComAmazonawsMpaAwsfluffycoreserviceState::pending_baseline_session_arn`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    Baseline,
    Update,
    Deletion,
}

impl SessionKind {
    /// The action this kind of session approves, as `ActionName` reports it: the MPA
    /// operation that starts it.
    #[must_use]
    pub fn action_name(self) -> &'static str {
        match self {
            Self::Baseline => "StartApprovalTeamBaseline",
            Self::Update => "UpdateApprovalTeam",
            Self::Deletion => "StartActiveApprovalTeamDeletion",
        }
    }
}

/// One approver taking part in a session, and their answer so far.
#[derive(Debug, Clone)]
pub struct SessionApprover {
    /// The `ApproverId` of the team member in the session — the same participant id
    /// [`Approver::approver_id`] carries, so a session names the team's own approvers.
    pub approver_id: String,
    pub identity_id: String,
    pub identity_source_arn: String,
    pub response: SessionResponse,
    pub response_time: Option<SystemTime>,
}

/// A stored approval session: everything `GetSession` reports.
#[derive(Debug, Clone)]
pub struct Session {
    pub arn: String,
    pub kind: SessionKind,
    /// The team whose approvers the session asks, and the name that team carries.
    pub approval_team_arn: String,
    pub approval_team_name: String,
    /// The team's approval strategy as it stood when the session started, so the
    /// threshold a session is judged against does not move under it.
    pub approval_strategy: StoredApprovalStrategy,
    pub approvers: Vec<SessionApprover>,
    /// The resource the approved action acts on.
    pub protected_resource_arn: String,
    pub status: SessionStatus,
    pub status_code: Option<SessionStatusCode>,
    pub status_message: Option<String>,
    pub execution_status: SessionExecutionStatus,
    pub action_completion_strategy: ActionCompletionStrategy,
    pub additional_security_requirements: Vec<AdditionalSecurityRequirement>,
    /// Service principal of the service the approved action belongs to.
    pub requester_service_principal: String,
    pub requester_principal_arn: String,
    pub requester_account_id: String,
    /// Region the request that started the session originated in, which need not be the
    /// region the team's (and the session's) own ARN carries.
    pub requester_region: String,
    pub requester_comment: Option<String>,
    pub description: Option<String>,
    pub metadata: BTreeMap<String, String>,
    pub initiation_time: SystemTime,
    pub expiration_time: SystemTime,
    /// Instant the session reached a terminal status, `None` while it is outstanding.
    pub completion_time: Option<SystemTime>,
    /// Instant at which this session's activation (`CREATING` -> `PENDING`) becomes due.
    /// `None` once it has been materialised.
    pub activation_due: Option<SystemTime>,
    /// Initiation order, the stable ordering a session listing pages over.
    pub seq: u64,
}

impl Session {
    /// A session a team's approvers are asked to answer, in the shape every session this
    /// store records starts in: `CREATING` until its activation comes due
    /// ([`SESSION_ACTIVATION_DELAY`]), nothing executed, and an expiration
    /// [`SESSION_EXPIRATION`] from `now`.
    ///
    /// The ONE place a session record is built, so the baseline session
    /// `StartApprovalTeamBaseline` starts and the update session `UpdateApprovalTeam`
    /// puts a requested change in cannot drift apart in any member but the two that
    /// differ: the [`SessionKind`] and the approvers the session asks. A session is
    /// minted FOR the team, so it takes the region off the team's own ARN rather than off
    /// the request; a team whose ARN carries no region segment the ARN grammar accepts
    /// falls back to `requester_region`, the region the request was signed for.
    ///
    /// Each caller keeps its own refusals, which the model fixes per operation: the
    /// store's session ceiling ([`MAX_SESSIONS`]) is reported with the error the calling
    /// operation declares for it, and `None` here says only that `now` names no instant
    /// this service could record the session's expiration at.
    #[must_use]
    pub fn for_team(
        kind: SessionKind,
        team: &ApprovalTeam,
        approvers: Vec<SessionApprover>,
        requester_region: String,
        now: SystemTime,
        seq: u64,
    ) -> Option<Self> {
        let region =
            arn_region(&team.arn).map_or_else(|| requester_region.clone(), ToString::to_string);
        Some(Self {
            arn: session_arn(&region, &team.name, seq),
            kind,
            approval_team_arn: team.arn.clone(),
            approval_team_name: team.name.clone(),
            // The threshold a session is judged against is the team's as it stood when
            // the session started, so it does not move under the session.
            approval_strategy: team.approval_strategy,
            approvers,
            // The resource the approved action acts on is the approval team itself.
            protected_resource_arn: team.arn.clone(),
            // A session is reported `CREATING` until its activation comes due, the
            // non-terminal status `SessionStatus` declares for one.
            status: SessionStatus::Creating,
            status_code: None,
            status_message: None,
            // Nothing has been carried out when a session is started: the action it
            // approves is waiting on the approvers, so its execution is `PENDING` until
            // that action is carried out ([`Session::record_execution`]).
            execution_status: SessionExecutionStatus::Pending,
            // The only strategy `ActionCompletionStrategy` declares.
            action_completion_strategy: ActionCompletionStrategy::AutoCompletionUponApproval,
            // No request that starts a session in this model names an additional security
            // requirement, so a session applies none.
            additional_security_requirements: Vec::new(),
            requester_service_principal: SERVICE_PRINCIPAL.to_string(),
            requester_principal_arn: caller_identity_arn(),
            requester_account_id: caller_account_id().to_string(),
            requester_region,
            // `RequesterComment` and `Description` are what a requester supplies, and no
            // input that starts a session in this model carries either.
            requester_comment: None,
            description: None,
            metadata: BTreeMap::new(),
            initiation_time: now,
            expiration_time: now.checked_add(SESSION_EXPIRATION)?,
            completion_time: None,
            activation_due: now.checked_add(SESSION_ACTIVATION_DELAY),
            seq,
        })
    }

    /// Total number of approvers in the session, as `NumberOfApprovers` reports it.
    #[must_use]
    pub fn number_of_approvers(&self) -> i32 {
        i32::try_from(self.approvers.len()).unwrap_or(i32::MAX)
    }

    /// End this session in the terminal `CANCELLED` status, completed at `now`.
    ///
    /// IDEMPOTENT: a session already `CANCELLED` keeps the completion instant the first
    /// call recorded, so cancelling twice reports one and the same record.
    ///
    /// No `SessionStatusCode` token stands for a cancellation (the four the model
    /// declares are `REJECTED | EXPIRED | CONFIGURATION_CHANGED |
    /// ALL_APPROVERS_IN_SESSION`), so a cancelled session reports no status code, and no
    /// approver is claimed to have answered: every response stays as it stood.
    pub fn cancel(&mut self, now: SystemTime) {
        if self.status == SessionStatus::Cancelled {
            return;
        }
        self.status = SessionStatus::Cancelled;
        self.status_code = None;
        self.completion_time = Some(now);
        // A cancelled session is over, so the activation the clock still owed one that
        // was `CREATING` is dropped: nothing may move it out of a terminal status.
        self.activation_due = None;
    }

    /// Whether `approver_id` is one of the approvers this session asks.
    #[must_use]
    pub fn includes_approver(&self, approver_id: &str) -> bool {
        self.approvers
            .iter()
            .any(|approver| approver.approver_id == approver_id)
    }

    /// End this session in the terminal `APPROVED` status, completed at `at`, and answer
    /// with the `ApproverId` of every approver whose approval it carries.
    ///
    /// The threshold the session was judged against is its own
    /// ([`Session::approval_strategy`]), so exactly the minimum number of approvals that
    /// strategy requires is recorded, in the order the session lists its approvers: the
    /// rest of the approvers were not needed and are not claimed to have answered.
    ///
    /// IDEMPOTENT: a session already `APPROVED` keeps the answers and the completion
    /// instant the first call recorded.
    pub fn approve(&mut self, at: SystemTime) -> Vec<String> {
        if self.status != SessionStatus::Approved {
            let required = usize::try_from(self.approval_strategy.min_approvals_required())
                .unwrap_or(0)
                .min(self.approvers.len());
            for approver in self.approvers.iter_mut().take(required) {
                approver.response = SessionResponse::Approved;
                approver.response_time = Some(at);
            }
            self.status = SessionStatus::Approved;
            self.status_code = None;
            self.completion_time = Some(at);
            // An approved session is over, so the activation the clock still owed one that
            // was `CREATING` is dropped: nothing may move it out of a terminal status.
            self.activation_due = None;
        }
        self.approvers
            .iter()
            .filter(|approver| approver.response == SessionResponse::Approved)
            .map(|approver| approver.approver_id.clone())
            .collect()
    }

    /// Record that the action this session approves HAS BEEN CARRIED OUT, which is what
    /// `ExecutionStatus` reports: `EXECUTED` from here, where it was `PENDING` while the
    /// action was still waiting to run.
    ///
    /// Every session this service records carries
    /// [`ActionCompletionStrategy::AutoCompletionUponApproval`], under which the approved
    /// action is executed on the approval, so this is recorded at the instant the action
    /// the session names really took effect on this store — never on the approval alone,
    /// which is the session's `Status`, and never because the session was read.
    ///
    /// IDEMPOTENT: an action is carried out once, so a repeat call reports the same thing.
    pub fn record_execution(&mut self) {
        self.execution_status = SessionExecutionStatus::Executed;
    }
}

/// The key a `ClientToken` is recorded under in
/// [`ComAmazonawsMpaAwsfluffycoreserviceState::idempotency_tokens`].
///
/// AWS scopes an `@idempotencyToken` to the create it was given to, so the key carries
/// the KIND of resource the token minted — the resource name of that kind's ARN
/// (`approval-team`, `identity-source`). Without it one map keyed by the token alone
/// would let a token reused across two creates resolve to the other kind's resource and
/// overwrite its record.
#[must_use]
pub fn idempotency_key(resource_kind: &str, token: &str) -> String {
    format!("{resource_kind}/{token}")
}

/// The ANSWER the first call of an `@idempotencyToken` create returned, recorded under
/// that token's [`idempotency_key`].
///
/// A retry the token identifies repeats that answer; it does not re-render one out of the
/// resource as it stands now. The whole point of the token is that a client which never
/// saw the first answer can ask again and be told the same thing, so a member whose
/// stored source a LATER call moves may not drift between two answers to the same
/// request.
///
/// A member fixed when the resource is minted (its ARN, its name, its type, the instant
/// it was created) is still read back out of the stored record, so a replay can only ever
/// describe a resource this store really holds — and an entry whose resource is gone is
/// dropped, so the replay mints a new one instead. The members a later call CAN move are
/// the ones recorded here.
#[derive(Clone, Debug)]
pub struct IdempotentCreate {
    /// ARN of the resource the first call minted: what the replay resolves the stored
    /// record through, and what frees this entry when that resource is removed.
    pub arn: String,
    /// The version id the first call answered with, for a created resource that carries
    /// one. An approval team's live version id MOVES — `UpdateApprovalTeam` applies a
    /// change in place on a team no approver has activated yet, and installs an approved
    /// change's version on an active one — so the version the stored team carries is not
    /// the version the create returned. `None` for a create whose answer names no
    /// version.
    pub version_id: Option<String>,
}

/// Resource kind of an approval team, as [`idempotency_key`] names it — the resource
/// name `ApprovalTeamArn`'s pattern carries.
pub const APPROVAL_TEAM_RESOURCE: &str = "approval-team";

/// Resource kind of an identity source, as [`idempotency_key`] names it — the resource
/// name the `IdentitySource` resource's `arn` template carries.
pub const IDENTITY_SOURCE_RESOURCE: &str = "identity-source";

/// Resource kind of an approval session — the resource name `SessionArn`'s pattern
/// carries (`…:session/<team>/<id>`).
pub const SESSION_RESOURCE: &str = "session";

/// ARN of an approval session, in the form `SessionArn`'s pattern fixes
/// (`^arn:aws(-[^:]+)?:mpa:[a-z0-9-]{1,20}:[0-9]{12}:session/[a-zA-Z0-9._-]+/[a-zA-Z0-9_-]+$`):
/// the approval team's name, then the session's own id.
///
/// The id carries no modelled form beyond `[a-zA-Z0-9_-]+`, so its shape is this
/// emulator's: the store's session counter, zero-padded, so an id is never reused and
/// ARN order and initiation order agree.
#[must_use]
pub fn session_arn(region: &str, team_name: &str, seq: u64) -> String {
    ::engine::emulator_arn(
        "mpa",
        region,
        &format!("{SESSION_RESOURCE}/{team_name}/session-{seq:012}"),
    )
}

/// The prefix every identity-source id this service mints carries: an identity-source id
/// is ONE token of this service's own, not a caller-supplied name, and
/// [`identity_source_arn`] is where it is written.
pub const IDENTITY_SOURCE_ID_PREFIX: &str = "ids-";

/// ARN of an identity source, in the form the `IdentitySource` resource's own `arn`
/// template fixes (`identity-source/{IdentitySourceArn}`):
/// `arn:aws:mpa:<region>:<account>:identity-source/<id>`.
///
/// The identifier carries no modelled pattern, so its form is this emulator's: the
/// [`IDENTITY_SOURCE_ID_PREFIX`] prefix and then the store's creation counter,
/// zero-padded, so an id is never reused and ARN order and creation order agree. This is
/// the one place that form is written, and [`is_identity_source_id`] is the one place it
/// is read, so what this service mints and what it accepts cannot drift apart.
#[must_use]
pub fn identity_source_arn(region: &str, seq: u64) -> String {
    ::engine::emulator_arn(
        "mpa",
        region,
        &format!("{IDENTITY_SOURCE_RESOURCE}/{IDENTITY_SOURCE_ID_PREFIX}{seq:012}"),
    )
}

/// Whether `arn` is an ARN of this service's identity-source resource
/// ([`is_service_resource_arn`] for [`IDENTITY_SOURCE_RESOURCE`]) — the GRAMMAR an
/// `IdentitySourceArn` takes, with no claim about the form of its resource id.
///
/// A value that is not one names no identity source whatever this store holds, so a member
/// that should carry one is refused rather than resolved: `PrimaryIdentitySourceArn`, which
/// REFERENCES the identity source an approver authenticates through, with the service-wide
/// malformed-ARN sentence ([`malformed_approver_identity_source`]). The two operations
/// ADDRESSED BY an identity source's ARN screen the id as well
/// ([`addresses_identity_source`]).
#[must_use]
pub fn is_identity_source_arn(arn: &str) -> bool {
    is_service_resource_arn(arn, IDENTITY_SOURCE_RESOURCE)
}

/// Whether `id` is an identity-source id of this service: the
/// [`IDENTITY_SOURCE_ID_PREFIX`] prefix and then a non-empty body of letters and digits
/// — the one token [`identity_source_arn`] mints, with no second separator in it.
///
/// An identity-source id is MINTED BY THIS SERVICE (nothing in any request names one), so
/// a value outside this form is not an id this service could ever have issued: an
/// otherwise well-formed identity-source ARN carrying one (a bare UUID, a word, a dotted
/// name) addresses no identity source and is refused as malformed rather than looked up.
#[must_use]
pub fn is_identity_source_id(id: &str) -> bool {
    id.strip_prefix(IDENTITY_SOURCE_ID_PREFIX).is_some_and(|body| {
        !body.is_empty() && body.bytes().all(|byte| byte.is_ascii_alphanumeric())
    })
}

/// Whether `arn` ADDRESSES an identity source of this service: an identity-source ARN of
/// this service ([`is_identity_source_arn`]) whose resource id is an identity-source id of
/// this service ([`is_identity_source_id`]).
///
/// The screen both operations that take an `IdentitySourceArn` in their URI label apply,
/// so `GetIdentitySource` and `DeleteIdentitySource` cannot disagree about which values
/// name an identity source at all. A value this answers `false` for is refused with
/// [`invalid_identity_source_arn`] (400) before the account boundary and before any
/// lookup.
#[must_use]
pub fn addresses_identity_source(arn: &str) -> bool {
    is_identity_source_arn(arn) && arn_resource_id(arn).is_some_and(is_identity_source_id)
}

/// The id an ARN's resource field carries after the resource kind: the segment after its
/// last `/`, or `None` for a resource field that carries no `/` at all.
fn arn_resource_id(arn: &str) -> Option<&str> {
    arn.splitn(6, ':')
        .nth(5)?
        .rsplit_once('/')
        .map(|(_, id)| id)
}

/// The id of the IAM Identity Center instance an `IdcInstanceArn` names: the segment
/// after `instance/`, the only resource segment the modelled pattern
/// `^arn:.+:sso:::instance/(?:sso)?ins-[a-zA-Z0-9-.]{16}$` allows.
fn idc_instance_id(instance_arn: &str) -> &str {
    instance_arn
        .rsplit_once('/')
        .map_or(instance_arn, |(_, id)| id)
}

/// The DNS suffix hostnames take in the partition an ARN names: `amazonaws.com`, except
/// in the China partition, where every AWS hostname ends in `amazonaws.com.cn`.
fn partition_dns_suffix(arn: &str) -> &'static str {
    match arn.split(':').nth(1) {
        Some("aws-cn") => "amazonaws.com.cn",
        _ => "amazonaws.com",
    }
}

/// The id of an IAM Identity Center instance as ONE hostname label: lowercased, with the
/// dots its id may carry turned into hyphens — a hostname label carries only letters,
/// digits and hyphens — and with no trailing hyphen.
fn instance_host_label(instance_arn: &str) -> String {
    let mut label = idc_instance_id(instance_arn)
        .to_ascii_lowercase()
        .replace('.', "-");
    let keep = label.trim_end_matches('-').len();
    if keep > 0 {
        label.truncate(keep);
    }
    label
}

/// URL of the approval portal associated with an IAM Identity Center instance, as
/// `ApprovalPortalUrl` reports it for an identity source.
///
/// The portal belongs to the INSTANCE, so its address is derived from the instance's own
/// id and from the region that instance is in — never from the request that reads it —
/// and is recorded on the identity-source record
/// ([`IdentitySource::approval_portal_url`]) so that every read of one source reports
/// one portal. The host is that instance's id under this service's own regional
/// hostname (`mpa.<region>.<partition dns suffix>`, the host this service's endpoint
/// rules resolve to), which keeps one portal per instance per region and stays inside
/// the DNS namespace this service owns.
#[must_use]
pub fn approval_portal_url(instance_arn: &str, instance_region: &str) -> String {
    // `IamIdentityCenter$Region` carries no modelled pattern, so a region segment is
    // only placed in the host when the stored instance names one: an empty one would
    // otherwise leave an empty label in the hostname.
    let region = if instance_region.is_empty() {
        String::new()
    } else {
        format!("{instance_region}.")
    };
    format!(
        "https://{}.mpa.{region}{}",
        instance_host_label(instance_arn),
        partition_dns_suffix(instance_arn)
    )
}

/// Longest category or name segment a policy ARN may carry, from the
/// `[a-zA-Z0-9_\.-]{1,1023}` segments `UnqualifiedPolicyArn`'s modelled pattern declares.
const MAX_POLICY_ARN_SEGMENT: usize = 1023;

/// The version segment that stands for "whichever version is this policy's default" —
/// the alternative `QualifiedPolicyArn`'s pattern allows beside a decimal version id
/// (`^arn:.{1,63}:mpa:::aws:policy/[a-zA-Z0-9_\.-]{1,1023}/[a-zA-Z0-9_\.-]{1,1023}/(?:[\d]+|\$DEFAULT)$`).
pub const DEFAULT_POLICY_VERSION: &str = "$DEFAULT";

/// Type of a policy, in the tokens `PolicyType` declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyType {
    AwsManaged,
    AwsRam,
}

impl PolicyType {
    /// The wire token, as `PolicyType` spells it.
    #[must_use]
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::AwsManaged => "AWS_MANAGED",
            Self::AwsRam => "AWS_RAM",
        }
    }
    /// The policy type `token` names, or `None` when `PolicyType` declares no such
    /// value.
    #[must_use]
    pub fn from_wire(token: &str) -> Option<Self> {
        match token {
            "AWS_MANAGED" => Some(Self::AwsManaged),
            "AWS_RAM" => Some(Self::AwsRam),
            _ => None,
        }
    }
}

/// Status of one version of a policy, in the tokens `PolicyStatus` declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyStatus {
    Attachable,
    Deprecated,
}

impl PolicyStatus {
    /// Whether a version in this status may still be attached to a team — the question
    /// this field answers, and the one the attach gate
    /// ([`ComAmazonawsMpaAwsfluffycoreserviceState::unattachable_policy_reference`]) asks
    /// it.
    #[must_use]
    pub fn is_attachable(self) -> bool {
        match self {
            Self::Attachable => true,
            Self::Deprecated => false,
        }
    }

    /// The wire token, as `PolicyStatus` spells it.
    #[must_use]
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Attachable => "ATTACHABLE",
            Self::Deprecated => "DEPRECATED",
        }
    }
}

/// The version of a policy a `QualifiedPolicyArn` names: an explicit
/// `PolicyVersionId`, or the policy's default version — the `$DEFAULT` alternative the
/// pattern allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyVersionSelector {
    Default,
    Id(i32),
}

/// The policy and version a `QualifiedPolicyArn` names, split into the unqualified ARN
/// the catalog is keyed by ([`policy_arn`]) and the version it selects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyVersionRef {
    pub policy_arn: String,
    pub version: PolicyVersionSelector,
}

/// Whether `segment` fits the `[a-zA-Z0-9_\.-]{1,1023}` category/name segment
/// `UnqualifiedPolicyArn`'s pattern declares.
fn is_policy_arn_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment.len() <= MAX_POLICY_ARN_SEGMENT
        && segment
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-')
}

/// Whether `arn` is an unqualified policy ARN in the form `UnqualifiedPolicyArn`'s
/// pattern fixes — the form [`policy_arn`] mints and the catalog is keyed by:
/// `arn:<partition>:mpa:::aws:policy/<Category>/<Name>`.
#[must_use]
pub fn is_policy_arn(arn: &str) -> bool {
    // `arn:<partition>:<service>:<region>:<account>:<resource>`, the resource being
    // `aws:policy/<Category>/<Name>` — which carries a colon of its own, so the ARN
    // splits into SIX fields and no more.
    let mut fields = arn.splitn(6, ':');
    if fields.next() != Some("arn") {
        return false;
    }
    // `.{1,63}` in the pattern's partition position.
    if !fields
        .next()
        .is_some_and(|partition| !partition.is_empty() && partition.len() <= 63)
    {
        return false;
    }
    if fields.next() != Some(SERVICE_ACTION_PREFIX) {
        return false;
    }
    // A policy is an AWS-owned, global resource: `:::` in the pattern, so its region
    // and account fields are both empty.
    if fields.next() != Some("") || fields.next() != Some("") {
        return false;
    }
    let Some(path) = fields.next().and_then(|r| r.strip_prefix("aws:policy/")) else {
        return false;
    };
    let mut segments = path.split('/');
    let (Some(category), Some(name), None) = (segments.next(), segments.next(), segments.next())
    else {
        return false;
    };
    is_policy_arn_segment(category) && is_policy_arn_segment(name)
}

/// The policy and version `arn` names, or `None` when it is not a policy-version ARN in
/// the grammar `QualifiedPolicyArn`'s pattern fixes (an unqualified policy ARN followed
/// by `/<PolicyVersionId>` or `/$DEFAULT`).
///
/// The inverse of [`policy_version_arn`], so what this service mints it also reads back.
#[must_use]
pub fn parse_policy_version_arn(arn: &str) -> Option<PolicyVersionRef> {
    let (policy_arn, version) = arn.rsplit_once('/')?;
    if !is_policy_arn(policy_arn) {
        return None;
    }
    let version = if version == DEFAULT_POLICY_VERSION {
        PolicyVersionSelector::Default
    } else {
        // `[\d]+` in the pattern, and `PolicyVersionId` is `range(min: 1)`, so a signed
        // or out-of-range value names no version.
        if !version.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let version_id: i32 = version.parse().ok()?;
        if version_id < FIRST_POLICY_VERSION {
            return None;
        }
        PolicyVersionSelector::Id(version_id)
    };
    Some(PolicyVersionRef {
        policy_arn: policy_arn.to_string(),
        version,
    })
}

/// A policy attached to a resource this store records, as the resource-policy reads
/// report it. Every member is read off the catalog entry the attachment resolves to, so
/// a reported resource policy can never disagree with the policy the catalog holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourcePolicy {
    /// ARN of the resource the policy is attached to.
    pub resource_arn: String,
    /// `UnqualifiedPolicyArn` of the policy itself.
    pub policy_arn: String,
    /// ARN of the version in effect — the version the attachment RESOLVES to, so an
    /// attachment of `$DEFAULT` reports the policy's default version by id.
    pub policy_version_arn: String,
    /// Id of that version.
    pub policy_version_id: i32,
    pub policy_name: String,
    pub policy_type: PolicyType,
    /// Document of the version in effect.
    pub document: String,
}

/// One version of a catalog policy: everything `ListPolicyVersions` reports for it that
/// is the VERSION's rather than the policy's.
///
/// The members a version shares with its policy — the policy ARN, the name and the
/// `PolicyType` — are not copied here: they live once on the owning
/// [`CatalogPolicy`], so a reader cannot report a version whose name disagrees with
/// its policy's.
#[derive(Debug, Clone)]
pub struct CatalogPolicyVersion {
    /// This version's id, as `PolicyVersionSummary$VersionId` reports it. At least
    /// [`FIRST_POLICY_VERSION`], from `PolicyVersionId`'s modelled `range(min: 1)`.
    pub version_id: i32,
    /// Whether this version may still be attached to a team.
    pub status: PolicyStatus,
    pub creation_time: SystemTime,
    pub last_updated_time: SystemTime,
    /// This version's document, as `PolicyVersion$Document` reports it — the text the
    /// catalog holds for it, which is what every reader of this version answers with.
    /// A version's document belongs to the VERSION, so each one carries its own and no
    /// reader synthesizes one.
    pub document: String,
}

/// One policy in the catalog: everything `ListPolicies` reports for it, plus the
/// versions `ListPolicyVersions` reports.
#[derive(Debug, Clone)]
pub struct CatalogPolicy {
    /// The policy's unqualified ARN, in the form [`policy_arn`] mints.
    pub arn: String,
    /// The policy's own DISPLAY name, as `Name` reports it on every catalog read.
    /// `PolicyName` is `length(max: 64)` and carries no pattern, so a display name is
    /// independent of the `[a-zA-Z0-9_\.-]` name segment of the ARN — it may hold a
    /// space, which that segment pattern forbids — and is therefore recorded here
    /// rather than read back out of the ARN.
    pub name: String,
    pub policy_type: PolicyType,
    /// Version id that is this policy's default, as `Policy$DefaultVersion` reports it.
    pub default_version: i32,
    /// Every version of this policy, keyed by its version id — the stable, ascending
    /// ordering `ListPolicyVersions` pages over.
    pub versions: BTreeMap<i32, CatalogPolicyVersion>,
}

impl CatalogPolicy {
    /// A catalog policy holding one version, [`FIRST_POLICY_VERSION`], which is
    /// therefore also its default, carrying `document` as that version's document.
    ///
    /// `name` is both the ARN's name segment and the policy's display name, which is
    /// what a policy whose display name carries no character `UnqualifiedPolicyArn`'s
    /// segment pattern forbids looks like. An entry whose display name differs from its
    /// ARN segment is built from the two values it holds ([`SeededPolicy`]).
    #[must_use]
    pub fn with_first_version(
        category: &str,
        name: &str,
        policy_type: PolicyType,
        created: SystemTime,
        document: &str,
    ) -> Self {
        let mut policy = Self {
            arn: policy_arn(category, name),
            name: name.to_string(),
            policy_type,
            default_version: FIRST_POLICY_VERSION,
            versions: BTreeMap::new(),
        };
        policy.insert_version(CatalogPolicyVersion {
            version_id: FIRST_POLICY_VERSION,
            status: PolicyStatus::Attachable,
            creation_time: created,
            last_updated_time: created,
            document: document.to_string(),
        });
        policy
    }

    /// Record a version of this policy, replacing any entry under the same id.
    pub fn insert_version(&mut self, version: CatalogPolicyVersion) {
        self.versions.insert(version.version_id, version);
    }

    /// This policy's version with id `version_id`.
    #[must_use]
    pub fn version(&self, version_id: i32) -> Option<&CatalogPolicyVersion> {
        self.versions.get(&version_id)
    }

    /// Whether `version_id` names the version this policy attaches by default.
    #[must_use]
    pub fn is_default_version(&self, version_id: i32) -> bool {
        version_id == self.default_version
    }

    /// The qualified ARN of this policy's version `version_id`.
    #[must_use]
    pub fn version_arn(&self, version_id: i32) -> String {
        policy_version_arn(&self.arn, version_id)
    }
}

/// Version id a policy's first version carries, from `PolicyVersionId`'s modelled
/// `range(min: 1)`.
pub const FIRST_POLICY_VERSION: i32 = 1;

/// ARN of a policy, in the form `UnqualifiedPolicyArn`'s pattern fixes:
/// `arn:aws:mpa:::aws:policy/<category>/<name>`.
///
/// A policy is an AWS-owned resource, so the ARN's region and account fields are EMPTY —
/// which is why `engine::emulator_arn`, which stamps this emulator's region and account
/// into both, cannot mint one and this builds the ARN itself.
#[must_use]
pub fn policy_arn(category: &str, name: &str) -> String {
    format!("arn:aws:mpa:::aws:policy/{category}/{name}")
}

/// ARN of ONE VERSION of a policy, in the form `QualifiedPolicyArn`'s pattern fixes:
/// the policy's own unqualified ARN followed by the version id
/// (`arn:aws:mpa:::aws:policy/<category>/<name>/<version id>`).
#[must_use]
pub fn policy_version_arn(policy_arn: &str, version_id: i32) -> String {
    format!("{policy_arn}/{version_id}")
}

/// One policy of the catalog this service publishes, with everything a read of it
/// reports that the entry itself owns.
///
/// A published policy is not the caller's and not this store's: no operation of this
/// model mints, updates or deletes one, and nothing about it is derived from the request
/// that reads it or from the instant this store came into being. So each entry carries
/// its own identity (the two segments its ARN is built from, plus the display name), its
/// `PolicyType`, and the one version it publishes — that version's status, its document
/// and the instant the catalog records for it.
#[derive(Debug, Clone, Copy)]
struct SeededPolicy {
    /// The CATEGORY segment of the ARN, from `UnqualifiedPolicyArn`'s pattern
    /// (`.../aws:policy/<category>/<name>`).
    category: &'static str,
    /// The NAME segment of the ARN, restricted to `[a-zA-Z0-9_\.-]` by that same
    /// pattern.
    arn_name: &'static str,
    /// The policy's DISPLAY name, as `Name` reports it: a `PolicyName`
    /// (`length(max: 64)`, no pattern), which is a different string from the ARN's name
    /// segment whenever it carries a character — a space, here — the ARN cannot.
    name: &'static str,
    policy_type: PolicyType,
    /// Status of the published version, from `PolicyStatus`' own tokens.
    status: PolicyStatus,
    /// The published version's document, as `PolicyVersion$Document` reports it:
    /// published text, handed to a reader unchanged (whitespace included).
    document: &'static str,
    /// Epoch seconds of the instant the catalog records as the published version's
    /// creation — and, since nothing updates a published version, as its last update
    /// too. It belongs to the ENTRY, so every read of it in every run reports the one
    /// instant, which is what makes a catalog read repeatable.
    published_at_epoch_secs: u64,
}

impl SeededPolicy {
    /// This entry as the catalog holds it: the policy, holding the one version
    /// [`FIRST_POLICY_VERSION`] names, which is therefore also its default.
    fn catalog_policy(self) -> CatalogPolicy {
        let published = SystemTime::UNIX_EPOCH + Duration::from_secs(self.published_at_epoch_secs);
        let mut policy = CatalogPolicy {
            arn: policy_arn(self.category, self.arn_name),
            name: self.name.to_string(),
            policy_type: self.policy_type,
            default_version: FIRST_POLICY_VERSION,
            versions: BTreeMap::new(),
        };
        policy.insert_version(CatalogPolicyVersion {
            version_id: FIRST_POLICY_VERSION,
            status: self.status,
            creation_time: published,
            last_updated_time: published,
            document: self.document.to_string(),
        });
        policy
    }
}

/// The document the published vault-protection policy's one version carries: approval
/// sessions for the protected operation alone.
///
/// A managed policy's document is the CATALOG's own PUBLISHED TEXT — not a value this
/// service derives per read and not a document this crate reflows — so it is held here
/// verbatim, byte for byte as the catalog publishes it: the same `Sid`, the same
/// `Principal` element, the same member order and the same layout (two-space indentation,
/// one member per line, a trailing newline). A reader hands this string on unchanged, so a
/// byte-level consumer (a hash, a signature, a diff against the published policy) sees
/// what it would see from the published catalog.
///
/// What it says: the two session actions a team's approvers exercise — and no other
/// action, never a service-wide wildcard — are allowed to any principal, scoped by the
/// condition keys this service's own model defines for them (`mpa:RequestedOperation`, the
/// operation that needs the team's approval, and `mpa:ProtectedResourceAccount`, the
/// account owning the resource it targets) and only for a request the protected service
/// makes on the caller's behalf (`aws:ViaAWSService`, a boolean key, hence IAM's `Bool`
/// operator).
///
/// The literal is laid out at column zero because its OWN indentation is part of the
/// published text.
const VAULT_PROTECTION_POLICY_DOCUMENT: &str = r#"{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Sid": "VaultOwnerPermissions",
      "Effect": "Allow",
      "Principal": { "AWS": "*" },
      "Resource": "*",
      "Action": [ "mpa:StartSession", "mpa:CancelSession" ],
      "Condition": {
        "StringEquals": {
          "mpa:RequestedOperation": "backup:RevokeRestoreAccessBackupVault",
          "mpa:ProtectedResourceAccount": "${aws:PrincipalAccount}"
        },
        "Bool": { "aws:ViaAWSService": "true" }
      }
    }
  ]
}
"#;

/// The policies this store holds when it is first touched: the catalog this service
/// publishes.
///
/// The model fixes the SHAPE of an entry, and that is what this emulator computes its
/// answers from: the ARN form comes from `UnqualifiedPolicyArn`'s pattern (whose two
/// `[a-zA-Z0-9_\.-]` segments are the category and the name), the display name from
/// `PolicyName`, the token from `PolicyType`'s enum, the status from `PolicyStatus`' and
/// the version from `PolicyVersionId`'s `range(min: 1)`. The entries themselves are the
/// published catalog, which no request can change and no model member enumerates.
/// Nothing is keyed to a particular entry — `ListPolicies` pages over whatever the store
/// holds, `ListPolicyVersions` over whatever versions an entry holds and
/// `GetPolicyVersion` reads one of them — so an entry or a version recorded later is
/// reported exactly like a seeded one, and one more published policy is one more row
/// here.
const SEEDED_POLICY_CATALOG: &[SeededPolicy] = &[SeededPolicy {
    category: "IdentityCenter",
    arn_name: "MPAApprovalTeamPolicy",
    name: "AWS Backup LAG Vault Protection Policy",
    policy_type: PolicyType::AwsManaged,
    status: PolicyStatus::Attachable,
    document: VAULT_PROTECTION_POLICY_DOCUMENT,
    // 2025-06-17T00:00:00Z, the instant the catalog records for this version.
    published_at_epoch_secs: 1_750_118_400,
}];

/// The catalog a fresh store starts with, keyed by ARN.
///
/// Takes no instant: a published policy was created when the catalog says it was, not
/// when this store came into being, so each entry's version carries its own recorded
/// instant ([`SeededPolicy::published_at_epoch_secs`]) and two stores answer alike.
#[must_use]
pub fn seeded_policy_catalog() -> BTreeMap<String, CatalogPolicy> {
    SEEDED_POLICY_CATALOG
        .iter()
        .map(|seed| {
            let policy = seed.catalog_policy();
            (policy.arn.clone(), policy)
        })
        .collect()
}

/// Per-service state for com-amazonaws-mpa-awsfluffycoreservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsMpaAwsfluffycoreserviceState {
    /// Every live team, keyed by its ARN.
    pub approval_teams: BTreeMap<String, ApprovalTeam>,
    /// Team name -> ARN. Backs the `ConflictException` a duplicate name earns.
    pub name_index: BTreeMap<String, String>,
    /// `ClientToken` -> the answer the create it was given to returned
    /// ([`IdempotentCreate`]). Backs the idempotent replay of the `@idempotencyToken`
    /// creates (`CreateApprovalTeam`, `CreateIdentitySource`). An `@idempotencyToken` is
    /// scoped to the resource the request creates, so an entry is keyed through
    /// [`idempotency_key`] and the same token used on two different creates replays each
    /// against its own resource instead of overwriting the other's record.
    pub idempotency_tokens: BTreeMap<String, IdempotentCreate>,
    /// The account that is the MANAGEMENT account of the organization this store
    /// records, if any.
    ///
    /// Multi-party approval is an AWS Organizations feature, and the operations
    /// that set an organization's approval teams up are restricted to its
    /// management account — a call from a member or standalone account is refused
    /// with `AccessDeniedException` (403). This emulator presents a single account
    /// ([`caller_account_id`]) and runs no Organizations control plane, so there is
    /// no organization and no management account to record: the field is `None` and
    /// every caller is a standalone account. Read it through
    /// [`ComAmazonawsMpaAwsfluffycoreserviceState::is_management_account`], which
    /// therefore answers `false` for every caller until something records one.
    pub organization_management_account_id: Option<String>,
    /// The policy catalog, keyed by each policy's ARN — the stable ordering
    /// `ListPolicies` pages over.
    ///
    /// Seeded by [`Default`] from [`seeded_policy_catalog`]: a policy is AWS's, so no
    /// operation in this model mints or removes one or one of its versions, and every
    /// reader — `ListPolicies` and `ListPolicyVersions` alike — answers out of this one
    /// map.
    pub policies: BTreeMap<String, CatalogPolicy>,
    /// Every identity source, keyed by its ARN. `CreateIdentitySource` records one here
    /// and `ListIdentitySources` reports them.
    pub identity_sources: BTreeMap<String, IdentitySource>,
    /// Every approval session, keyed by its ARN. `StartApprovalTeamBaseline` records one
    /// here and `GetSession` reports it, so both read one and the same record.
    pub sessions: BTreeMap<String, Session>,
    /// Source of [`ApprovalTeam::seq`].
    next_team_seq: u64,
    /// Source of [`Approver::approver_id`].
    next_approver_seq: u64,
    /// Source of [`IdentitySource::seq`] and of the id its ARN carries.
    next_identity_source_seq: u64,
    /// Source of [`Session::seq`] and of the id its ARN carries.
    next_session_seq: u64,
}

impl Default for ComAmazonawsMpaAwsfluffycoreserviceState {
    /// A fresh store: no team, no organization, and the policy catalog seeded — the
    /// engine builds this service's state with `Default` on first touch, so this is the
    /// only place the catalog comes into being.
    fn default() -> Self {
        Self {
            approval_teams: BTreeMap::new(),
            name_index: BTreeMap::new(),
            idempotency_tokens: BTreeMap::new(),
            organization_management_account_id: None,
            policies: seeded_policy_catalog(),
            identity_sources: BTreeMap::new(),
            sessions: BTreeMap::new(),
            next_team_seq: 0,
            next_approver_seq: 0,
            next_identity_source_seq: 0,
            next_session_seq: 0,
        }
    }
}

impl ComAmazonawsMpaAwsfluffycoreserviceState {
    /// Whether `account_id` is the management account of the organization this store
    /// records. False whenever no management account is recorded, which is the only
    /// state this emulator reaches on its own: an organization-only operation is then
    /// available to nobody.
    #[must_use]
    pub fn is_management_account(&self, account_id: &str) -> bool {
        self.organization_management_account_id.as_deref() == Some(account_id)
    }

    /// Apply every change the clock already owes at `now`, so each handler reads a
    /// store that is current. Idempotent, and a no-op when nothing is due.
    ///
    /// Only clock-owed changes happen here: an invitation response that has come due, the
    /// approval and activation of a change a team was asked for, the approval of a
    /// deletion it was asked for, and the removal an APPROVED deletion owes once its
    /// pending window has run out. Nothing a caller has to ask for.
    ///
    /// Carrying out one of those actions is also what records its session's execution
    /// ([`Session::record_execution`]), so a session reports the action it approves as
    /// `EXECUTED` from the instant this store really applied it.
    ///
    /// What is owed is applied in the order the CLOCK made it due, not in the order this
    /// function walks the store, so the outcome depends on `now` alone and not on when the
    /// request that materialises it happens to arrive: where two changes bear on the same
    /// record — the approvers' answer to a session and that session's expiration — the one
    /// that fell due first is applied first, and the later one acts on its result.
    ///
    /// A removal the clock owes is carried out only while the rule that permits it still
    /// holds at the instant it comes due, read out of the store here and not out of the
    /// request that scheduled it: an identity source a stored team's approvers
    /// authenticate through is in use, so it keeps its record and its owed removal
    /// ([`ComAmazonawsMpaAwsfluffycoreserviceState::team_arn_using_identity_source`]).
    pub fn materialize_due(&mut self, now: SystemTime) {
        for team in self.approval_teams.values_mut() {
            // The approvers of the team AND of the change it has been asked to approve
            // (`ApprovalTeam::approver_records_mut`): an approver that change ADDS is
            // invited when the change is requested, so its response comes due while the
            // change is still waiting.
            for approver in team.approver_records_mut() {
                if approver.primary_identity_status != ApproverIdentityStatus::Pending {
                    continue;
                }
                let Some(due) = approver.invitation_response_due else {
                    continue;
                };
                if due > now {
                    continue;
                }
                // The response is stamped with the instant it was DUE, not with
                // `now`, so a materialised value does not move between reads.
                approver.primary_identity_status = ApproverIdentityStatus::Accepted;
                approver.response_time = Some(due);
                approver.last_activity = Some(ApproverLastActivity::RespondedToInvitation);
                approver.last_activity_time = Some(due);
                approver.invitation_response_due = None;
            }

            // A team is active once every approver has accepted their invitation. A team
            // with NO approvers never activates: `Approvers` is `length(1..20)`, so such
            // a team cannot be created, and a vacuous `all()` must not promote one. A team
            // that is `PENDING` because a change is waiting on its approvers is not
            // waiting on an invitation, so its activation is the change's, below.
            if team.status == TeamStatus::Pending
                && team.outstanding_update().is_none()
                && !team.approvers.is_empty()
                && team
                    .approvers
                    .iter()
                    .all(|a| a.primary_identity_status == ApproverIdentityStatus::Accepted)
            {
                team.status = TeamStatus::Active;
                team.status_code = None;
            }
        }

        // A new identity source is ACTIVE once its activation comes due.
        for source in self.identity_sources.values_mut() {
            if source.status != IdentitySourceStatus::Creating {
                continue;
            }
            let Some(due) = source.activation_due else {
                continue;
            };
            if due > now {
                continue;
            }
            source.activate();
        }

        // The instant the approvers' answer is owed at, per session the clock owes one in:
        // a requested change's approval ([`PendingTeamUpdate::approval_due`]) and a
        // requested deletion's ([`PendingTeamDeletion::approval_due`]). It is read BEFORE
        // the endings below, because what the clock owes is owed in the order it fell due,
        // not in the order this function happens to walk the store: an answer owed while
        // the session was still open is the earlier change, so the session carries that
        // answer (the loops below record it) instead of ending unanswered, however long
        // after both instants the next request arrives.
        let answers_owed: BTreeMap<&str, SystemTime> = self
            .approval_teams
            .values()
            .flat_map(|team| {
                let update = team.outstanding_update().and_then(|update| {
                    update
                        .approval_due
                        .map(|due| (update.update_session_arn.as_str(), due))
                });
                let deletion = team.unapproved_deletion().and_then(|deletion| {
                    deletion
                        .approval_due
                        .map(|due| (deletion.session_arn.as_str(), due))
                });
                update.into_iter().chain(deletion)
            })
            .collect();

        for session in self.sessions.values_mut() {
            // A new session is PENDING — waiting on its approvers — once its activation
            // comes due.
            if session.status == SessionStatus::Creating
                && session.activation_due.is_some_and(|due| due <= now)
            {
                session.status = SessionStatus::Pending;
                session.activation_due = None;
            }

            // A session no approver answered before its expiration is over: it FAILED,
            // with the `EXPIRED` status code, and completed at the instant it expired.
            // The completion is stamped with the DUE instant, not with `now`, so a
            // materialised value does not move between reads. Nothing claims an approver
            // answered: every response stays `NO_RESPONSE`.
            //
            // A session whose answer the clock owed BEFORE that expiration was answered
            // while it was still open, so the expiration pre-empts nothing: it is left
            // outstanding for the loops below to record the answer in, and only a session
            // no answer was owed in before it ran out ends unanswered.
            if session.status.is_outstanding()
                && session.expiration_time <= now
                && answers_owed
                    .get(session.arn.as_str())
                    .is_none_or(|due| *due >= session.expiration_time)
            {
                session.status = SessionStatus::Failed;
                session.status_code = Some(SessionStatusCode::Expired);
                session.completion_time = Some(session.expiration_time);
                session.activation_due = None;
            }
        }

        // The change a team's approvers were asked to approve: their approval comes due
        // first, then the activation of the configuration it carries. A session that ended
        // BEFORE that approval was owed — cancelled, or run out unanswered — is a change
        // the team never approved, so the team keeps the configuration it has; a session
        // whose expiration fell due only afterwards was left outstanding above, so the
        // approval the clock owed first is recorded here.
        let requested: Vec<String> = self
            .approval_teams
            .values()
            .filter(|team| team.outstanding_update().is_some())
            .map(|team| team.arn.clone())
            .collect();
        for team_arn in requested {
            let Some((session_arn, approval_due)) = self
                .approval_teams
                .get(&team_arn)
                .and_then(ApprovalTeam::outstanding_update)
                .map(|update| (update.update_session_arn.clone(), update.approval_due))
            else {
                continue;
            };
            if let Some(due) = approval_due {
                match self
                    .sessions
                    .get(&session_arn)
                    .map(|session| (session.status, session.completion_time))
                {
                    // Still outstanding: the approvers' answer is owed at `due`, and the
                    // answer is stamped with that instant rather than with `now`, so a
                    // materialised value does not move between reads.
                    Some((status, _)) if status.is_outstanding() => {
                        if due <= now {
                            let answered = self
                                .sessions
                                .get_mut(&session_arn)
                                .map_or_else(Vec::new, |session| session.approve(due));
                            if let Some(team) = self.approval_teams.get_mut(&team_arn) {
                                team.record_update_approval(due, &answered);
                            }
                        }
                    }
                    // The session already carries the team's approval, which is only ever
                    // recorded at the instant it was DUE, so it is read back under the
                    // same gate rather than on trust.
                    Some((SessionStatus::Approved, completed)) if due <= now => {
                        let answered = self
                            .sessions
                            .get_mut(&session_arn)
                            .map_or_else(Vec::new, |session| session.approve(due));
                        if let Some(team) = self.approval_teams.get_mut(&team_arn) {
                            team.record_update_approval(completed.unwrap_or(due), &answered);
                        }
                    }
                    // Nothing is owed yet: an approval that has not come due.
                    Some((SessionStatus::Approved, _)) => {}
                        // The session ended before an approval was owed in it, or no longer
                    // exists: the requested version is one this team will never move to.
                    _ => {
                        if let Some(team) = self.approval_teams.get_mut(&team_arn) {
                            team.record_update_failure();
                        }
                        continue;
                    }
                }
            }

            // The approved configuration becomes the live one once its activation is due.
            // That activation IS the action the session approved being carried out — the
            // requested description, strategy and approvers take effect and the requested
            // version becomes the team's live one — so the session it was approved in
            // records its execution at the same instant, and reports it as `EXECUTED`
            // rather than as still waiting to run.
            let activation_due = self
                .approval_teams
                .get(&team_arn)
                .and_then(ApprovalTeam::outstanding_update)
                .and_then(|update| update.activation_due);
            if let Some(due) = activation_due.filter(|due| *due <= now) {
                if let Some(team) = self.approval_teams.get_mut(&team_arn) {
                    team.activate_pending_update(due);
                }
                if let Some(session) = self.sessions.get_mut(&session_arn) {
                    session.record_execution();
                }
            }
        }

        // The deletion a team's approvers were asked to approve. Their approval is what
        // STARTS the pending window, so it is recorded first and the removal below is owed
        // only afterwards. A session that ended BEFORE that approval was owed — cancelled,
        // or run out unanswered — is a deletion the team refused, so the team stays; a
        // session whose expiration fell due only afterwards was left outstanding above, so
        // the approval the clock owed first is recorded here and the deletion the caller
        // was promised stands.
        let requested_deletions: Vec<String> = self
            .approval_teams
            .values()
            .filter(|team| team.unapproved_deletion().is_some())
            .map(|team| team.arn.clone())
            .collect();
        for team_arn in requested_deletions {
            let Some((session_arn, Some(due))) = self
                .approval_teams
                .get(&team_arn)
                .and_then(ApprovalTeam::unapproved_deletion)
                .map(|deletion| (deletion.session_arn.clone(), deletion.approval_due))
            else {
                continue;
            };
            match self
                .sessions
                .get(&session_arn)
                .map(|session| (session.status, session.completion_time))
            {
                // Still outstanding: the approvers' answer is owed at `due`, and the
                // answer is stamped with that instant rather than with `now`, so a
                // materialised value does not move between reads — which is also why the
                // completion instant the request was answered with stays true.
                Some((status, _)) if status.is_outstanding() => {
                    if due <= now {
                        let answered = self
                            .sessions
                            .get_mut(&session_arn)
                            .map_or_else(Vec::new, |session| session.approve(due));
                        if let Some(team) = self.approval_teams.get_mut(&team_arn) {
                            team.record_deletion_approval(due, &answered);
                        }
                    }
                }
                // The session already carries the team's approval, which is only ever
                // recorded at the instant it was DUE, so it is read back under the same
                // gate rather than on trust.
                Some((SessionStatus::Approved, completed)) if due <= now => {
                    let answered = self
                        .sessions
                        .get_mut(&session_arn)
                        .map_or_else(Vec::new, |session| session.approve(due));
                    if let Some(team) = self.approval_teams.get_mut(&team_arn) {
                        team.record_deletion_approval(completed.unwrap_or(due), &answered);
                    }
                }
                // Nothing is owed yet: an approval that has not come due.
                Some((SessionStatus::Approved, _)) => {}
                // The session ended before an approval was owed in it, or no longer
                // exists: this is a deletion the team did not approve, and an unapproved
                // deletion deletes nothing.
                _ => {
                    if let Some(team) = self.approval_teams.get_mut(&team_arn) {
                        team.record_deletion_failure();
                    }
                }
            }
        }

        // A team whose APPROVED deletion's pending window has elapsed is gone. The window
        // runs from the approval ([`PendingTeamDeletion::removal_due`]), so a team that
        // was asked to be deleted and never approved it is never removed here.
        //
        // This removal is the action the team's deletion session approved being carried
        // out, and it takes that session with it: a session asks ONE team's approvers and
        // cannot outlive that team
        // ([`ComAmazonawsMpaAwsfluffycoreserviceState::remove_team`]), so there is no
        // record left to report the execution on — the session's own answer from here is
        // the one a session of a team this store no longer holds earns.
        let elapsed: Vec<String> = self
            .approval_teams
            .values()
            .filter(|team| {
                team.pending_deletion
                    .as_ref()
                    .and_then(PendingTeamDeletion::removal_due)
                    .is_some_and(|due| due <= now)
            })
            .map(|team| team.arn.clone())
            .collect();
        for arn in elapsed {
            self.remove_team(&arn);
        }

        // An identity source whose deletion `DeleteIdentitySource` started is gone once
        // that removal comes due: until then it is reported `DELETING`, afterwards
        // `ListIdentitySources` omits it. The removal is carried out only while the
        // condition that permits it still holds — a source an approver record of a stored
        // team authenticates through is IN USE and is not removed at all — so the path
        // that really drops the record consults the one resolver that decides it
        // ([`ComAmazonawsMpaAwsfluffycoreserviceState::team_arn_using_identity_source`])
        // rather than trusting the request that scheduled the removal: a reference
        // recorded after that request was accepted keeps the source, so no read ever
        // reports an approver authenticated by a source this store no longer holds. The
        // removal stays owed and is carried out at the first instant no stored team names
        // the source any more.
        //
        // It is the LAST thing this pass decides, after every other change the clock owes
        // at `now`, because what is owed is owed in the order it fell due and not in the
        // order this function walks the store: a change that drops the last reference to
        // the source — the activation of an approver set that no longer names it, or the
        // removal of the team holding it — is materialised above, so this decision reads
        // the store as it stands at `now`.
        let removable: Vec<String> = self
            .identity_sources
            .values()
            .filter(|source| {
                source.status == IdentitySourceStatus::Deleting
                    && source.deletion_due.is_some_and(|due| due <= now)
                    && self.team_arn_using_identity_source(&source.arn).is_none()
            })
            .map(|source| source.arn.clone())
            .collect();
        for arn in removable {
            self.remove_identity_source(&arn);
        }
    }

    /// Drop a team and everything that indexes it.
    pub fn remove_team(&mut self, arn: &str) {
        if let Some(team) = self.approval_teams.remove(arn) {
            if self.name_index.get(&team.name).is_some_and(|a| a == arn) {
                self.name_index.remove(&team.name);
            }
        }
        self.idempotency_tokens
            .retain(|_, first| first.arn != arn);
        // A session asks ONE team's approvers, so it cannot outlive that team: when the
        // team is gone its sessions name nobody, and `GetSession` answers for them the
        // same 404 `GetApprovalTeam` answers for the team.
        self.sessions
            .retain(|_, session| session.approval_team_arn != arn);
    }

    /// The answer the create that was given this idempotency key already returned, if
    /// this store recorded one.
    ///
    /// The one place a `@idempotencyToken` replay is resolved, so every create of this
    /// service answers a retry out of what its first call really returned
    /// ([`IdempotentCreate`]) rather than out of the resource as it stands now.
    #[must_use]
    pub fn idempotent_create(&self, key: &str) -> Option<&IdempotentCreate> {
        self.idempotency_tokens.get(key)
    }

    /// Record the answer a create returned under its idempotency key, so a retry carrying
    /// the same token repeats it.
    pub fn record_idempotent_create(&mut self, key: String, first: IdempotentCreate) {
        self.idempotency_tokens.insert(key, first);
    }

    /// The stored team with this ARN.
    #[must_use]
    pub fn team(&self, arn: &str) -> Option<&ApprovalTeam> {
        self.approval_teams.get(arn)
    }

    /// The stored team with this ARN, for update.
    pub fn team_mut(&mut self, arn: &str) -> Option<&mut ApprovalTeam> {
        self.approval_teams.get_mut(arn)
    }

    /// The tags of the resource `arn` names, or `None` when this store holds no
    /// resource under that ARN.
    ///
    /// The one place a tagging operation resolves a resource ARN, so `TagResource` and
    /// `ListTagsForResource` agree on which resources exist and read the same map. The
    /// resources this service stores are the approval team and the identity source, so
    /// an ARN naming anything else is not found; when another resource is added here,
    /// both operations pick it up from this one function — and so does every other
    /// ARN-addressed operation, whose existence decision is this resolution
    /// ([`ComAmazonawsMpaAwsfluffycoreserviceState::records_resource`]).
    #[must_use]
    pub fn resource_tags(&self, arn: &str) -> Option<&BTreeMap<String, String>> {
        if let Some(team) = self.team(arn) {
            return Some(&team.tags);
        }
        self.identity_source(arn).map(|source| &source.tags)
    }

    /// Whether this store records a RESOURCE under `arn`.
    ///
    /// The one place an operation addressing a resource by ARN decides whether it
    /// exists, so the tagging and the resource-policy operations agree on it. Every
    /// resource this service stores is taggable, so the decision IS the tag resolution
    /// ([`ComAmazonawsMpaAwsfluffycoreserviceState::resource_tags`]): whatever resource
    /// kinds that one resolver holds — the approval team and the identity source — are
    /// exactly the ones an ARN-addressed operation finds, and an ARN naming anything else
    /// names no resource. A resource kind added to that resolver is therefore picked up
    /// by every operation that resolves a resource ARN without a second inventory to keep
    /// in step.
    #[must_use]
    pub fn records_resource(&self, arn: &str) -> bool {
        self.resource_tags(arn).is_some()
    }

    /// The tags of the resource `arn` names, for update. Same resolution as
    /// [`ComAmazonawsMpaAwsfluffycoreserviceState::resource_tags`].
    pub fn resource_tags_mut(&mut self, arn: &str) -> Option<&mut BTreeMap<String, String>> {
        if self.approval_teams.contains_key(arn) {
            return self.team_mut(arn).map(|team| &mut team.tags);
        }
        self.identity_source_mut(arn)
            .map(|source| &mut source.tags)
    }

    /// The stored identity source with this ARN.
    #[must_use]
    pub fn identity_source(&self, arn: &str) -> Option<&IdentitySource> {
        self.identity_sources.get(arn)
    }

    /// The stored identity source with this ARN, for update.
    pub fn identity_source_mut(&mut self, arn: &str) -> Option<&mut IdentitySource> {
        self.identity_sources.get_mut(arn)
    }

    /// Record an identity source, replacing any entry under the same ARN.
    pub fn insert_identity_source(&mut self, source: IdentitySource) {
        self.identity_sources.insert(source.arn.clone(), source);
    }

    /// Drop an identity source and everything that indexes it.
    ///
    /// The record's tags go with it, so the source stops being a taggable resource at
    /// the same instant it stops being listed, and an `@idempotencyToken` that resolved
    /// to it is freed the way [`ComAmazonawsMpaAwsfluffycoreserviceState::remove_team`]
    /// frees a team's: a replay of the create that minted this source mints a new one
    /// rather than answering with a record that is gone.
    pub fn remove_identity_source(&mut self, arn: &str) {
        self.identity_sources.remove(arn);
        self.idempotency_tokens
            .retain(|_, first| first.arn != arn);
    }

    /// The ARN of a stored approval team whose approvers authenticate through the
    /// identity source `arn` names, if any.
    ///
    /// An approver is recorded with the identity source they are authenticated by
    /// (`Approver::primary_identity_source_arn`), so a source with such a reference is IN
    /// USE: removing it would leave the team pointing at a source this store no longer
    /// holds. BOTH approver sets a team holds are read
    /// ([`ApprovalTeam::approver_records`]) — the live one and the one belonging to the
    /// change the team has been asked to approve, which the team read reports and
    /// activation installs as the live set — because a reference in either is a reference
    /// this store holds. It is the one place that reference is resolved, so every
    /// operation deciding whether a source is in use reads the same records, and so does
    /// the clock pass that carries a scheduled removal out
    /// ([`ComAmazonawsMpaAwsfluffycoreserviceState::materialize_due`]).
    #[must_use]
    pub fn team_arn_using_identity_source(&self, arn: &str) -> Option<&str> {
        self.approval_teams
            .values()
            .find(|team| {
                team.approver_records()
                    .any(|approver| approver.primary_identity_source_arn == arn)
            })
            .map(|team| team.arn.as_str())
    }

    /// Every stored identity source in creation order — the stable ordering
    /// `ListIdentitySources` pages over.
    #[must_use]
    pub fn identity_sources_in_creation_order(&self) -> Vec<&IdentitySource> {
        let mut sources: Vec<&IdentitySource> = self.identity_sources.values().collect();
        sources.sort_by_key(|source| source.seq);
        sources
    }

    /// The next identity-source creation-order sequence number, which is also the id its
    /// ARN carries ([`identity_source_arn`]). Never reused.
    pub fn next_identity_source_seq(&mut self) -> u64 {
        self.next_identity_source_seq += 1;
        self.next_identity_source_seq
    }

    /// Every stored team in creation order — the stable ordering
    /// `ListApprovalTeams` pages over.
    #[must_use]
    pub fn teams_in_creation_order(&self) -> Vec<&ApprovalTeam> {
        let mut teams: Vec<&ApprovalTeam> = self.approval_teams.values().collect();
        teams.sort_by_key(|team| team.seq);
        teams
    }

    /// Every policy attached to the resource `resource_arn` names, in attachment
    /// order, or `None` when this store records NO resource under that ARN — which is
    /// what tells a missing resource from a resource with no policy of the name asked
    /// for.
    ///
    /// Whether a resource exists is decided by the one existence resolver every
    /// ARN-addressed operation of this service shares
    /// ([`ComAmazonawsMpaAwsfluffycoreserviceState::records_resource`]), so a resource
    /// the tagging reads report cannot be missing here. What a recorded resource CARRIES
    /// is then its own: an approval team's attached policies are the `Policies`
    /// references it holds, so the resource-policy reads and `GetApprovalTeam$Policies`
    /// report one and the same attachment list, and a recorded resource of a kind no
    /// policy attaches to — an identity source — carries no attachment and resolves to an
    /// EMPTY list rather than to a missing resource. Each reference is resolved through
    /// the CATALOG, so every member reported comes from the policy record itself; a
    /// reference the catalog does not hold names no policy and is reported by nothing.
    #[must_use]
    pub fn resource_policies(&self, resource_arn: &str) -> Option<Vec<ResourcePolicy>> {
        if !self.records_resource(resource_arn) {
            return None;
        }
        let Some(team) = self.team(resource_arn) else {
            return Some(Vec::new());
        };
        Some(
            team.policies
                .iter()
                .filter_map(|attached| self.attached_resource_policy(resource_arn, attached))
                .collect(),
        )
    }

    /// The policy of this name and type attached to the resource `resource_arn` names,
    /// or `None` when no resource is recorded under that ARN or none of its attached
    /// policies carries that name and type.
    #[must_use]
    pub fn resource_policy(
        &self,
        resource_arn: &str,
        policy_name: &str,
        policy_type: PolicyType,
    ) -> Option<ResourcePolicy> {
        self.resource_policies(resource_arn)?
            .into_iter()
            .find(|policy| policy.policy_name == policy_name && policy.policy_type == policy_type)
    }

    /// The resource policy an attachment of `attached_arn` to `resource_arn` reports,
    /// or `None` when the attachment names no policy version the catalog holds.
    ///
    /// `PolicyReference$PolicyArn` is a `QualifiedPolicyArn`, so an attachment names a
    /// policy and one of its versions: the version id when it names one, and the
    /// policy's current default version when it names `$DEFAULT`.
    #[must_use]
    pub fn attached_resource_policy(
        &self,
        resource_arn: &str,
        attached_arn: &str,
    ) -> Option<ResourcePolicy> {
        // The one resolver for a qualified policy ARN, so an attachment and
        // `GetPolicyVersion` name the same version — and the same document — for the
        // same ARN.
        let (policy, version) = self.policy_version(attached_arn)?;
        Some(ResourcePolicy {
            resource_arn: resource_arn.to_string(),
            policy_arn: policy.arn.clone(),
            policy_version_arn: policy.version_arn(version.version_id),
            policy_version_id: version.version_id,
            policy_name: policy.name.clone(),
            policy_type: policy.policy_type,
            document: version.document.clone(),
        })
    }

    /// The catalog policy with this ARN.
    #[must_use]
    pub fn policy(&self, arn: &str) -> Option<&CatalogPolicy> {
        self.policies.get(arn)
    }

    /// The refusal the first of `references` that may not be attached earns, or `None`
    /// when every one of them is attachable.
    ///
    /// A policy is AWS-owned — no operation of this model mints, updates or deletes one —
    /// so a reference is attachable only when the catalog already holds the policy AND
    /// the version it names, and that version's [`PolicyStatus`] still allows attaching
    /// it ([`PolicyStatus::is_attachable`]): a status is the catalog's own statement of
    /// whether a version may still be attached, so a version it reports as no longer
    /// attachable is refused even though it resolves.
    ///
    /// References are resolved through [`Self::policy_version`], the one resolver a
    /// `QualifiedPolicyArn` is read by — which keeps resolving a version in any status, so
    /// the catalog reads report every version this store holds and an attachment recorded
    /// while its version was attachable stays resolvable. An attachment this gate accepts
    /// is therefore one every later read of the resource can resolve.
    #[must_use]
    pub fn unattachable_policy_reference<'a, I>(&self, references: I) -> Option<ValidationException>
    where
        I: IntoIterator<Item = &'a str>,
    {
        references
            .into_iter()
            .find_map(|reference| match self.policy_version(reference) {
                None => Some(unattachable_policy(reference)),
                Some((_, version)) if !version.status.is_attachable() => {
                    Some(policy_version_not_attachable(reference, version.status))
                }
                Some(_) => None,
            })
    }

    /// The catalog policy and the one of its versions that a `QualifiedPolicyArn` names,
    /// or `None` when `arn` is outside that grammar or the catalog holds no such version.
    ///
    /// The grammar is [`parse_policy_version_arn`]'s, so every reader of a qualified
    /// policy ARN — the resource-policy reads and `GetPolicyVersion` alike — resolves one
    /// through this one function and accepts exactly the ARNs the model's pattern fixes.
    /// `$DEFAULT` is resolved against the policy's RECORDED default, so such an ARN names
    /// whichever version is the default when it is read.
    #[must_use]
    pub fn policy_version(&self, arn: &str) -> Option<(&CatalogPolicy, &CatalogPolicyVersion)> {
        let reference = parse_policy_version_arn(arn)?;
        let policy = self.policy(&reference.policy_arn)?;
        let version_id = match reference.version {
            PolicyVersionSelector::Default => policy.default_version,
            PolicyVersionSelector::Id(version_id) => version_id,
        };
        policy.version(version_id).map(|version| (policy, version))
    }

    /// Record a policy in the catalog, replacing any entry under the same ARN.
    pub fn insert_policy(&mut self, policy: CatalogPolicy) {
        self.policies.insert(policy.arn.clone(), policy);
    }

    /// Store a team and index it by name.
    ///
    /// Replacing a team already stored under this ARN drops the name entry the previous
    /// record held, so the index can never point at a name no stored team carries.
    pub fn insert_team(&mut self, team: ApprovalTeam) {
        if let Some(previous) = self.approval_teams.get(&team.arn) {
            if previous.name != team.name
                && self
                    .name_index
                    .get(&previous.name)
                    .is_some_and(|arn| arn == &team.arn)
            {
                self.name_index.remove(&previous.name);
            }
        }
        self.name_index.insert(team.name.clone(), team.arn.clone());
        self.approval_teams.insert(team.arn.clone(), team);
    }

    /// The next creation-order sequence number.
    pub fn next_team_seq(&mut self) -> u64 {
        self.next_team_seq += 1;
        self.next_team_seq
    }

    /// A fresh, never-reused approver id.
    pub fn mint_approver_id(&mut self) -> String {
        self.next_approver_seq += 1;
        format!("approver-{}", self.next_approver_seq)
    }

    /// The approver set a requested configuration produces: an approver `requested` names
    /// that `existing` already holds — the same identity from the same identity source —
    /// keeps its stored record, and therefore its id and its invitation response, while
    /// one the request adds is newly invited under a fresh id.
    ///
    /// Minting an id advances this store's approver sequence, so building a set is a WRITE
    /// to the store and not a local computation: this is the one place either write path
    /// that builds an approver set calls, and a handler calls it only once every refusal
    /// the request can earn has been decided.
    ///
    /// `None` says only that `now` names no instant an invited approver's response could
    /// be recorded at ([`Approver::invited`]), which the calling operation answers with
    /// the server error it declares. That instant is computed BEFORE the first id is
    /// minted, so such a refusal writes nothing and consumes no `ApproverId`, exactly as
    /// [`ComAmazonawsMpaAwsfluffycoreserviceState::start_session`] consumes no
    /// initiation-order id for a session it cannot build.
    pub fn invite_requested_approvers<'a, I>(
        &mut self,
        requested: I,
        existing: &[Approver],
        now: SystemTime,
    ) -> Option<Vec<Approver>>
    where
        I: IntoIterator<Item = (&'a str, &'a str)>,
    {
        // Computed before anything is minted, so the store is untouched when the instant
        // an invitation's response comes due cannot be computed at all.
        invitation_response_due(now)?;
        requested
            .into_iter()
            .map(|(identity_id, source_arn)| {
                let retained = existing.iter().find(|stored| {
                    stored.primary_identity_id == identity_id
                        && stored.primary_identity_source_arn == source_arn
                });
                match retained {
                    Some(stored) => Some(stored.clone()),
                    None => Approver::invited(
                        self.mint_approver_id(),
                        identity_id.to_string(),
                        source_arn.to_string(),
                        now,
                    ),
                }
            })
            .collect()
    }

    /// The stored session with this ARN.
    #[must_use]
    pub fn session(&self, arn: &str) -> Option<&Session> {
        self.sessions.get(arn)
    }

    /// The stored session with this ARN, for update.
    pub fn session_mut(&mut self, arn: &str) -> Option<&mut Session> {
        self.sessions.get_mut(arn)
    }

    /// The session an operation ADDRESSED BY A SESSION ARN may act on, or the refusal a
    /// `SessionArn` that resolves to no session this caller can see earns.
    ///
    /// Whether a session exists is not something this service tells a caller: a session
    /// ARN this store holds nothing under is an AUTHORIZATION refusal
    /// ([`access_denied`], 403) and never the not-found an ARN-addressed team, policy,
    /// identity source or tagged resource answers ([`resource_not_found`], 404) — so the
    /// answer to a session ARN cannot be read as "that session is gone" by a caller who
    /// may simply not be permitted to see it. The ONE place that decision is made, so
    /// every session-addressed operation refuses alike.
    pub fn visible_session(&self, arn: &str) -> Result<&Session, AccessDeniedException> {
        self.session(arn).ok_or_else(access_denied)
    }

    /// The session an operation addressed by a session ARN may act on, for update — the
    /// write path's half of [`Self::visible_session`], refused identically, so a mutation
    /// of a session the caller cannot see is never reported as one of an absent session.
    pub fn visible_session_mut(
        &mut self,
        arn: &str,
    ) -> Result<&mut Session, AccessDeniedException> {
        self.session_mut(arn).ok_or_else(access_denied)
    }

    /// Record a session, replacing any entry under the same ARN.
    pub fn insert_session(&mut self, session: Session) {
        self.sessions.insert(session.arn.clone(), session);
    }

    /// Every stored session of one approval team, in initiation order — the stable
    /// ordering a session listing pages over.
    #[must_use]
    pub fn team_sessions_in_initiation_order(&self, team_arn: &str) -> Vec<&Session> {
        let mut sessions: Vec<&Session> = self
            .sessions
            .values()
            .filter(|session| session.approval_team_arn == team_arn)
            .collect();
        sessions.sort_by_key(|session| session.seq);
        sessions
    }

    /// The team's OUTSTANDING baseline sessions, in initiation order: the sessions an
    /// approver can be waiting on. The ONE predicate both the team read's resolver
    /// ([`Self::pending_baseline_session_arn`]) and the guard a baseline session is minted
    /// behind ([`Self::approver_with_pending_baseline_session`]) ask, so the two cannot
    /// mean different sessions by "pending".
    #[must_use]
    pub fn outstanding_baseline_sessions(&self, team_arn: &str) -> Vec<&Session> {
        self.team_sessions_in_initiation_order(team_arn)
            .into_iter()
            .filter(|session| {
                session.kind == SessionKind::Baseline && session.status.is_outstanding()
            })
            .collect()
    }

    /// The ARN of the team's OUTSTANDING baseline session that asks `approver_id`, or
    /// `None` when that approver has no baseline session awaiting them.
    ///
    /// The one resolver for `GetApprovalTeamResponseApprover$PendingBaselineSessionArn`,
    /// so the team read and `GetSession` report the same session for the same approver.
    /// A session that has completed, expired or been cancelled is no longer pending and
    /// so is not reported.
    ///
    /// At most one outstanding baseline session can ask an approver — a start that would
    /// ask one who already has such a session is refused
    /// ([`Self::approver_with_pending_baseline_session`]) — so the single session this
    /// answers with is the approver's only pending baseline session, never the first of
    /// several.
    #[must_use]
    pub fn pending_baseline_session_arn(
        &self,
        team_arn: &str,
        approver_id: &str,
    ) -> Option<String> {
        self.outstanding_baseline_sessions(team_arn)
            .into_iter()
            .find(|session| session.includes_approver(approver_id))
            .map(|session| session.arn.clone())
    }

    /// The first of `approver_ids` that is ALREADY asked by an outstanding baseline
    /// session of `team_arn`, with that session's ARN — or `None` when none of them is.
    ///
    /// An approver has at most ONE pending baseline session, so a start that would ask an
    /// approver who is already waiting on one is refused rather than recorded. The sessions
    /// searched are the ones [`Self::outstanding_baseline_sessions`] answers with — the
    /// same sessions, in the same order, that the team read resolves an approver's pending
    /// baseline session out of — so the guard a session is minted behind and the session
    /// that read reports can never be about different sessions. The team's sessions are
    /// gathered ONCE, whatever the approver set's size.
    #[must_use]
    pub fn approver_with_pending_baseline_session<'a, I>(
        &self,
        team_arn: &str,
        approver_ids: I,
    ) -> Option<(&'a str, String)>
    where
        I: IntoIterator<Item = &'a str>,
    {
        let outstanding = self.outstanding_baseline_sessions(team_arn);
        approver_ids.into_iter().find_map(|approver_id| {
            outstanding
                .iter()
                .find(|session| session.includes_approver(approver_id))
                .map(|session| (approver_id, session.arn.clone()))
        })
    }

    /// The sentence a request that would record a further session is refused with once the
    /// store already holds as many as it will ([`MAX_SESSIONS`]), or `None` while it can
    /// hold another.
    ///
    /// The ONE place the store's session ceiling is decided and worded, so the operations
    /// that start a session cannot drift apart in the count they compare, the comparison
    /// they make or the words they refuse with — [`MAX_SESSIONS`]'s own doc owns the reason
    /// for the bound. Only the ERROR stays per operation, the way the model fixes it: each
    /// caller lifts this sentence into the error IT declares for a request it cannot
    /// satisfy, exactly as every create lifts [`tags_past_resource_ceiling`] into its own.
    #[must_use]
    pub fn session_ceiling_refusal(&self) -> Option<String> {
        (self.sessions.len() >= MAX_SESSIONS).then(|| {
            format!("The account has reached its quota of {MAX_SESSIONS} approval sessions.")
        })
    }

    /// The next initiation-order sequence number, which is also the id a session's ARN
    /// carries ([`session_arn`]). Never reused.
    pub fn next_session_seq(&mut self) -> u64 {
        self.next_session_seq += 1;
        self.next_session_seq
    }

    /// Record a NEW session for `team` and answer its ARN — the one path every operation
    /// of this service that starts a session goes through.
    ///
    /// The initiation-order sequence number the session's ARN carries is consumed by a
    /// session that is really stored and by nothing else: when the session cannot be built
    /// ([`Session::for_team`] answers `None` because `now` names no instant its expiration
    /// could be recorded at) nothing is written, so the refusal the calling operation
    /// answers with leaves the store — and the next session's ARN — exactly as they were.
    pub fn start_session(
        &mut self,
        kind: SessionKind,
        team: &ApprovalTeam,
        approvers: Vec<SessionApprover>,
        requester_region: String,
        now: SystemTime,
    ) -> Option<String> {
        let seq = self.next_session_seq + 1;
        let session = Session::for_team(kind, team, approvers, requester_region, now, seq)?;
        let arn = session.arn.clone();
        self.next_session_seq = seq;
        self.insert_session(session);
        Some(arn)
    }
}

/// The refusal an `Approvers` list that names one identity more than once earns, or
/// `None` when every `(PrimaryIdentityId, PrimaryIdentitySourceArn)` pair is distinct.
///
/// One identity cannot hold two seats on a team: a repeat would inflate the approver
/// count (N) that `MofN` is measured against without adding anyone who can approve.
///
/// Neither member is `@sensitive`, so the refusal names the pair that was repeated —
/// held, like every refusal that quotes a request value back, to the bound its own
/// `Message` member declares ([`bounded_message`]): `PrimaryIdentitySourceArn` alone is
/// `@length(max: 1000)`, the whole bound of the message it goes into. Both write paths
/// that take an approver list raise this one refusal, so neither can word — or overrun —
/// it differently.
#[must_use]
pub fn repeated_identity<'a, I>(approvers: I) -> Option<ValidationException>
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let mut seen = ::std::collections::BTreeSet::new();
    for (identity_id, source_arn) in approvers {
        if !seen.insert((identity_id, source_arn)) {
            return Some(ValidationException {
                message: bounded_message(format!(
                    "Approvers names the same identity more than once ({identity_id} from \
                     {source_arn}); each approver must be a distinct identity."
                )),
            });
        }
    }
    None
}

/// The refusal an `Approvers` list earns when an approver's `PrimaryIdentitySourceArn` is
/// not the ARN of an identity source ([`is_identity_source_arn`]), or `None` when every
/// entry of the list names one.
///
/// The member is documented to carry the Amazon Resource Name of the identity source an
/// approver authenticates through, and the model gives it no ARN `@pattern`
/// (`String`, `@length(max: 1000)`), so the framework's constraint boundary cannot decide
/// it and the handler that takes it has to: a value that is not an ARN this service could
/// have minted for an identity source addresses no identity source at all, and the store
/// would otherwise keep — and later report, from `GetApprovalTeam` and `GetSession` — an
/// identity source reference that resolves to nothing and that
/// [`ComAmazonawsMpaAwsfluffycoreserviceState::team_arn_using_identity_source`] can never
/// match. It is therefore refused with the one sentence this service answers the
/// malformed-ARN class with ([`invalid_arn`], 400), which names neither the member nor the
/// grammar. Both write paths that take an approver list raise this one refusal, so neither
/// can word it differently.
#[must_use]
pub fn malformed_approver_identity_source<'a, I>(source_arns: I) -> Option<ValidationException>
where
    I: IntoIterator<Item = &'a str>,
{
    source_arns
        .into_iter()
        .any(|arn| !is_identity_source_arn(arn))
        .then(invalid_arn)
}

/// ARN of an approval team, in the form `ApprovalTeamArn`'s pattern fixes:
/// `arn:aws:mpa:<region>:<account>:approval-team/<name>`.
#[must_use]
pub fn team_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("mpa", region, &format!("{APPROVAL_TEAM_RESOURCE}/{name}"))
}

/// The account every caller of this emulator authenticates as: every request is
/// signed with the emulator's own credentials, so the caller's account is
/// [`::engine::emulator_account_id`].
#[must_use]
pub fn caller_account_id() -> &'static str {
    ::engine::emulator_account_id()
}

/// The ARN of the identity this emulator's callers authenticate as, as an
/// authorization refusal names the principal it refused.
#[must_use]
pub fn caller_identity_arn() -> String {
    ::engine::emulator_arn("iam", "", "user/emulator")
}

/// The account id an ARN embeds: its fifth colon-separated field, as every ARN
/// grammar places it (`arn:<partition>:<service>:<region>:<account>:<resource>`).
#[must_use]
pub fn arn_account_id(arn: &str) -> Option<&str> {
    arn.split(':').nth(4)
}

/// The region an ARN embeds: its fourth colon-separated field, empty on a global
/// resource's ARN.
///
/// A resource minted FOR another resource (a session for its approval team) takes its
/// region from that resource's ARN rather than from the request, so the two ARNs cannot
/// disagree about where the resource lives.
#[must_use]
pub fn arn_region(arn: &str) -> Option<&str> {
    arn.split(':').nth(3).filter(|region| is_arn_region(region))
}

/// Whether `arn` is a well-formed Amazon Resource Name:
/// `arn:<partition>:<service>:<region>:<account>:<resource>` — six colon-separated
/// fields with a non-empty partition, service and resource.
///
/// Region and account are empty on a global resource's ARN (an AWS-managed MPA policy,
/// `arn:aws:mpa:::aws:policy/...`, for one), so neither is required.
#[must_use]
pub fn is_well_formed_arn(arn: &str) -> bool {
    let mut fields = arn.splitn(6, ':');
    let prefix = fields.next() == Some("arn");
    let partition = fields.next().is_some_and(|field| !field.is_empty());
    let service = fields.next().is_some_and(|field| !field.is_empty());
    let region = fields.next().is_some();
    let account = fields.next().is_some();
    let resource = fields.next().is_some_and(|field| !field.is_empty());
    prefix && partition && service && region && account && resource
}

/// Whether `arn` is an ARN of one of THIS service's regional, account-owned resources of
/// the kind `resource_name` names (`identity-source`, `approval-team`, …).
///
/// The grammar is the one every such ARN of this service takes, as `ApprovalTeamArn`'s
/// own pattern spells it
/// (`^arn:aws(-[^:]+)?:mpa:[a-z0-9-]{1,20}:[0-9]{12}:approval-team/[a-zA-Z0-9._-]+$`, from
/// `smithy_query(query="shape",shape="ApprovalTeamArn")`), with the resource name of the
/// kind asked for in the resource field — the name that kind's own `arn` template and
/// `@iamResource` carry (`identity-source/{IdentitySourceArn}` on the `IdentitySource`
/// resource, [`IDENTITY_SOURCE_RESOURCE`]):
/// `arn:<partition>:mpa:<region>:<account>:<resource_name>/<id>`.
///
/// Stricter than [`is_well_formed_arn`], which answers only whether a value is an ARN at
/// all: this answers whether it is an ARN this service could have minted for that kind of
/// resource, which is what a member documented to carry one ADDRESSES. An ARN of another
/// service's namespace, of another resource kind, or with no resource id names no resource
/// of this kind, so it is refused as malformed ([`invalid_arn`]) rather than looked up.
#[must_use]
pub fn is_service_resource_arn(arn: &str, resource_name: &str) -> bool {
    let mut fields = arn.splitn(6, ':');
    if fields.next() != Some("arn") {
        return false;
    }
    if !fields.next().is_some_and(is_arn_partition) {
        return false;
    }
    if fields.next() != Some(SERVICE_ACTION_PREFIX) {
        return false;
    }
    if !fields.next().is_some_and(is_arn_region) {
        return false;
    }
    if !fields.next().is_some_and(is_arn_account_id) {
        return false;
    }
    fields
        .next()
        .and_then(|resource| resource.strip_prefix(resource_name))
        .and_then(|rest| rest.strip_prefix('/'))
        .is_some_and(is_arn_resource_id)
}

/// Whether `partition` fits the `aws(-[^:]+)?` partition segment this service's ARN
/// patterns declare: the `aws` partition, or one of its suffixed partitions.
fn is_arn_partition(partition: &str) -> bool {
    partition == "aws"
        || partition
            .strip_prefix("aws-")
            .is_some_and(|suffix| !suffix.is_empty())
}

/// Whether `account` fits the `[0-9]{12}` account segment this service's ARN patterns
/// declare.
fn is_arn_account_id(account: &str) -> bool {
    account.len() == 12 && account.bytes().all(|byte| byte.is_ascii_digit())
}

/// Whether `id` fits the `[a-zA-Z0-9._-]+` resource id this service's ARN patterns
/// declare after the resource name.
fn is_arn_resource_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'_' || byte == b'-')
}

/// Whether `arn` names a resource in the caller's own account.
#[must_use]
pub fn is_caller_account_arn(arn: &str) -> bool {
    arn_account_id(arn) == Some(caller_account_id())
}

/// Authorize a request that identifies a resource by ARN: a resource outside the
/// caller's own account is not one the caller can act on, so the request is refused
/// BEFORE the resource is resolved — the answer to a foreign-account ARN never
/// depends on whether anything is stored under it.
///
/// `action` is the operation's name (`Self::NAME` in a handler), which the refusal
/// names as the IAM action `mpa:<action>`.
pub fn authorize_resource_arn(action: &str, arn: &str) -> Result<(), AccessDeniedException> {
    if is_caller_account_arn(arn) {
        return Ok(());
    }
    Err(cross_account_access_denied(action, arn))
}

/// The `AccessDeniedException` payload for a resource ARN outside the caller's
/// account. It is built from the request — the caller principal, the attempted
/// action and the ARN asked for — and says nothing about the resource, which is
/// never looked up.
#[must_use]
pub fn cross_account_access_denied(action: &str, arn: &str) -> AccessDeniedException {
    AccessDeniedException {
        message: bounded_message(format!(
            "User: {} is not authorized to perform: {SERVICE_ACTION_PREFIX}:{action} on \
             resource: {arn} because no resource-based policy allows the \
             {SERVICE_ACTION_PREFIX}:{action} action",
            caller_identity_arn()
        )),
    }
}

/// The `AccessDeniedException` payload for an operation an organization restricts to
/// its management account, called by an account that is not it.
///
/// The sentence is the one AWS answers this condition with; the requirement is the
/// 403 and the `AccessDeniedException` code, and the refusal names no value from the
/// request because the decision does not depend on one.
#[must_use]
pub fn not_management_account() -> AccessDeniedException {
    AccessDeniedException {
        message: "Action can only be performed by an organization\u{2019}s management account."
            .to_string(),
    }
}

/// The one message this service refuses a caller it does not tell whether the addressed
/// resource exists with.
///
/// `AccessDeniedException$Message` is `@required` with `@length(max: 1000)` and the model
/// fixes no text for it, so the rule is RECORDED behaviour: the refusal is this one bare
/// sentence, with no part taken from the request — naming neither the operation, nor the
/// caller, nor the ARN asked for, because a refusal that quoted any of them would be the
/// very disclosure it exists to withhold.
pub const ACCESS_DENIED_MESSAGE: &str = "Access Denied.";

/// The `AccessDeniedException` (403) a lookup whose resource's EXISTENCE this service does
/// not disclose answers with: [`ACCESS_DENIED_MESSAGE`].
///
/// Raised where the answer must not depend on whether the resource is stored — see
/// [`ComAmazonawsMpaAwsfluffycoreserviceState::visible_session`], the one resolution of a
/// `SessionArn` — rather than for the account boundary, which is decided from the ARN
/// alone and is worded by [`cross_account_access_denied`].
#[must_use]
pub fn access_denied() -> AccessDeniedException {
    AccessDeniedException {
        message: ACCESS_DENIED_MESSAGE.to_string(),
    }
}

/// The one message every resource-lookup failure of this service answers with.
///
/// `ResourceNotFoundException$Message` is `@required` with `@length(max: 1000)` and the
/// model fixes no text for it, so the rule is RECORDED behaviour: four ARN-addressed
/// operations (`GetApprovalTeam`, `StartActiveApprovalTeamDeletion`, `ListPolicyVersions`,
/// `GetResourcePolicy`) answer one and the same sentence for an ARN that resolves to
/// nothing, independent of the operation and of the ARN supplied. It is therefore a
/// SERVICE CONSTANT: the refusal names no value the request carried.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "The requested resource does not exist.";

/// The `ResourceNotFoundException` an ARN-addressed lookup answers with when nothing is
/// recorded under the ARN it was given: 404, and [`RESOURCE_NOT_FOUND_MESSAGE`].
///
/// The tag family is the one exception — see [`tagged_resource_not_found`].
#[must_use]
pub fn resource_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: RESOURCE_NOT_FOUND_MESSAGE.to_string(),
    }
}

/// The `ResourceNotFoundException` the TAG FAMILY (`TagResource`, `UntagResource`,
/// `ListTagsForResource`) answers with for a `ResourceArn` no stored resource carries.
///
/// Its message is BUILT FROM THE REQUEST and is the `ResourceArn` member verbatim, with
/// no surrounding prose: on this family the not-found message IS the identifier of the
/// resource that was not found, which is what a caller reads out of it. `ResourceArn` is
/// `@length(max: 1000)` here — exactly the bound on `ResourceNotFoundException$Message`
/// — so the value always fits the message it becomes and is never cut.
#[must_use]
pub fn tagged_resource_not_found(resource_arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: resource_arn.to_string(),
    }
}

/// The one message this service refuses a malformed Amazon Resource Name with.
///
/// `ValidationException$Message` is `@required` and the model fixes no text for it, so
/// the rule is RECORDED behaviour: a value that is not a well-formed MPA ARN is refused
/// with 400 `ValidationException` and this one sentence, on every member that carries an
/// ARN (`GetPolicyVersion$PolicyVersionArn`, and the `ResourceArn` of the tag family,
/// `GetResourcePolicy` and `ListResourcePolicies`). It names neither the member nor the
/// grammar: the malformed-ARN class is one service-level rule, not a per-member
/// constraint report.
pub const INVALID_ARN_MESSAGE: &str = "Invalid Resource Arn";

/// The `ValidationException` (400) a member that should carry an ARN and does not is
/// refused with: [`INVALID_ARN_MESSAGE`].
#[must_use]
pub fn invalid_arn() -> ValidationException {
    ValidationException {
        message: INVALID_ARN_MESSAGE.to_string(),
    }
}

/// The one message this service refuses an `IdentitySourceArn` that addresses no identity
/// source with.
///
/// `ValidationException$Message` is `@required` and the model fixes no text for it, so the
/// rule is RECORDED behaviour: the refusal of the member that ADDRESSES an identity source
/// NAMES that member, so a caller sending several ARN-valued members can tell which one
/// was rejected. The whole sentence is fixed text — nothing in it comes from the request,
/// and the rejected value is not echoed.
pub const INVALID_IDENTITY_SOURCE_ARN_MESSAGE: &str = "Invalid IdentitySourceArn.";

/// The `ValidationException` (400) an `IdentitySourceArn` that addresses no identity
/// source ([`addresses_identity_source`]) is refused with:
/// [`INVALID_IDENTITY_SOURCE_ARN_MESSAGE`]. Raised by both operations that read the
/// member, so they cannot word one refusal two ways.
#[must_use]
pub fn invalid_identity_source_arn() -> ValidationException {
    ValidationException {
        message: INVALID_IDENTITY_SOURCE_ARN_MESSAGE.to_string(),
    }
}

/// The one message this service refuses a `NextToken` it did not issue with.
///
/// `ValidationException$Message` is `@required` and the model fixes no text for it, so
/// the rule is RECORDED behaviour: every paginated listing of this service answers one
/// and the same sentence for a token it cannot read as a position of its own, naming
/// neither the operation nor the value supplied — the pagination-token class is one
/// service-level rule.
pub const FOREIGN_NEXT_TOKEN_MESSAGE: &str = "NextToken is not a token this service issued.";

/// The `ValidationException` (400) a `NextToken` no page of this service handed out is
/// refused with: [`FOREIGN_NEXT_TOKEN_MESSAGE`].
#[must_use]
pub fn foreign_next_token() -> ValidationException {
    ValidationException {
        message: FOREIGN_NEXT_TOKEN_MESSAGE.to_string(),
    }
}

/// Whether `pattern` is an ARN grammar.
///
/// Every ARN-valued member of this model anchors its `@pattern` on the literal `arn:`
/// prefix an Amazon Resource Name starts with — `ApprovalTeamArn`
/// (`^arn:aws(-[^:]+)?:mpa:…:approval-team/…$`), `SessionArn` and `QualifiedPolicyArn`
/// (`^arn:.{1,63}:mpa:::aws:policy/…$`), from
/// `smithy_query(query="shape",shape="ApprovalTeamArn,SessionArn,QualifiedPolicyArn")` —
/// and no member whose value is not an ARN does. So the prefix is what tells a failed
/// ARN grammar from any other failed pattern, without naming a member.
#[must_use]
pub fn is_arn_pattern(pattern: &str) -> bool {
    pattern.starts_with("^arn:")
}

/// Whether `violation` is a value that should have been an ARN failing the ARN grammar
/// its member's `@pattern` declares — the malformed-ARN class this service answers with
/// [`invalid_arn`] rather than with the framework's per-member constraint report.
#[must_use]
pub fn is_malformed_arn_violation(violation: &::aws_smithy_schema_server::Violation) -> bool {
    matches!(
        &violation.kind,
        ::aws_smithy_schema_server::ViolationKind::Pattern(pattern) if is_arn_pattern(pattern)
    )
}

/// `message` cut to the modelled message bound ([`MAX_EXCEPTION_MESSAGE`]) on a
/// character boundary, so a long value quoted back — a resource ARN
/// (`ApprovalTeamArn` is `length(20..2048)`), a qualified policy reference
/// (`QualifiedPolicyArn` is `length(max: 1224)`), an identity source ARN or a `Region`
/// (`length(max: 1000)`) — cannot push the refusal past the length its own member
/// declares.
///
/// Every `Message` member of every error this service declares is `@length(max: 1000)`,
/// so this is the one bound, and every refusal built by interpolating a value the
/// REQUEST carried goes through here: a refusal is a modelled response shape, and a
/// request within its own constraints can never make this service answer outside them.
#[must_use]
pub fn bounded_message(message: String) -> String {
    if message.len() <= MAX_EXCEPTION_MESSAGE {
        return message;
    }
    let mut end = MAX_EXCEPTION_MESSAGE;
    while end > 0 && !message.is_char_boundary(end) {
        end -= 1;
    }
    message[..end].to_string()
}

/// The region to stamp into a minted ARN: the one this request's SigV4 credential
/// scope names, or [`DEFAULT_REGION`] when it names none the ARN grammar accepts.
#[must_use]
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_arn_region(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `region` fits the `[a-z0-9-]{1,20}` region segment every `mpa` ARN
/// pattern declares.
fn is_arn_region(region: &str) -> bool {
    !region.is_empty()
        && region.len() <= 20
        && region
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Whether `region` names an Amazon Web Services Region.
///
/// A member documented to carry a Region (`IamIdentityCenter$Region`, "Amazon Web
/// Services Region where the IAM Identity Center instance is located") is modelled as a
/// plain string with only a length bound — no `@pattern` and no value set — so which
/// values it may carry is not expressible on the wire and the constraint boundary cannot
/// decide it. The set is the shared catalog `emulator_telemetry::policy::KNOWN_REGIONS`
/// ("every AWS region the wire may carry, sorted for binary search"), already a
/// dependency of this crate, rather than a list copied in here, so the Regions this
/// service accepts cannot drift from the ones the rest of the emulator knows. The lookup
/// is the binary search that ordering is maintained for.
#[must_use]
pub fn is_aws_region(region: &str) -> bool {
    ::emulator_telemetry::policy::KNOWN_REGIONS
        .binary_search(&region)
        .is_ok()
}

/// The `ValidationException` (400) a member that should name an Amazon Web Services
/// Region is refused with when its value names none ([`is_aws_region`]).
///
/// The message names the member and the value it carried: neither `IamIdentityCenter`
/// nor its `Region` is `@sensitive`, so the value is quoted back, and the whole message
/// is held to the bound `ValidationException$Message` declares.
#[must_use]
pub fn invalid_region(member: &str, region: &str) -> ValidationException {
    ValidationException {
        message: bounded_message(format!(
            "{member} {region:?} is not an Amazon Web Services Region."
        )),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMpaAwsfluffycoreserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMpaAwsfluffycoreserviceState>)
}
