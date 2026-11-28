// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice service.
//!
//! The service has two resources in this slice:
//!
//! * the **benefit catalog** — service-owned, read-only data. The API models no
//!   `CreateBenefit`, so the catalogs and the benefits inside them are seeded here, at
//!   state init, and `ListBenefits` reports them from the store like any other read. The
//!   **benefit allocations** AWS has granted are service-owned in the same way (the API
//!   models no `CreateBenefitAllocation`), so they are seeded beside the benefits they
//!   draw from and `ListBenefitAllocations` reports them from the store.
//! * the **benefit application** — created, updated, submitted and cancelled by the
//!   caller. Every application is stored exactly once, keyed by its
//!   `benappl-[0-9a-z]{14}` id inside the catalog it was created in, so each operation
//!   reads and writes the one record.
//!
//! Timestamps are stored as epoch MILLISECONDS (`i64`) and rendered into the modelled
//! `date-time` members with [`timestamp`] on the way out. Keeping an integer in the
//! store (rather than a `DateTime`) is what lets [`bump_millis`] guarantee an update is
//! *observably* later than the write before it even when both land inside one clock
//! tick.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

use ::aws_smithy_types::{Document, Number};
use ::sha2::{Digest, Sha256};

use crate::ops::types::{Amendment, Contact, FileDetail, FileInput, Tag};

/// Redeeming one of the promotional credit codes this service has issued, which is what
/// spends the record the reads above report.
mod credit_code_redemption;

pub use credit_code_redemption::*;

/// This service's ARN namespace, from
/// `smithy_query(query="service_info")` -> `arnNamespace: partnercentral`.
pub const ARN_SERVICE: &str = "partnercentral";

/// The region an ARN carries when the request names none; the emulator's documented
/// default region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// What a handler reports when the shared store cannot be reached. Each handler still
/// builds its own modelled error (the generated error enums are per-operation), so this
/// is the one part of that answer they share.
pub const STORE_UNAVAILABLE: &str =
    "the benefit application store is unavailable; retry the request";

/// The local name of the error shape this service denies an unauthorized caller with:
/// the modelled `com.amazonaws.partnercentralbenefits#AccessDeniedException`, which is
/// also what `smithy_server_runtime::OperationError::shape_name` reports for it and what
/// the wire carries as the error code.
pub const ACCESS_DENIED_SHAPE: &str = "AccessDeniedException";

/// The HTTP status this service serves `AccessDeniedException` with.
///
/// The model declares the shape `@error("client")` with `@httpError(403)`, but the
/// service answers its access denial with **400**: recorded behaviour (corpus
/// `simparity-wire-aws-v1`, fixture
/// `partnercentral-benefits-list-and-tag-validation-error-paths`, step `task#1`
/// `ListBenefits` and step `task#4` `ListTagsForResource`), where the body is the
/// modelled `AccessDeniedException` carrying the partner-entitlement sentence and the
/// status on the wire is 400. A status the model does not predict is settled by what the
/// service serves, so the recording is the authority for this one.
pub const ACCESS_DENIED_STATUS: u16 = 400;

/// The status this service serves a modelled error with, given the status that error's
/// own `@httpError` declares.
///
/// The model is the authority for every error shape but [`ACCESS_DENIED_SHAPE`], whose
/// served status is [`ACCESS_DENIED_STATUS`]. The choice is keyed on the error SHAPE and
/// made once for the whole service — `crate::scaffolded`'s `OperationError::http_status`
/// is its only caller — so every operation that can deny a caller answers the same
/// status rather than one handler differing from the next.
pub fn served_status(shape_name: &str, modelled: u16) -> u16 {
    match shape_name {
        ACCESS_DENIED_SHAPE => ACCESS_DENIED_STATUS,
        _ => modelled,
    }
}

/// What this service answers a caller whose account holds no active AWS partner
/// benefit: the entitlement gate's refusal, carried by the modelled
/// `AccessDeniedException` (served with [`ACCESS_DENIED_STATUS`], which is the status the
/// recordings show rather than the model's `@httpError`).
///
/// The gate is an account-level authorization stage, not a per-request computation, so
/// its wording is a fixed sentence rather than something built from the request. This
/// is the sentence the service answers with, recorded against the real API.
pub const PARTNER_ENTITLEMENT_REQUIRED: &str = "This action requires an active AWS \
     partner benefit. Register as an AWS partner if you haven't already, or contact AWS \
     partner central support.";

/// The name a `ResourceNotFoundException` message gives the catalog a request named.
///
/// The catalog is not a modelled `@resource` (no operation creates one); it is named by
/// the `Catalog` input member every operation carries, so that member's name is what the
/// message states. See [`not_found_message`].
pub const CATALOG_KIND: &str = "Catalog";

/// The name a `ResourceNotFoundException` message gives a benefit: the modelled resource
/// shape `com.amazonaws.partnercentralbenefits#Benefit`
/// (`smithy_query(query="shape",shape="Benefit")`).
pub const BENEFIT_KIND: &str = "Benefit";

/// The name a `ResourceNotFoundException` message gives a benefit application: the
/// modelled resource shape `com.amazonaws.partnercentralbenefits#BenefitApplication`
/// (`smithy_query(query="shape",shape="BenefitApplication")`).
pub const APPLICATION_KIND: &str = "BenefitApplication";

/// The name a `ResourceNotFoundException` message gives a benefit allocation: the
/// modelled resource shape `com.amazonaws.partnercentralbenefits#BenefitAllocation`
/// (`smithy_query(query="shape",shape="BenefitAllocation")`).
pub const ALLOCATION_KIND: &str = "BenefitAllocation";

/// The prefix a benefit id carries, from the modelled `BenefitSummary$Id` pattern
/// `^ben-[0-9a-z]{14}$`.
pub const BENEFIT_ID_PREFIX: &str = "ben-";

/// The prefix a benefit-application id carries, from the modelled
/// `BenefitApplicationId` pattern `^benappl-[0-9a-z]{14}$`.
pub const APPLICATION_ID_PREFIX: &str = "benappl-";

/// The prefix a benefit-allocation id carries, from the modelled `BenefitAllocationId`
/// pattern `^benalloc-[0-9a-z]{14}$`.
pub const ALLOCATION_ID_PREFIX: &str = "benalloc-";

/// How many characters follow the prefix in either id, from those same patterns.
pub const ID_BODY_LEN: usize = 14;

/// The characters an id body is built from: `[0-9a-z]`, from those same patterns.
const ID_ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// The `BenefitApplicationStatus` a freshly created application carries — the one the
/// enum names for an application the partner has not submitted yet.
pub const STATUS_PENDING_SUBMISSION: &str = "PENDING_SUBMISSION";

/// The `BenefitApplicationStatus` a submitted application carries while AWS reviews it.
pub const STATUS_IN_REVIEW: &str = "IN_REVIEW";

/// The terminal `BenefitApplicationStatus` of a cancelled application.
pub const STATUS_CANCELED: &str = "CANCELED";

/// The `BenefitApplicationStatus` a submitted application carries while AWS waits on the
/// partner. No operation in this slice puts an application into it — AWS's own review
/// workflow does — so it is named here only because [`RECALLABLE_STATUSES`] is defined
/// over the statuses a *submitted* application can carry, not over one example of them.
pub const STATUS_ACTION_REQUIRED: &str = "ACTION_REQUIRED";

/// The `BenefitApplicationStatus` of an application AWS's review approved. No operation
/// in this slice puts an application into it — AWS's own review workflow does — so it is
/// named here because [`TERMINAL_STATUSES`] is defined over every status AWS has finished
/// with, not over the one this emulator can reach.
pub const STATUS_APPROVED: &str = "APPROVED";

/// The `BenefitApplicationStatus` of an application AWS's review rejected. Reached the
/// same way as [`STATUS_APPROVED`], and terminal for the same reason.
pub const STATUS_REJECTED: &str = "REJECTED";

/// The statuses AWS has finished with: `APPROVED` and `REJECTED` end the review, and
/// `CANCELED` ends the application.
///
/// Nothing moves an application out of one of these — `SubmitBenefitApplication`,
/// `RecallBenefitApplication` and `CancelBenefitApplication` each refuse from them — so
/// no change to the application's content is accepted in one either (see
/// [`BenefitApplication::is_terminal`]). The six `BenefitApplicationStatus` values are
/// `crate::ops::types::BENEFIT_APPLICATION_STATUS_VALUES`.
pub const TERMINAL_STATUSES: &[&str] = &[STATUS_APPROVED, STATUS_REJECTED, STATUS_CANCELED];

/// The statuses in which `UpdateBenefitApplication` rewrites an application's members.
///
/// An update is the partner editing its own DRAFT, so it is accepted only while the
/// application has not been submitted: `PENDING_SUBMISSION`. Once it is submitted the
/// record is the one AWS is reviewing, which is why the API models
/// `RecallBenefitApplication` (back to draft "for further modifications") and
/// `AmendBenefitApplication` (change named fields of a submitted application, with an
/// `AmendmentReason`); from every other status an update is refused with the modelled
/// `ConflictException`. See [`BenefitApplication::is_updatable`].
pub const UPDATABLE_STATUSES: &[&str] = &[STATUS_PENDING_SUBMISSION];

/// The statuses `RecallBenefitApplication` returns to draft.
///
/// A recall takes back an application that has been SUBMITTED and that AWS has not
/// finished with: `PENDING_SUBMISSION` was never submitted, and `APPROVED`, `REJECTED`
/// and `CANCELED` are terminal, so a recall from any of those is refused with the
/// modelled `ConflictException`. The six `BenefitApplicationStatus` values are
/// `crate::ops::types::BENEFIT_APPLICATION_STATUS_VALUES`.
pub const RECALLABLE_STATUSES: &[&str] = &[STATUS_IN_REVIEW, STATUS_ACTION_REQUIRED];

/// The `BenefitApplicationStage` of an application the partner has not submitted yet: the
/// record is still the partner's own draft, so AWS's processing workflow has not started
/// on it.
///
/// `BenefitApplicationStage` is an unconstrained string in the model
/// (`smithy_query(query="shape",shape="BenefitApplicationStage")`), so the stage
/// vocabulary is CHOSEN here: these five names are this emulator's, not values AWS
/// publishes. What is not chosen is the relation — the stage is the position in the
/// processing workflow that the application's current `BenefitApplicationStatus` puts it
/// at, which is what [`stage_for_status`] computes and the only thing this service knows
/// about AWS's review.
pub const STAGE_DRAFT: &str = "DRAFT";

/// The stage of a submitted application AWS is reviewing. See [`STAGE_DRAFT`] for where
/// the vocabulary comes from.
pub const STAGE_REVIEW: &str = "REVIEW";

/// The stage of a submitted application whose review is waiting on the partner. See
/// [`STAGE_DRAFT`].
pub const STAGE_PARTNER_ACTION: &str = "PARTNER_ACTION";

/// The stage of an application AWS's review has decided, either way. See [`STAGE_DRAFT`].
pub const STAGE_DECISION: &str = "DECISION";

/// The stage of an application the partner withdrew, which therefore never reached a
/// decision. See [`STAGE_DRAFT`].
pub const STAGE_CLOSED: &str = "CLOSED";

/// Every stage [`stage_for_status`] reports, so a caller (and a test) can enumerate the
/// values this service's `Stages` filter can select on.
pub const BENEFIT_APPLICATION_STAGE_VALUES: &[&str] = &[
    STAGE_DRAFT,
    STAGE_REVIEW,
    STAGE_PARTNER_ACTION,
    STAGE_DECISION,
    STAGE_CLOSED,
];

/// The workflow stage an application in `status` stands at, or `None` for a status that is
/// no `BenefitApplicationStatus` value (nothing in the store holds one, so no stage is
/// invented for it).
///
/// The modelled `Stage` is "the current stage in the benefit application processing
/// workflow", and the only thing this service knows about where an application stands in
/// that workflow is the status it holds: a draft has not entered the workflow, a submitted
/// application is under review or back with the partner, and a decided or withdrawn one
/// has left it. So the stage is a projection of the status rather than a separately stored
/// field, which is also what keeps it from going stale when the status moves.
pub fn stage_for_status(status: &str) -> Option<&'static str> {
    match status {
        STATUS_PENDING_SUBMISSION => Some(STAGE_DRAFT),
        STATUS_IN_REVIEW => Some(STAGE_REVIEW),
        STATUS_ACTION_REQUIRED => Some(STAGE_PARTNER_ACTION),
        STATUS_APPROVED | STATUS_REJECTED => Some(STAGE_DECISION),
        STATUS_CANCELED => Some(STAGE_CLOSED),
        _ => None,
    }
}

/// The two `BenefitStatus` values the model declares (see
/// `crate::ops::types::BENEFIT_STATUS_VALUES`).
pub const BENEFIT_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`BENEFIT_STATUS_ACTIVE`].
pub const BENEFIT_STATUS_INACTIVE: &str = "INACTIVE";

/// The `FulfillmentType` of a benefit fulfilled by paying the partner money; one of the
/// three values the model declares for that enum (`CREDITS | CASH | ACCESS`, see
/// `crate::ops::types::FULFILLMENT_TYPE_VALUES`).
pub const FULFILLMENT_TYPE_CASH: &str = "CASH";
/// The `FulfillmentType` of a benefit fulfilled with AWS promotional credits.
pub const FULFILLMENT_TYPE_CREDITS: &str = "CREDITS";
/// The `FulfillmentType` of a benefit fulfilled by granting access rather than value.
pub const FULFILLMENT_TYPE_ACCESS: &str = "ACCESS";

/// The `BenefitAllocationStatus` of an allocation inside its term and not yet consumed;
/// one of the three values the model declares for that enum (`ACTIVE | INACTIVE |
/// FULFILLED`, see `crate::ops::types::BENEFIT_ALLOCATION_STATUS_VALUES`).
pub const ALLOCATION_STATUS_ACTIVE: &str = "ACTIVE";
/// The `BenefitAllocationStatus` of an allocation that is no longer usable — the status
/// an expiry leaves behind (see [`BenefitAllocation::expire_due`]).
pub const ALLOCATION_STATUS_INACTIVE: &str = "INACTIVE";
/// The `BenefitAllocationStatus` of an allocation whose benefit has been delivered in
/// full. Terminal: an expiry does not move it, because there is nothing left to expire.
pub const ALLOCATION_STATUS_FULFILLED: &str = "FULFILLED";

/// The `CurrencyCode` every monetary amount this service holds is denominated in; a value
/// of the model's own `CurrencyCode` enum (see `crate::ops::types::CURRENCY_CODE_VALUES`).
///
/// AWS partner funding benefits are granted in US dollars and no input member carries a
/// currency, so this is the one currency the store uses — and the one whose minor unit is
/// [`MINOR_UNITS_PER_UNIT`].
pub const ALLOCATION_CURRENCY: &str = "USD";

/// Minor units in one unit of [`ALLOCATION_CURRENCY`]: USD is an ISO-4217 currency with two
/// decimal digits, so an amount is stored as cents and rendered with two decimals (see
/// [`Money::amount_text`]).
pub const MINOR_UNITS_PER_UNIT: i64 = 100;

/// The amount a granted allocation is created with, in minor units of
/// [`ALLOCATION_CURRENCY`].
///
/// CHOSEN, not an AWS amount: AWS decides what it allocates when it fulfills a benefit and
/// this API models no member that carries the amount, so a grant needs a figure and this is
/// it. Nothing is issued against a fresh grant — the issuance is a later act of AWS's
/// fulfillment workflow — so a granted allocation reports this as allocated and zero as
/// issued.
pub const GRANTED_ALLOCATION_AMOUNT_MINOR: i64 = 25_000 * MINOR_UNITS_PER_UNIT;

/// The `StatusReason` an expired allocation reports (`BenefitAllocationSummary$StatusReason`:
/// "additional information explaining the current status of the benefit allocation").
///
/// AWS publishes no wording for this member, so the sentence is the emulator's own; what
/// is not chosen is the FACT it reports — the allocation's `ExpiresAt` has passed, which
/// the model documents as the instant it "expires and is no longer usable".
pub const ALLOCATION_EXPIRED_REASON: &str =
    "The allocation's term ended at its ExpiresAt and it is no longer usable.";

/// The largest page either list operation builds, and the size applied when the request
/// names no `MaxResults`. It is the maximum of the modelled `MaxResults` range
/// (`range {min: 1, max: 100}`), which the centralized walk already refuses values
/// outside of.
pub const MAX_PAGE_SIZE: i32 = 100;

/// The most benefit applications this emulator holds in one catalog.
///
/// `CreateBenefitApplication` is the only operation that grows the store, so this is the
/// bound that keeps it finite. AWS publishes no quota for it, so the number is CHOSEN:
/// large enough that ordinary use never reaches it, small enough that a caller cannot
/// grow the process's memory without limit. `client_tokens` holds at most one entry per
/// application created, so it is bounded by the same count.
pub const MAX_APPLICATIONS_PER_CATALOG: usize = 500;

/// The most benefit allocations this emulator holds in one catalog.
///
/// The catalog is seeded with [`SEEDED_ALLOCATIONS`] and grown only by
/// [`ComAmazonawsPartnercentralbenefitsPartnercentralbenefitsserviceState::grant_allocation`],
/// which no request from the wire reaches, so this is a store guard rather than a quota a
/// caller can push against. CHOSEN, like [`MAX_APPLICATIONS_PER_CATALOG`]: AWS publishes
/// no quota for allocations per partner.
pub const MAX_ALLOCATIONS_PER_CATALOG: usize = 500;

/// Milliseconds in a day, the unit the seeded allocation terms below are written in.
const MILLIS_PER_DAY: i64 = 24 * 60 * 60 * 1000;

/// The term a granted benefit allocation runs for, from `StartsAt` to `ExpiresAt`.
///
/// CHOSEN, not an AWS duration: AWS documents `ExpiresAt` ("the timestamp when the benefit
/// allocation expires and is no longer usable") but publishes no default term for an
/// allocation, and no input member carries one. A year is the term AWS partner funding
/// benefits are granted for in practice, and keeps a granted allocation inside its window
/// for the whole life of an emulator process.
pub const ALLOCATION_TERM_MILLIS: i64 = 365 * MILLIS_PER_DAY;

/// The most amendments one `AmendBenefitApplication` request may carry.
///
/// The modelled `AmendmentList` has no `@length`, so this is the bound on the work one
/// request drives — the loop that applies them runs under the store lock — and on how
/// much the amendment record below keeps. CHOSEN, not an AWS quota: far more fields than
/// one correction touches, while keeping one request's work and footprint finite.
pub const MAX_AMENDMENTS_PER_REQUEST: usize = 20;

/// The fewest amendments one `AmendBenefitApplication` request may carry.
///
/// The model marks `Amendments` `@required` but gives `AmendmentList` no `@length`, so an
/// empty list satisfies the centralized walk and this is the other end of the bound the
/// handler enforces for itself: a request that names nothing to amend amends nothing, and
/// is refused rather than answered with a write that did not happen.
pub const MIN_AMENDMENTS_PER_REQUEST: usize = 1;

/// The longest `Amendment$FieldPath` this service reads, in CHARACTERS — the unit
/// [`length_clause`] counts every length this service enforces in.
///
/// The member is an unconstrained `String`, so this is what keeps a path (and the record
/// of it below) from being arbitrarily long. CHOSEN: wider than any path into a document
/// this service stores, since [`MAX_DETAILS_DEPTH`] also caps the segments a path may
/// have.
pub const MAX_FIELD_PATH_LEN: usize = 256;

/// The longest value an amendment may write into `BenefitApplicationDetails`.
///
/// `Amendment$NewValue` is an unconstrained `String`, while the members a path can name
/// on the record itself carry the model's own `@length` (`BenefitApplicationName` 1..255,
/// `BenefitApplicationDescription` 1..1000). This is the matching bound for a leaf of
/// the document, which the model constrains not at all: the longest string member the
/// model lets a caller store on this resource.
pub const MAX_DETAILS_VALUE_LEN: usize = 1000;

/// The longest `AmendmentReason` this service stores, which is the modelled
/// `AmendBenefitApplicationInput$AmendmentReason` `@length(max: 1000)`
/// (`smithy_query(query="operation",operation="AmendBenefitApplication")`). The
/// amendment record below keeps the reason, so this is also the bound on the bytes one
/// record holds for it.
pub const MAX_AMENDMENT_REASON_LEN: usize = 1000;

/// The longest `ClientToken` this service stores, which is the modelled
/// `@length(min: 1, max: 64)` this service's `ClientToken` carries wherever the model
/// constrains it at all — `CreateBenefitApplication`, `UpdateBenefitApplication`,
/// `CancelBenefitApplication` and `AmendBenefitApplication` (the patterns
/// `^[a-zA-Z0-9-]{1,64}$` and `^[!-~]{1,64}$` say the same). The applied-write record
/// keeps the token, so this bounds the bytes one record holds for it.
///
/// `RecallBenefitApplicationInput$ClientToken` is modelled as a bare `String`
/// (`smithy_query(query="operation",operation="RecallBenefitApplication")`), so there the
/// centralized walk refuses nothing and the handler applies this bound itself with
/// [`length_clause`]; the token the store holds is bounded either way.
pub const MAX_CLIENT_TOKEN_LEN: usize = 64;

/// The shortest `ClientToken` this service accepts: the `min` half of the same modelled
/// `@length(min: 1, max: 64)` [`MAX_CLIENT_TOKEN_LEN`] is the `max` half of, so both
/// halves of one trait move together.
pub const MIN_CLIENT_TOKEN_LEN: usize = 1;

/// The longest `FileName` a stored `FileDetail` carries, which is the `max` half of the
/// modelled `FileDetail$FileName` `@length(min: 1, max: 255)`
/// (`smithy_query(query="shape",shape="FileDetail")`).
///
/// The member is DERIVED from the caller's `FileURI` rather than supplied, so the
/// centralized walk never sees it and [`file_name_of`] applies this bound itself with
/// [`length_clause`].
pub const MAX_FILE_NAME_LEN: usize = 255;

/// The shortest `FileName` a stored `FileDetail` carries: the `min` half of the same
/// modelled `@length(min: 1, max: 255)` [`MAX_FILE_NAME_LEN`] is the `max` half of, so
/// both halves of one trait move together.
pub const MIN_FILE_NAME_LEN: usize = 1;

/// The longest `StatusReason` this service stores on a benefit application.
///
/// It is the model's own bound on the member the value is REPORTED through —
/// `GetBenefitApplicationOutput$StatusReason` is `length {max: 1000}`
/// (`smithy_query(query="operation",operation="GetBenefitApplication")`; the generated
/// header of that operation prints the member's target only) — and the bound the model
/// puts on the input that writes it where it constrains that input at all
/// (`CancelBenefitApplicationInput$Reason` is `length {max: 1000}`, so the centralized
/// walk refuses a longer one there). `RecallBenefitApplicationInput$Reason` is modelled as
/// a bare `String`, so that handler applies this bound itself with [`length_clause`] and a
/// read can never report a reason longer than the member it is reported through admits.
pub const MAX_STATUS_REASON_LEN: usize = 1000;

/// The shortest `StatusReason` this service stores on a benefit application: the `min` end
/// of the SAME modelled constraint [`MAX_STATUS_REASON_LEN`] is the `max` end of, so both
/// ends of one bound move together.
///
/// The model states that end on the input that writes this member where it constrains that
/// input at all — `CancelBenefitApplicationInput$Reason` carries
/// `@pattern("^[\s\S]{1,1000}$")` beside its `@length(max: 1000)`
/// (`smithy_query(query="operation",operation="CancelBenefitApplication")`), which admits
/// 1 to 1000 characters, so the centralized walk refuses an EMPTY reason there. The input
/// that writes the member on the other operation is modelled as a bare `String`, so that
/// handler applies this end itself with [`length_clause`]: one stored member has one
/// acceptance rule whichever write sets it, and a recorded reason always says something.
pub const MIN_STATUS_REASON_LEN: usize = 1;

/// The bytes a stored request fingerprint occupies: a SHA-256 digest in hex.
///
/// A spent `ClientToken` is kept with the fingerprint of the request that spent it (see
/// [`request_fingerprint`]), so what the store holds for one token is this fixed size
/// however large that request was.
pub const REQUEST_FINGERPRINT_LEN: usize = 64;

/// What a request spending a `ClientToken` another, DIFFERENT request already spent is
/// refused with.
///
/// The token is the identity of one request, so a second request carrying it is not a
/// retry: it conflicts with the request the store already holds under that token, and is
/// refused with the modelled `ConflictException` (HTTP 409) rather than reported as a
/// success for a write it never made. The sentence is AWS's wording for this error class
/// — the same two-part guidance every AWS API gives a caller that reuses an idempotency
/// token with different parameters — and names no value of either request.
pub const IDEMPOTENT_PARAMETER_MISMATCH: &str = "The request uses the same client token as a \
     previous, but non-identical request. Do not reuse a client token with different requests, \
     unless the requests are identical.";

/// The most writes one benefit application keeps a record of.
///
/// Every operation that changes an application and carries a `ClientToken` is idempotent
/// in that token, which it recognises by looking the token up in this record, so the
/// record is what bounds how far back a retry is recognised. AWS publishes no retention
/// for the token, so the number is CHOSEN: more writes than a retry window needs, while
/// keeping the record of one application finite however many times it is written. Beyond
/// it the oldest entry is dropped, and a retry of a token that old applies its write
/// again.
///
/// With [`MAX_AMENDMENTS_PER_REQUEST`], [`MAX_FIELD_PATH_LEN`],
/// [`MAX_AMENDMENT_REASON_LEN`], [`MAX_CLIENT_TOKEN_LEN`] and
/// [`REQUEST_FINGERPRINT_LEN`] — each enforced, or fixed, before the record is written —
/// one application's write record holds at most 20 x (64-character token + 64-byte
/// fingerprint + 1000-character reason + 20 x 256-character path) of caller text:
/// bounded, like every other part of this store.
pub const MAX_APPLIED_WRITE_HISTORY: usize = 20;

/// The most members a stored `BenefitApplicationDetails` document may hold, counted over
/// the whole tree.
///
/// The member is an unconstrained `Document` and `AmendBenefitApplication` can ADD a
/// member the document did not hold, so a count bound is what keeps repeated amendments
/// from growing one record without limit. Emulator resource guard, NOT an AWS quota.
pub const MAX_DETAILS_MEMBERS: usize = 256;

/// The most text (member names and string values) a stored `BenefitApplicationDetails`
/// document may carry, in CHARACTERS — the unit [`length_clause`] counts every length
/// this service enforces in, and so the unit the requirement this bound is stated as
/// ([`details_requirement`]) names. Emulator resource guard, NOT an AWS quota; see
/// [`MAX_DETAILS_MEMBERS`].
///
/// A character costs at most four bytes in UTF-8, so one stored document holds at most
/// four times this many bytes of text: the store stays bounded whatever script the
/// caller's text is written in.
pub const MAX_DETAILS_TEXT_LEN: usize = 64 * 1024;

/// The most AWS resources one benefit application carries.
///
/// It is the model's own published bound on the list: `AssociatedResources` is
/// `length {max: 10}` on both `CreateBenefitApplicationInput` and
/// `GetBenefitApplicationOutput`
/// (`smithy_query(query="shape",shape="CreateBenefitApplicationInput")`), so a create and
/// an `AssociateBenefitApplicationResource` agree on how many resources one application can
/// be linked to, and a stored record never holds more than the read can report.
/// `AssociateBenefitApplicationResourceInput` binds one `ResourceArn` and carries no length
/// on how MANY resources a record may hold, so this is also the bound that keeps repeated
/// associations from growing one record without limit; an association past it is refused
/// with the modelled `ValidationException` and persists nothing. How LONG one of those ARNs
/// may be is [`MAX_ASSOCIATED_RESOURCE_ARN_LEN`].
pub const MAX_ASSOCIATED_RESOURCES_PER_APPLICATION: usize = 10;

/// The longest resource ARN one benefit application may be linked to, in CHARACTERS — the
/// unit [`length_clause`] counts every length this service enforces in.
///
/// It is the model's own published bound on this value: `ResourceArn` is
/// `length {min: 0, max: 1600}` on both operations that name one resource —
/// `AssociateBenefitApplicationResourceInput` and
/// `DisassociateBenefitApplicationResourceInput`
/// (`smithy_query(query="operation",operation="AssociateBenefitApplicationResource")`) — so
/// the centralized walk is the enforcement point there. The `Arns` list a create supplies
/// the same value through, and a read reports it with, bounds its ELEMENTS not at all
/// (`smithy_query(query="shape",shape="Arns")`), so that handler applies this bound itself
/// with [`length_clause`] (see [`associated_resources_refusal`]): one stored member has one
/// acceptance rule whichever write sets it, and no read can report an ARN another write
/// path would refuse.
pub const MAX_ASSOCIATED_RESOURCE_ARN_LEN: usize = 1600;

/// The most tags one resource of this service holds.
///
/// It is the model's own published bound on an application's tag set —
/// `CreateBenefitApplicationInput$Tags` is `length {max: 200}` (see
/// `smithy_query(query="shape",shape="CreateBenefitApplicationInput")`) — so a create
/// and a `TagResource` agree on how many tags one resource can carry.
/// `TagResourceRequest$tags` itself carries no length, so this is the bound that keeps a
/// resource's tag set finite; a merge that would take it past this is refused with the
/// modelled `ServiceQuotaExceededException`.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// The `ServiceQuotaExceededException$ResourceType` a tagging refusal reports: the
/// resource-type segment of the ARN whose tag set is full.
pub const QUOTA_RESOURCE_TYPE_APPLICATION: &str = APPLICATION_RESOURCE;

/// The `ServiceQuotaExceededException$QuotaCode` a tagging refusal reports.
///
/// The member is required and AWS publishes no quota code for this service's tag limit,
/// so the code NAMES the quota ([`MAX_TAGS_PER_RESOURCE`]) rather than imitating a
/// Service Quotas `L-########` identifier that no AWS rule produces.
pub const QUOTA_CODE_TAGS_PER_RESOURCE: &str = "TagsPerResource";

/// How this service renders a `Catalog` the request did not name, in the message of a
/// lookup that failed (see [`catalog_not_found_message`]).
pub const CATALOG_ABSENT: &str = "null";

/// The deepest `BenefitApplicationDetails` document this service stores.
///
/// The member is modelled as an unconstrained `Document`, so its nesting is entirely
/// caller-controlled and every walk over it is recursive. This is the bound that keeps
/// such a walk from exhausting the stack; a create or update carrying a deeper document
/// is refused with the modelled `ValidationException` rather than stored. Emulator
/// resource guard, NOT an AWS quota.
pub const MAX_DETAILS_DEPTH: usize = 32;

/// The catalogs this service serves.
///
/// PartnerCentral models `Catalog` as a required input on every operation and offers no
/// operation that creates one, so the set is fixed: `AWS` is the production catalog and
/// `Sandbox` the test catalog that PartnerCentral's APIs document. A request naming any
/// other catalog resolves to no catalog, and the handlers answer the modelled
/// `ResourceNotFoundException`.
pub const SEEDED_CATALOGS: &[&str] = &["AWS", "Sandbox"];

/// One benefit in the seeded catalog, before its per-catalog id is derived.
struct SeededBenefit {
    name: &'static str,
    description: &'static str,
    programs: &'static [&'static str],
    fulfillment_types: &'static [&'static str],
    status: &'static str,
    /// `BenefitRequestSchema` members: the field name and the JSON type a request for
    /// this benefit must put in `BenefitApplicationDetails`.
    schema: &'static [(&'static str, &'static str)],
}

/// The benefits every catalog is seeded with.
///
/// Each `fulfillment_types` entry is a value of the model's `FulfillmentType` enum
/// (`crate::ops::types::FULFILLMENT_TYPE_VALUES`) and each `status` a value of its
/// `BenefitStatus` enum (`crate::ops::types::BENEFIT_STATUS_VALUES`); a `programs` entry
/// satisfies the modelled `Program` constraint (len 1..255, `^[A-Za-z0-9_-]+$`). The set
/// deliberately spans every fulfillment type and both statuses, so a filtered
/// `ListBenefits` has something to include and something to exclude.
const SEEDED_BENEFITS: &[SeededBenefit] = &[
    SeededBenefit {
        name: "Marketing Development Funds",
        description: "Cash funding for partner-led marketing campaigns.",
        programs: &["AWS_PARTNER_NETWORK"],
        fulfillment_types: &["CASH"],
        status: BENEFIT_STATUS_ACTIVE,
        schema: &[("CampaignName", "string"), ("RequestedAmount", "number")],
    },
    SeededBenefit {
        name: "Promotional Credits",
        description: "AWS promotional credits for customer proofs of concept.",
        programs: &["ISV_ACCELERATE"],
        fulfillment_types: &["CREDITS"],
        status: BENEFIT_STATUS_ACTIVE,
        schema: &[("CustomerName", "string"), ("RequestedCredits", "number")],
    },
    SeededBenefit {
        name: "Partner Training Access",
        description: "Seats in AWS partner training and certification programs.",
        programs: &["AWS_PARTNER_NETWORK"],
        fulfillment_types: &["ACCESS"],
        status: BENEFIT_STATUS_ACTIVE,
        schema: &[("SeatCount", "number")],
    },
    SeededBenefit {
        name: "Migration Acceleration Funding",
        description: "Funding and credits for migration engagements.",
        programs: &["MIGRATION_ACCELERATION_PROGRAM"],
        fulfillment_types: &["CASH", "CREDITS"],
        status: BENEFIT_STATUS_ACTIVE,
        schema: &[("WorkloadName", "string"), ("RequestedAmount", "number")],
    },
    SeededBenefit {
        name: "Retired Innovation Sandbox",
        description: "A retired sandbox benefit, kept for applications that reference it.",
        programs: &["ISV_ACCELERATE"],
        fulfillment_types: &["ACCESS"],
        status: BENEFIT_STATUS_INACTIVE,
        schema: &[],
    },
];

/// One benefit allocation the service has granted, before its per-catalog ids and its
/// timestamps are derived.
struct SeededAllocation {
    /// The `name` of the [`SEEDED_BENEFITS`] entry this allocation is based on; the
    /// allocation's `BenefitId`, fulfillment types, description and applicable benefits
    /// all come from that benefit.
    benefit_name: &'static str,
    /// `BenefitAllocationSummary$Name` (modelled length 1..255).
    name: &'static str,
    /// The `BenefitAllocationStatus` the allocation was GRANTED with. The term below may
    /// have closed since, in which case the first read materialises the expiry the clock
    /// already made due — see [`BenefitAllocation::expire_due`].
    status: &'static str,
    /// `BenefitAllocationSummary$StatusReason`, where the status has one to explain.
    status_reason: Option<&'static str>,
    /// The one `FulfillmentType` this allocation is fulfilled with, which must be one of
    /// the types its benefit offers (a benefit offering two is fulfilled with one of them).
    /// It is what decides which member of the modelled `FulfillmentDetails` union
    /// `GetBenefitAllocation` reports — see [`AllocationFulfillment`].
    fulfillment_type: &'static str,
    /// What AWS allocated, in minor units of [`ALLOCATION_CURRENCY`]; zero for an `ACCESS`
    /// allocation, which fulfills no money.
    allocated_amount_minor: i64,
    /// How much of [`Self::allocated_amount_minor`] has been issued (disbursed, or handed
    /// over as credit codes) as of state init. Never more than what was allocated.
    issued_amount_minor: i64,
    /// How long before state init the allocation was created, in days.
    created_days_ago: i64,
    /// The term, in days, from `StartsAt` (= creation) to `ExpiresAt`.
    term_days: i64,
}

/// The benefit allocations every catalog is seeded with.
///
/// The allocation is service-owned data like the benefit catalog it draws from: AWS grants
/// one when it fulfills a partner's benefit, and this API models no operation that creates
/// one, so the set is seeded here and every read reports it from the store.
///
/// Each `status` is a value of the model's `BenefitAllocationStatus` enum
/// (`crate::ops::types::BENEFIT_ALLOCATION_STATUS_VALUES`, i.e. `ACTIVE | INACTIVE |
/// FULFILLED`), each `fulfillment_type` a value of its `FulfillmentType` enum
/// (`crate::ops::types::FULFILLMENT_TYPE_VALUES`) that the named benefit offers, and each
/// `benefit_name` names a row of [`SEEDED_BENEFITS`]. The set deliberately spans every
/// `FulfillmentType` (`CASH`, `CREDITS`, `ACCESS`, and a benefit carrying two of them,
/// fulfilled with the second), an allocation inside its term, one already fulfilled, one
/// whose term closed before state init, an allocation partly issued and one with nothing
/// issued yet — so a filtered `ListBenefitAllocations` has something to include and
/// something to exclude whichever filter it is given, and `GetBenefitAllocation` has one
/// allocation per member of the modelled `FulfillmentDetails` union to report.
const SEEDED_ALLOCATIONS: &[SeededAllocation] = &[
    SeededAllocation {
        benefit_name: "Marketing Development Funds",
        name: "Marketing Development Funds allocation",
        status: ALLOCATION_STATUS_ACTIVE,
        status_reason: None,
        fulfillment_type: FULFILLMENT_TYPE_CASH,
        allocated_amount_minor: 25_000 * MINOR_UNITS_PER_UNIT,
        // Part of the funding has been disbursed, so this allocation carries an issuance.
        issued_amount_minor: 10_000 * MINOR_UNITS_PER_UNIT,
        created_days_ago: 30,
        term_days: 365,
    },
    SeededAllocation {
        benefit_name: "Promotional Credits",
        name: "Promotional Credits allocation",
        status: ALLOCATION_STATUS_FULFILLED,
        status_reason: Some("The allocated credits were issued in full."),
        fulfillment_type: FULFILLMENT_TYPE_CREDITS,
        allocated_amount_minor: 5_000 * MINOR_UNITS_PER_UNIT,
        // Issued in full, which is what `FULFILLED` says: the credit code carries all of it.
        issued_amount_minor: 5_000 * MINOR_UNITS_PER_UNIT,
        created_days_ago: 60,
        term_days: 365,
    },
    SeededAllocation {
        benefit_name: "Partner Training Access",
        name: "Partner Training Access allocation",
        status: ALLOCATION_STATUS_ACTIVE,
        status_reason: None,
        fulfillment_type: FULFILLMENT_TYPE_ACCESS,
        // An `ACCESS` allocation fulfills a privilege, not an amount.
        allocated_amount_minor: 0,
        issued_amount_minor: 0,
        // Granted more than a term ago, so its `ExpiresAt` is already past: the expiry is
        // what moves it to `INACTIVE`, rather than a status seeded as already moved.
        created_days_ago: 400,
        term_days: 365,
    },
    SeededAllocation {
        benefit_name: "Migration Acceleration Funding",
        name: "Migration Acceleration Funding allocation",
        status: ALLOCATION_STATUS_ACTIVE,
        status_reason: None,
        // The benefit offers `CASH` and `CREDITS`; this allocation is fulfilled with the
        // second of them, so the type is the allocation's own choice rather than the
        // benefit's first.
        fulfillment_type: FULFILLMENT_TYPE_CREDITS,
        allocated_amount_minor: 10_000 * MINOR_UNITS_PER_UNIT,
        // Nothing issued yet, so the credit-code list is empty.
        issued_amount_minor: 0,
        created_days_ago: 10,
        term_days: 365,
    },
];

/// File extension -> the `FileType` a `FileDetail` reports for it.
///
/// Every value is one of the model's own `FileType` enum values, whose catalog is
/// `crate::ops::types::FILE_TYPE_VALUES`
/// (`smithy_query(query="shape",shape="FileType")`): the enum is a list of media types,
/// so the file's type is the media type its extension names. An extension not listed
/// here leaves `FileType` absent rather than guessing one.
const FILE_TYPES_BY_EXTENSION: &[(&str, &str)] = &[
    ("csv", "text/csv"),
    ("doc", "application/msword"),
    (
        "docx",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    ),
    ("jpeg", "image/jpeg"),
    ("jpg", "image/jpeg"),
    ("pdf", "application/pdf"),
    ("png", "image/png"),
    (
        "pptx",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    ),
    ("svg", "image/svg+xml"),
    (
        "xlsx",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    ),
];

/// One benefit the service offers: seeded, read-only catalog data.
#[derive(Debug, Clone)]
pub struct Benefit {
    /// `ben-` + 14 `[0-9a-z]` characters, derived from the catalog and the benefit's
    /// name, so the same benefit keeps the same id across runs and two catalogs never
    /// share one.
    pub id: String,
    pub name: String,
    pub description: String,
    pub programs: Vec<String>,
    /// `FulfillmentType` values.
    pub fulfillment_types: Vec<String>,
    pub benefit_request_schema: Option<Document>,
    /// A `BenefitStatus` value.
    pub status: String,
}

impl Benefit {
    /// Whether this benefit offers `fulfillment_type`, which is what an allocation of it
    /// may be fulfilled with (see [`BenefitAllocation::fulfillment_type`]).
    pub fn offers(&self, fulfillment_type: &str) -> bool {
        self.fulfillment_types
            .iter()
            .any(|offered| offered == fulfillment_type)
    }

    /// The fulfillment type an allocation of this benefit is granted with: the FIRST type it
    /// offers — a benefit offering two is fulfilled with one of them and nothing picks which
    /// — and `None` for a benefit that offers none, which can be fulfilled with nothing at
    /// all. The one place that answers this, so no record is written with a type its benefit
    /// does not offer.
    pub fn offered_fulfillment_type(&self) -> Option<&str> {
        self.fulfillment_types.first().map(String::as_str)
    }
}

/// A monetary amount as the store holds it: minor units of its currency, so an amount is
/// an integer in the store and is rendered into the modelled `MonetaryValue$Amount` once,
/// on the way out (see [`Self::amount_text`]).
#[derive(Debug, Clone)]
pub struct Money {
    /// The amount in minor units (cents, for [`ALLOCATION_CURRENCY`]). Never negative:
    /// [`Self::new`] is the only constructor and clamps at zero.
    minor_units: i64,
    /// A `CurrencyCode` value.
    pub currency_code: String,
}

impl Money {
    /// `minor_units` of [`ALLOCATION_CURRENCY`]. A negative figure is not an amount this
    /// service allocates, so it clamps to zero rather than rendering a signed amount.
    pub fn new(minor_units: i64) -> Self {
        Self {
            minor_units: minor_units.max(0),
            currency_code: ALLOCATION_CURRENCY.to_string(),
        }
    }

    /// This amount in minor units of [`Self::currency_code`].
    pub fn minor_units(&self) -> i64 {
        self.minor_units
    }

    /// The modelled `MonetaryValue$Amount` text: the units, then the currency's minor units
    /// as its decimal digits (`"25000.00"` for 2_500_000 cents of USD).
    pub fn amount_text(&self) -> String {
        let units = self.minor_units / MINOR_UNITS_PER_UNIT;
        let fraction = self.minor_units % MINOR_UNITS_PER_UNIT;
        format!("{units}.{fraction:02}")
    }

    /// This amount as the modelled `MonetaryValue`.
    pub fn to_monetary_value(&self) -> crate::ops::types::MonetaryValue {
        crate::ops::types::MonetaryValue {
            amount: self.amount_text(),
            currency_code: self.currency_code.clone(),
        }
    }
}

/// One issuance of value against a benefit allocation: the record the modelled
/// `IssuanceDetail` reports.
#[derive(Debug, Clone)]
pub struct Issuance {
    /// The issuance's own identifier, derived from the allocation's id so it is stable
    /// across reads and distinct per allocation (see [`issuance_id`]).
    pub id: String,
    /// What this issuance handed over.
    pub amount: Money,
    /// When it was processed.
    pub issued_at_millis: i64,
}

/// One AWS promotional credit code issued against a benefit allocation: the record the
/// modelled `CreditCode` reports.
#[derive(Debug, Clone)]
pub struct AllocatedCreditCode {
    /// The AWS account the code can be applied to: the one account this emulator presents.
    pub aws_account_id: String,
    /// The code itself, derived from the allocation's id and this code's index in the
    /// allocation (see [`credit_code_text`]).
    pub code: String,
    /// The code's value.
    pub value: Money,
    /// A `BenefitAllocationStatus` value: `ACTIVE` while the code can still be redeemed,
    /// `FULFILLED` once it has been, `INACTIVE` once it has expired.
    pub status: String,
    pub issued_at_millis: i64,
    /// When the code expires and can no longer be redeemed.
    pub expires_at_millis: i64,
}

impl AllocatedCreditCode {
    /// Whether this code's term has closed at `now_millis`, decided by [`term_closed`] —
    /// the one place the boundary lives — against this code's own `ExpiresAt`.
    pub fn has_expired(&self, now_millis: i64) -> bool {
        term_closed(self.expires_at_millis, now_millis)
    }

    /// Materialise the expiry the clock has already made due on this code, exactly as
    /// [`BenefitAllocation::expire_due`] does for the allocation — the same boundary, read
    /// through [`Self::has_expired`] and so through [`term_closed`]: a code past its own
    /// `ExpiresAt` can no longer be redeemed, so it is `INACTIVE` whether or not anybody
    /// has looked at it. A redeemed (`FULFILLED`) code is terminal.
    ///
    /// Answers WHEN the change fell due — this code's own `ExpiresAt` — so the allocation
    /// that holds it can date the write by that instant rather than by the instant it was
    /// read, and `None` where nothing was owed.
    fn expire_due(&mut self, now_millis: i64) -> Option<i64> {
        if self.status != ALLOCATION_STATUS_ACTIVE || !self.has_expired(now_millis) {
            return None;
        }
        self.status = ALLOCATION_STATUS_INACTIVE.to_string();
        Some(self.expires_at_millis)
    }
}

/// How a benefit allocation is fulfilled, as the store holds it.
///
/// The allocation's `FulfillmentType` decides the shape of this record, which is what the
/// modelled `FulfillmentDetails` union is: one member per kind of fulfillment
/// (`smithy_query(query="shape",shape="FulfillmentDetails")`). `CASH` is fulfilled by
/// disbursing money (`DisbursementDetails`, "direct payments or reimbursements"), `CREDITS`
/// by issuing AWS promotional credits (`CreditDetails`), and `ACCESS` by granting a
/// privilege (`AccessDetails`, "service permissions or feature access"), whose description
/// is the allocation's own — so the variant carries nothing.
///
/// The union's fourth member, `ConsumableDetails` ("usage quotas or service limits"), is
/// produced by no `FulfillmentType` the model declares, so nothing in this store holds one;
/// a fulfillment type with no record here leaves `FulfillmentDetail` absent rather than
/// reporting a kind of fulfillment that did not happen.
#[derive(Debug, Clone)]
pub enum AllocationFulfillment {
    /// `CASH`: what has been disbursed so far, and the issuance that disbursed it (absent
    /// while nothing has been).
    Disbursement {
        disbursed: Money,
        issuance: Option<Issuance>,
    },
    /// `CREDITS`: what was allocated, what has been issued as credit codes, and those
    /// codes (empty while nothing has been issued).
    Credits {
        allocated: Money,
        issued: Money,
        codes: Vec<AllocatedCreditCode>,
    },
    /// `ACCESS`: the privilege itself, described by the allocation's `Description`.
    Access,
}

impl AllocationFulfillment {
    /// The fulfillment record of an allocation of `fulfillment_type` that was granted
    /// `allocated_minor` and has had `issued_minor` of it issued, or `None` for a
    /// fulfillment type this store holds no record shape for.
    ///
    /// `allocation_id` seeds the issuance id and the credit codes, and the allocation's own
    /// `issued_at`/`expires_at` are what an issuance and a code carry: a code expires with
    /// the allocation it was issued against.
    fn new(
        fulfillment_type: &str,
        allocated_minor: i64,
        issued_minor: i64,
        allocation_id: &str,
        issued_at_millis: i64,
        expires_at_millis: i64,
    ) -> Option<Self> {
        let issued = Money::new(issued_minor.clamp(0, allocated_minor.max(0)));
        match fulfillment_type {
            FULFILLMENT_TYPE_CASH => Some(Self::Disbursement {
                issuance: (issued.minor_units() > 0).then(|| Issuance {
                    id: issuance_id(allocation_id),
                    amount: issued.clone(),
                    issued_at_millis,
                }),
                disbursed: issued,
            }),
            FULFILLMENT_TYPE_CREDITS => Some(Self::Credits {
                allocated: Money::new(allocated_minor),
                // One issuance, so one code carrying what has been issued; nothing issued
                // means no codes rather than a zero-valued one.
                codes: Vec::from_iter((issued.minor_units() > 0).then(|| AllocatedCreditCode {
                    aws_account_id: ::engine::emulator_account_id().to_string(),
                    code: credit_code_text(allocation_id, 0),
                    value: issued.clone(),
                    // A code is handed over REDEEMABLE: the redemption that applies it
                    // to a bill is what moves it to `FULFILLED` (see
                    // `redeem_credit_code`), and its own expiry what moves it to
                    // `INACTIVE` (see [`AllocatedCreditCode::expire_due`]).
                    status: ALLOCATION_STATUS_ACTIVE.to_string(),
                    issued_at_millis,
                    expires_at_millis,
                })),
                issued,
            }),
            FULFILLMENT_TYPE_ACCESS => Some(Self::Access),
            _ => None,
        }
    }
}

/// One benefit allocation the service has granted: the record every allocation operation
/// reads.
///
/// Granted by AWS when it fulfills a benefit for a partner — this API models no operation
/// that creates one — so the record arrives from [`SEEDED_ALLOCATIONS`] or from
/// [`ComAmazonawsPartnercentralbenefitsPartnercentralbenefitsserviceState::grant_allocation`],
/// and every operation reports it from here. The ARN is NOT stored: it carries the region
/// the request was signed for, so it is built per read with [`allocation_arn`], exactly as
/// a benefit's is.
#[derive(Debug, Clone)]
pub struct BenefitAllocation {
    /// `benalloc-` + 14 `[0-9a-z]` characters, from the modelled `BenefitAllocationId`.
    pub id: String,
    pub catalog: String,
    /// The `ben-...` id of the benefit this allocation is based on.
    pub benefit_id: String,
    /// The `benappl-...` id of the application that resulted in this allocation, where one
    /// did. Absent on an allocation AWS granted directly, which is what every seeded
    /// allocation is: the model makes the member optional for exactly that case.
    pub benefit_application_id: Option<String>,
    pub name: String,
    pub description: Option<String>,
    /// A `BenefitAllocationStatus` value.
    pub status: String,
    pub status_reason: Option<String>,
    /// The one `FulfillmentType` this allocation is fulfilled with, which is always one of
    /// the types its benefit offers: a benefit may offer two, and the allocation is
    /// fulfilled with one of them. The benefit's offer list is the BENEFIT's own field —
    /// what an allocation carries is the fulfillment it got (see
    /// [`Self::fulfillment_types`]). Every path that records an allocation — the catalog
    /// seeds and
    /// [`ComAmazonawsPartnercentralbenefitsPartnercentralbenefitsserviceState::grant_allocation`]
    /// — takes it from [`Benefit::offered_fulfillment_type`] or records no allocation at
    /// all, and every report reads it through [`Self::fulfillment_types`].
    pub fulfillment_type: String,
    /// How this allocation is fulfilled: the amounts, issuances and credit codes behind
    /// [`Self::fulfillment_type`]. `None` for a fulfillment type this store holds no record
    /// shape for — see [`AllocationFulfillment`].
    pub fulfillment: Option<AllocationFulfillment>,
    /// The `ben-...` ids of the benefits this allocation can be applied to.
    pub applicable_benefit_ids: Vec<String>,
    pub created_at_millis: i64,
    pub updated_at_millis: i64,
    /// When the allocation becomes active and usable.
    pub starts_at_millis: i64,
    /// When the allocation expires and is no longer usable.
    pub expires_at_millis: i64,
}

impl BenefitAllocation {
    /// The `FulfillmentType` values USED for this allocation — the fulfillment it was
    /// granted with, which is the one [`Self::fulfillment_type`] names, so the list holds
    /// exactly that type. It is NOT the offer list of the benefit the allocation is based
    /// on, which may be wider and belongs to the benefit; every report of, and filter on,
    /// an allocation's fulfillment types reads this.
    ///
    /// Empty for an allocation no fulfillment type was recorded for, so such an allocation
    /// reports no fulfillment type rather than an unmodelled empty one.
    pub fn fulfillment_types(&self) -> &[String] {
        if self.fulfillment_type.is_empty() {
            &[]
        } else {
            std::slice::from_ref(&self.fulfillment_type)
        }
    }

    /// Whether this allocation's term has closed at `now_millis`, decided by
    /// [`term_closed`] — the one place the boundary lives — against the allocation's own
    /// `ExpiresAt`.
    pub fn has_expired(&self, now_millis: i64) -> bool {
        term_closed(self.expires_at_millis, now_millis)
    }

    /// Materialise the expiry the clock has already made due, and report whether this call
    /// is what materialised it.
    ///
    /// An `ACTIVE` allocation past its `ExpiresAt` is no longer usable, so it is
    /// `INACTIVE` whether or not anybody has looked at it: the change was owed at that
    /// instant, which is why a reporting operation may apply it. `FULFILLED` is terminal —
    /// the benefit was delivered, so there is nothing left to expire — and an allocation
    /// already `INACTIVE` has nowhere to move. Each credit code the allocation has issued
    /// expires on the same rule — the same [`term_closed`] boundary — against its own
    /// `ExpiresAt` (see [`AllocatedCreditCode::expire_due`]).
    ///
    /// `UpdatedAt` is dated by the instant the change FELL DUE — the `ExpiresAt` the record
    /// already holds, the latest one where this call materialises several — and never by
    /// `now_millis`: nothing happened to the allocation at the instant somebody looked at
    /// it, so dating the write there would report a change at a moment no change was made
    /// and would make the member a function of when the store was read rather than of what
    /// it holds.
    pub fn expire_due(&mut self, now_millis: i64) -> bool {
        let mut due_millis: Option<i64> = None;
        if self.status == ALLOCATION_STATUS_ACTIVE && self.has_expired(now_millis) {
            self.status = ALLOCATION_STATUS_INACTIVE.to_string();
            self.status_reason = Some(ALLOCATION_EXPIRED_REASON.to_string());
            due_millis = Some(self.expires_at_millis);
        }
        if let Some(AllocationFulfillment::Credits { codes, .. }) = self.fulfillment.as_mut() {
            for code in codes.iter_mut() {
                if let Some(code_due_millis) = code.expire_due(now_millis) {
                    due_millis =
                        Some(due_millis.map_or(code_due_millis, |due| due.max(code_due_millis)));
                }
            }
        }
        let Some(due_millis) = due_millis else {
            return false;
        };
        self.updated_at_millis = bump_millis(self.updated_at_millis, due_millis);
        true
    }
}

/// One write a `ClientToken`-carrying operation applied to a benefit application.
///
/// Kept so a replayed `ClientToken` applies nothing a second time and answers with what
/// the first call reported, and as the record of WHICH fields each write changed and
/// why. No modelled output reports it: the model binds no member for it on any output
/// shape, so it is state this service holds rather than something it reports. The new
/// values are not copied here — the fields themselves hold them — which is also what
/// keeps the record small.
#[derive(Debug, Clone)]
pub struct AppliedWrite {
    /// The operation that applied the write, as the model names it. A `ClientToken` is
    /// the identity of ONE request, so a token is a replay only of the same operation.
    pub operation: &'static str,
    /// The request's `ClientToken`.
    pub client_token: String,
    /// The fingerprint of the request the token was spent on ([`request_fingerprint`]),
    /// which is what tells a RETRY of that request from a different request carrying the
    /// same token: the first is answered with what this write reported, the second is
    /// refused — see [`replayed_write`]. [`REQUEST_FINGERPRINT_LEN`] bytes, so the
    /// record keeps no copy of the request itself.
    pub request_fingerprint: String,
    /// Why the write was made, where the request carried a reason of its own (an
    /// `AmendmentReason`) that the record does not already hold elsewhere.
    pub reason: Option<String>,
    /// The fields the write changed: an amendment's `FieldPath`s, or the input members
    /// an update applied, in the order they were applied. At most
    /// [`MAX_AMENDMENTS_PER_REQUEST`] of them, each at most [`MAX_FIELD_PATH_LEN`]
    /// characters.
    pub fields: Vec<String>,
    /// When the write was applied, as epoch milliseconds.
    pub applied_at_millis: i64,
    /// The revision the write produced — the `Revision` a replay of the token reports.
    pub revision: u64,
}

/// One `ClientToken` a write spent, with the fingerprint of the request that spent it.
///
/// The pair is what an operation whose idempotency window is the resource's own state —
/// rather than the applied-write record — judges a repeated token against: the token says
/// WHICH request, the fingerprint says whether this is that same request.
#[derive(Debug, Clone)]
pub struct SpentToken {
    /// The request's `ClientToken`, at most [`MAX_CLIENT_TOKEN_LEN`] bytes.
    pub client_token: String,
    /// The fingerprint of the request it was spent on ([`request_fingerprint`]).
    pub request_fingerprint: String,
}

/// One stored benefit application: the single source of truth for every application
/// operation.
#[derive(Debug, Clone)]
pub struct BenefitApplication {
    pub id: String,
    pub arn: String,
    pub catalog: String,
    /// The `ben-...` id of the benefit this application requests.
    pub benefit_id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    /// `FulfillmentType` values, as requested on the create — each distinct value once,
    /// in the order the create first named it (see [`distinct_values`]).
    pub fulfillment_types: Vec<String>,
    pub benefit_application_details: Option<Document>,
    /// The partner programs this application belongs to: the programs of the benefit it
    /// requests, not a caller-supplied value — the model binds no `Programs` member on
    /// any application input.
    pub programs: Vec<String>,
    /// A `BenefitApplicationStatus` value. The workflow `Stage` this application reports
    /// is this status's position in the processing workflow, computed by
    /// [`BenefitApplication::stage`] rather than stored beside it, so the two cannot
    /// disagree.
    pub status: String,
    /// Why the application holds the status it holds, when the transition into it carried
    /// a reason. Replaced by every transition (see [`BenefitApplication::set_status`]), so
    /// it never explains a status the application has left. Between
    /// [`MIN_STATUS_REASON_LEN`] and [`MAX_STATUS_REASON_LEN`] characters, the bounds of
    /// the member it is reported through and of the inputs that write it, so a read never
    /// answers a reason longer than that member admits — and a recorded reason always
    /// says something rather than being present and empty.
    pub status_reason: Option<String>,
    pub status_reason_codes: Vec<String>,
    pub created_at_millis: i64,
    pub updated_at_millis: i64,
    /// The optimistic-concurrency revision, rendered on the wire by
    /// [`BenefitApplication::revision_text`].
    pub revision: u64,
    /// The revision the create minted. Replaying the create's `ClientToken` answers with
    /// THIS revision, so a retry sees the first call's response even if the application
    /// has been updated since.
    pub created_revision: u64,
    /// The `ClientToken` of the recall that put this application back into
    /// `PENDING_SUBMISSION`, if one did and it is still in draft, with the fingerprint of
    /// the request that recall carried.
    ///
    /// `RecallBenefitApplication` documents its `ClientToken` as what makes the recall
    /// idempotent, so a retry carrying the token of the recall still in effect answers
    /// success and changes nothing, while a DIFFERENT request carrying that token is
    /// refused rather than answered success — see [`BenefitApplication::recall_retry`].
    /// One token per application, so it is bounded by the application count; a fresh
    /// submission makes the application recallable again, at which point the token no
    /// longer matches a recall in effect.
    pub recall_client_token: Option<SpentToken>,
    /// The AWS resources this application is linked to, as a SET: each distinct ARN once,
    /// in the order it was first stored, whether by the create (see [`distinct_values`])
    /// or by `AssociateBenefitApplicationResource`. At most
    /// [`MAX_ASSOCIATED_RESOURCES_PER_APPLICATION`] of them, which is therefore a count of
    /// distinct resources, and each at most [`MAX_ASSOCIATED_RESOURCE_ARN_LEN`] characters
    /// — the length the model publishes for this value — whichever write stored it (see
    /// [`associated_resources_refusal`]).
    pub associated_resources: Vec<String>,
    pub partner_contacts: Vec<Contact>,
    pub file_details: Vec<FileDetail>,
    pub tags: BTreeMap<String, String>,
    /// The writes a `ClientToken`-carrying operation applied to this application, oldest
    /// first, at most [`MAX_APPLIED_WRITE_HISTORY`] of them. It is what makes those
    /// operations idempotent in their token — see [`BenefitApplication::applied_write`].
    pub applied_writes: Vec<AppliedWrite>,
}

impl BenefitApplication {
    /// The current revision as the modelled `Revision` string.
    pub fn revision_text(&self) -> String {
        self.revision.to_string()
    }

    /// The revision the create returned, as the modelled `Revision` string.
    pub fn created_revision_text(&self) -> String {
        self.created_revision.to_string()
    }

    /// Advance `UpdatedAt` past the previous write.
    ///
    /// The timestamp half of recording a write: every change to the stored application
    /// also moves its revision, so a handler records its write with
    /// [`BenefitApplication::revise`], which advances both.
    pub fn touch(&mut self, now_millis: i64) {
        self.updated_at_millis = bump_millis(self.updated_at_millis, now_millis);
    }

    /// This application's modelled `Stage`: the stage of the processing workflow its
    /// current status puts it at (see [`stage_for_status`]).
    pub fn stage(&self) -> Option<String> {
        stage_for_status(&self.status).map(str::to_string)
    }

    /// Move this application to `status`, with `reason` as the explanation of the status
    /// it is moving INTO.
    ///
    /// `StatusReason` (and the `StatusReasonCodes` beside it) say why the application
    /// holds the status it holds, so a transition replaces them: a transition the caller
    /// gives no reason for leaves none behind rather than keeping the previous status's
    /// explanation. The workflow `Stage` needs no write at all — it follows the status
    /// itself (see [`BenefitApplication::stage`]).
    ///
    /// Neither `UpdatedAt` nor the revision is touched here: the caller records the write
    /// with [`BenefitApplication::touch`] or [`BenefitApplication::revise`], whichever
    /// that transition is.
    pub fn set_status(&mut self, status: &str, reason: Option<String>) {
        self.status = status.to_string();
        self.status_reason = reason;
        self.status_reason_codes.clear();
    }

    /// Whether a recall may take this application back to draft: true exactly for the
    /// statuses in [`RECALLABLE_STATUSES`].
    pub fn is_recallable(&self) -> bool {
        RECALLABLE_STATUSES.contains(&self.status.as_str())
    }

    /// Whether `UpdateBenefitApplication` may rewrite this application's members: true
    /// exactly for the statuses in [`UPDATABLE_STATUSES`].
    pub fn is_updatable(&self) -> bool {
        UPDATABLE_STATUSES.contains(&self.status.as_str())
    }

    /// Whether AWS has finished with this application: true exactly for the statuses in
    /// [`TERMINAL_STATUSES`]. No change to its content is accepted in one.
    pub fn is_terminal(&self) -> bool {
        TERMINAL_STATUSES.contains(&self.status.as_str())
    }

    /// Whether `client_token` is the token of the recall that is still in effect on this
    /// application — i.e. a retry of that same recall, which must change nothing — or the
    /// refusal that token was spent on a DIFFERENT recall request.
    ///
    /// Takes the token itself, never an `Option`, so a request that carries none cannot
    /// be read as a retry of a recall that never happened: the caller passes a token only
    /// when the request has one. `request_fingerprint` is the fingerprint of the request
    /// carrying it, judged exactly as [`replayed_write`] judges one against an applied
    /// write.
    pub fn recall_retry(
        &self,
        client_token: &str,
        request_fingerprint: &str,
    ) -> Result<bool, crate::ops::types::ConflictException> {
        let Some(spent) = self.recall_client_token.as_ref() else {
            return Ok(false);
        };
        if self.status != STATUS_PENDING_SUBMISSION || spent.client_token != client_token {
            return Ok(false);
        }
        if spent.request_fingerprint != request_fingerprint {
            return Err(idempotency_conflict());
        }
        Ok(true)
    }

    /// Record a change to the stored application: bump the revision and advance
    /// `UpdatedAt`.
    ///
    /// The revision is the application's optimistic-concurrency token — the `Revision` a
    /// write has to carry to be accepted — so EVERY call that changes the stored record
    /// moves it: a change to the application's own content, and equally a status
    /// transition the caller drove, because a caller holding the revision it read before
    /// that change no longer holds the current one.
    ///
    /// Any such change also ENDS a recall's idempotency window: the record the recall
    /// left has moved on, so replaying that recall must not answer as a retry of it. A
    /// recall itself records its token after calling this.
    pub fn revise(&mut self, now_millis: i64) {
        self.recall_client_token = None;
        self.revision = self.revision.saturating_add(1);
        self.touch(now_millis);
    }

    /// The write `operation` already applied to this application under `client_token`,
    /// if this application still holds a record of it — the request a retry carrying
    /// that token is a replay of, which must change nothing and answer with what the
    /// first call reported.
    ///
    /// The operation is part of the key: a `ClientToken` identifies ONE request, so the
    /// same token spent on a different operation is a different request rather than a
    /// retry of this one.
    pub fn applied_write(&self, operation: &str, client_token: &str) -> Option<&AppliedWrite> {
        self.applied_writes
            .iter()
            .find(|applied| applied.operation == operation && applied.client_token == client_token)
    }

    /// Record one applied write, dropping the oldest once the record is full.
    pub fn record_write(&mut self, applied: AppliedWrite) {
        self.applied_writes.push(applied);
        if self.applied_writes.len() > MAX_APPLIED_WRITE_HISTORY {
            self.applied_writes.remove(0);
        }
    }

    /// A list member, omitted from the wire when nothing was ever stored for it. The one
    /// implementation is the free [`optional_list`], which the allocation summaries reach
    /// directly.
    pub fn optional_list<T: Clone>(values: &[T]) -> Option<Vec<T>> {
        optional_list(values)
    }

    /// `BenefitApplicationSummary$BenefitApplicationDetails`, which the model types as
    /// `Attributes` (`map<String, String>`) while the full record carries a `Document`.
    ///
    /// The summary therefore reports the stored document's top-level members, each
    /// rendered by [`document_text`]. A document that is not an object has no top-level
    /// members, so the summary omits the member rather than inventing one.
    pub fn summary_details(&self) -> Option<HashMap<String, String>> {
        let Document::Object(members) = self.benefit_application_details.as_ref()? else {
            return None;
        };
        Some(
            members
                .iter()
                .map(|(name, value)| (name.clone(), document_text(value)))
                .collect(),
        )
    }

    /// Whether this application carries at least one of `arns` in its associated
    /// resources.
    ///
    /// `arns` are the entries a filter the caller SUPPLIED resolved to (see
    /// [`supplied_filter`]), so an empty `arns` — a supplied filter naming no resource —
    /// matches nothing. A filter the caller did not supply is `None` there and this is
    /// never asked about it.
    pub fn matches_resource_arns(&self, arns: &[String]) -> bool {
        arns.iter()
            .any(|wanted| self.associated_resources.contains(wanted))
    }
}

/// What one spent `CreateBenefitApplication` `ClientToken` holds: the application that
/// create minted, and the fingerprint of the request it was spent on.
///
/// The fingerprint is what tells a RETRY of that create from a different create carrying
/// the same token: the first is answered with the minted id, ARN and revision, the second
/// is refused with [`idempotency_conflict`] rather than reported as a success for an
/// application it never created. [`REQUEST_FINGERPRINT_LEN`] bytes, so the index keeps no
/// copy of the request.
#[derive(Debug, Clone)]
pub struct ClientTokenRecord {
    /// The catalog the create minted the application in. Part of the record rather than
    /// of the key: the token identifies the request, and the catalog the request named is
    /// one of the members its fingerprint covers.
    pub catalog: String,
    /// The `benappl-...` id of the application the create minted.
    pub application_id: String,
    /// The fingerprint of the create request ([`request_fingerprint`]).
    pub request_fingerprint: String,
}

/// What a replayed `ClientToken` answers with: the application the write it was spent on
/// landed on, and the revision that write reported.
///
/// Owned rather than borrowed, so a handler can judge a repeated token against the whole
/// store and then take the application for writing.
#[derive(Debug, Clone)]
pub struct ReplayedWrite {
    /// The `benappl-...` id of the application the write landed on.
    pub id: String,
    /// That application's ARN.
    pub arn: String,
    /// The revision the write produced — the `Revision` a replay reports.
    pub revision: u64,
}

/// One benefit catalog: its seeded benefits and allocations, and the applications created
/// in it.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    /// Benefits by `ben-...` id, in one stable order so `ListBenefits` can page.
    pub benefits: BTreeMap<String, Benefit>,
    /// Applications by `benappl-...` id, in one stable order so
    /// `ListBenefitApplications` can page.
    pub applications: BTreeMap<String, BenefitApplication>,
    /// Allocations by `benalloc-...` id, in one stable order so
    /// `ListBenefitAllocations` can page.
    pub allocations: BTreeMap<String, BenefitAllocation>,
}

impl Catalog {
    /// The catalog named `catalog`, with [`SEEDED_BENEFITS`] and the
    /// [`SEEDED_ALLOCATIONS`] drawn from them in it, as of `seeded_at_millis`.
    fn seeded(catalog: &str, seeded_at_millis: i64) -> Self {
        let benefits: BTreeMap<String, Benefit> = SEEDED_BENEFITS
            .iter()
            .map(|seed| {
                let id = benefit_id(catalog, seed.name);
                let benefit = Benefit {
                    id: id.clone(),
                    name: seed.name.to_string(),
                    description: seed.description.to_string(),
                    programs: seed.programs.iter().map(|p| (*p).to_string()).collect(),
                    fulfillment_types: seed
                        .fulfillment_types
                        .iter()
                        .map(|t| (*t).to_string())
                        .collect(),
                    benefit_request_schema: benefit_request_schema(seed.schema),
                    status: seed.status.to_string(),
                };
                (id, benefit)
            })
            .collect();
        // Each seeded allocation names the benefit it is based on, so a row whose benefit
        // is not in the catalog — or one naming a fulfillment type that benefit does not
        // offer — seeds nothing rather than an allocation no read can make sense of.
        let allocations = SEEDED_ALLOCATIONS
            .iter()
            .filter_map(|seed| {
                let benefit = benefits
                    .values()
                    .find(|benefit| benefit.name == seed.benefit_name)?;
                if !benefit.offers(seed.fulfillment_type) {
                    return None;
                }
                let created_at_millis =
                    seeded_at_millis - seed.created_days_ago * MILLIS_PER_DAY;
                let expires_at_millis = created_at_millis + seed.term_days * MILLIS_PER_DAY;
                let id = allocation_id(catalog, seed.name);
                let allocation = BenefitAllocation {
                    id: id.clone(),
                    catalog: catalog.to_string(),
                    benefit_id: benefit.id.clone(),
                    // Seeded allocations are granted directly, not through an
                    // application, so they carry no `BenefitApplicationId`.
                    benefit_application_id: None,
                    name: seed.name.to_string(),
                    description: Some(benefit.description.clone()),
                    status: seed.status.to_string(),
                    status_reason: seed.status_reason.map(str::to_string),
                    fulfillment_type: seed.fulfillment_type.to_string(),
                    fulfillment: AllocationFulfillment::new(
                        seed.fulfillment_type,
                        seed.allocated_amount_minor,
                        seed.issued_amount_minor,
                        &id,
                        created_at_millis,
                        expires_at_millis,
                    ),
                    applicable_benefit_ids: applicable_benefit_ids(&benefits, benefit),
                    created_at_millis,
                    // Nothing has changed a seeded allocation since it was granted; an
                    // expiry that falls due is what moves this on.
                    updated_at_millis: created_at_millis,
                    starts_at_millis: created_at_millis,
                    expires_at_millis,
                };
                Some((id, allocation))
            })
            .collect();
        Self {
            benefits,
            applications: BTreeMap::new(),
            allocations,
        }
    }

    /// The id of the benefit `identifier` names, accepting either form the modelled
    /// `BenefitId` pattern allows (`^(arn:.+|ben-[0-9a-z]{14})$`): the bare id, or the
    /// benefit ARN this service issues in `region`. `None` when this catalog holds no
    /// such benefit.
    pub fn resolve_benefit_id(
        &self,
        catalog: &str,
        region: &str,
        identifier: &str,
    ) -> Option<String> {
        let id = resolve_identifier(catalog, region, identifier, BENEFIT_RESOURCE)?;
        self.benefits.contains_key(&id).then_some(id)
    }

    /// The id of the benefit allocation `identifier` names, accepting either form the
    /// modelled `BenefitAllocationIdentifier` pattern allows
    /// (`^(arn:.+|benalloc-[0-9a-z]{14})$`): the bare id, or the allocation ARN this
    /// service issues in `region`. `None` when this catalog holds no such allocation —
    /// which is also the answer for an ARN this service does not issue for the record,
    /// one of another catalog, another partition, region or account included.
    pub fn resolve_allocation_id(
        &self,
        catalog: &str,
        region: &str,
        identifier: &str,
    ) -> Option<String> {
        let id = resolve_identifier(catalog, region, identifier, ALLOCATION_RESOURCE)?;
        self.allocations.contains_key(&id).then_some(id)
    }
}

/// The resource id `identifier` names inside `catalog`, for a resource of `resource`'s
/// type: `identifier` itself when it is a bare id, or the ARN's id segment when it is the
/// ARN this service issues for a resource of that type in that catalog and `region`.
///
/// One construction for every identifier member whose modelled pattern admits both forms
/// (`^(arn:.+|<prefix>-[0-9a-z]{14})$`), so an ARN that is not one this service issues for
/// the record — another catalog, another service, another resource type, or another
/// partition, region or account — resolves to nothing rather than to the record that
/// happens to share its id segment, exactly as the ARN index does for the records that
/// keep a minted ARN ([`ComAmazonawsPartnercentralbenefitsPartnercentralbenefitsserviceState::resolve_application_id`]).
/// Whether a record of that id EXISTS is the caller's own check.
fn resolve_identifier(
    catalog: &str,
    region: &str,
    identifier: &str,
    resource: &str,
) -> Option<String> {
    if !identifier.starts_with("arn:") {
        return Some(identifier.to_string());
    }
    let (arn_catalog, arn_id) = parse_resource_arn(identifier, region, resource)?;
    (arn_catalog == catalog).then(|| arn_id.to_string())
}

/// The `ben-...` ids of the benefits an allocation based on `base` can be applied to: the
/// catalog's `ACTIVE` benefits that share at least one partner program with it, and `base`
/// itself whatever its status.
///
/// `ApplicableBenefitIds` is "the identifiers of the benefits applicable for this
/// allocation", and what makes a benefit applicable is the program the allocation was
/// granted under; a benefit that is no longer offered cannot be applied for, so an
/// `INACTIVE` one is not applicable — except the allocation's own benefit, which the
/// allocation already applies to however the catalog has moved on since. Walked over the
/// catalog's id-keyed map, so the list is in one stable order.
fn applicable_benefit_ids(benefits: &BTreeMap<String, Benefit>, base: &Benefit) -> Vec<String> {
    benefits
        .values()
        .filter(|candidate| {
            candidate.id == base.id
                || (candidate.status == BENEFIT_STATUS_ACTIVE
                    && candidate
                        .programs
                        .iter()
                        .any(|program| base.programs.contains(program)))
        })
        .map(|candidate| candidate.id.clone())
        .collect()
}

/// The resource-type segment a benefit ARN carries.
pub const BENEFIT_RESOURCE: &str = "benefit";
/// The resource-type segment a benefit-application ARN carries.
pub const APPLICATION_RESOURCE: &str = "benefit-application";
/// The resource-type segment a benefit-allocation ARN carries, from the modelled
/// `BenefitAllocation` resource's own ARN template
/// (`catalog/{CatalogName}/benefit-allocation/benalloc-{BenefitAllocationId}`, see
/// `smithy_query(query="shape",shape="BenefitAllocation")`).
pub const ALLOCATION_RESOURCE: &str = "benefit-allocation";

/// Per-service state for com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice. A handler reaches it through [`service_state`].
///
/// BOUNDED: `CreateBenefitApplication` refuses a create that would take a catalog past
/// [`MAX_APPLICATIONS_PER_CATALOG`], the catalog set is fixed by [`SEEDED_CATALOGS`],
/// `arns` and `client_tokens` carry at most one entry per stored application, one
/// application records at most [`MAX_APPLIED_WRITE_HISTORY`] applied writes, and a catalog holds
/// at most [`MAX_ALLOCATIONS_PER_CATALOG`] allocations, and every caller string a record
/// holds carries a length — the model's own where it states one, on whichever path writes
/// the value ([`associated_resources_refusal`]), and this service's where it states none
/// ([`unconstrained_members_refusal`], [`details_requirement`]) — so no caller can grow
/// this store without limit.
#[derive(Debug)]
pub struct ComAmazonawsPartnercentralbenefitsPartnercentralbenefitsserviceState {
    /// Every catalog, keyed by the `Catalog` input string.
    pub catalogs: BTreeMap<String, Catalog>,
    /// The AWS accounts that hold an active AWS partner benefit — the whole input to the
    /// entitlement gate ([`entitlement_refusal`], and [`write_request_refusal`] on a
    /// writing operation).
    ///
    /// EMPTY at state init, and deliberately so: being a registered AWS partner is a
    /// property of an account that this API cannot establish — it models no partner
    /// registration — so no account is entitled until something outside the API records
    /// one. Seeding an entitled account here would make the gate unobservable, which is
    /// the defect this field exists to close.
    partner_accounts: BTreeSet<String>,
    /// Application ARN -> (catalog, application id), so an `Identifier` given as an ARN
    /// resolves to the same record as the bare id.
    pub arns: HashMap<String, (String, String)>,
    /// The `ClientToken` of every `CreateBenefitApplication` that succeeded -> what that
    /// create minted and the request it came on.
    ///
    /// ONE namespace for the whole service, not one per catalog: a `ClientToken`
    /// identifies one REQUEST of the operation, and the `Catalog` the request named is one
    /// of the members its fingerprint covers — so a create repeating a spent token in
    /// another catalog is a different request, which only a store-wide index can tell. At
    /// most one entry per stored application, so it is bounded by the application count.
    pub client_tokens: BTreeMap<String, ClientTokenRecord>,
    /// Monotonic counter feeding [`Self::mint_application_id`]; it is what makes a minted
    /// id unique even within one clock tick.
    id_sequence: u64,
}

impl Default for ComAmazonawsPartnercentralbenefitsPartnercentralbenefitsserviceState {
    /// The benefit catalog is service-owned — the API models no `CreateBenefit` — so it
    /// is seeded here, where the engine first touches this service's state.
    fn default() -> Self {
        Self {
            catalogs: {
                // One instant for every catalog, so two catalogs' seeded allocations
                // carry the same timestamps rather than drifting by the seeding loop.
                let seeded_at_millis = now_millis();
                SEEDED_CATALOGS
                    .iter()
                    .map(|name| ((*name).to_string(), Catalog::seeded(name, seeded_at_millis)))
                    .collect()
            },
            partner_accounts: BTreeSet::new(),
            arns: HashMap::new(),
            client_tokens: BTreeMap::new(),
            id_sequence: 0,
        }
    }
}

impl ComAmazonawsPartnercentralbenefitsPartnercentralbenefitsserviceState {
    /// Whether `account` holds an active AWS partner benefit, which is what the
    /// entitlement gate admits a request on.
    pub fn has_active_partner_benefit(&self, account: &str) -> bool {
        self.partner_accounts.contains(account)
    }

    /// Record an active AWS partner benefit for `account`, so the gate admits its
    /// requests. `true` when this call is what recorded it.
    ///
    /// No modelled operation reaches this: the API has none that registers a partner, so
    /// entitlement arrives from outside the API — which in this emulator means an
    /// in-process caller holding the shared engine (a test standing in for an entitled
    /// partner account). Nothing on the wire can grant itself the entitlement.
    pub fn grant_partner_entitlement(&mut self, account: &str) -> bool {
        self.partner_accounts.insert(account.to_string())
    }

    /// The entitlement gate, judged on a store the caller already holds the lock on:
    /// `Some(..)` when the one account this emulator presents holds no active AWS partner
    /// benefit, so the request is refused rather than served.
    ///
    /// The one construction of the denial — [`entitlement_refusal`] takes the lock itself
    /// and then asks this, so a handler that has already locked the store (because it
    /// resolved the catalog first) applies the very same decision without re-entering the
    /// lock.
    pub fn entitlement_refusal(&self) -> Option<crate::ops::types::AccessDeniedException> {
        (!self.has_active_partner_benefit(::engine::emulator_account_id())).then(|| {
            crate::ops::types::AccessDeniedException {
                message: PARTNER_ENTITLEMENT_REQUIRED.to_string(),
            }
        })
    }

    /// The catalog a request names, or the refusal it is answered with.
    ///
    /// Every operation binds a required `Catalog` and every resource this service holds
    /// lives inside one, so a request whose catalog does not exist has NO resource to look
    /// up: the missing resource is the catalog, and that is what the answer names — through
    /// [`not_found_message`], the one construction. ONE resolution for every operation that
    /// binds the member: the lookup and the refusal it fails with are the same step, so an
    /// operation that goes on to read the catalog reads the record this resolution found
    /// rather than resolving the key again, and no operation can report a resource inside a
    /// catalog that is not there.
    pub fn catalog(
        &self,
        catalog: &str,
    ) -> Result<&Catalog, crate::ops::types::ResourceNotFoundException> {
        self.catalogs.get(catalog).ok_or_else(|| {
            crate::ops::types::ResourceNotFoundException {
                message: not_found_message(CATALOG_KIND, catalog),
            }
        })
    }

    /// The refusal a request naming a catalog this store does not hold is answered with,
    /// or `None` when the catalog exists: [`Self::catalog`]'s refusal alone, for the
    /// operations that only need to know whether the catalog is there.
    pub fn catalog_refusal(
        &self,
        catalog: &str,
    ) -> Option<crate::ops::types::ResourceNotFoundException> {
        self.catalog(catalog).err()
    }

    /// The application `identifier` names inside `catalog`, accepting either form the
    /// modelled `BenefitApplicationIdentifier` allows (`^(arn:.+|benappl-[0-9a-z]{14})$`).
    ///
    /// An ARN must match a stored ARN exactly, so an ARN naming another partition,
    /// region or account resolves to nothing rather than to the application that happens
    /// to share its id segment — and an ARN whose catalog is not the one the request
    /// named resolves to nothing either.
    pub fn resolve_application_id(&self, catalog: &str, identifier: &str) -> Option<String> {
        if identifier.starts_with("arn:") {
            return self
                .arns
                .get(identifier)
                .filter(|(arn_catalog, _)| arn_catalog == catalog)
                .map(|(_, id)| id.clone());
        }
        self.catalogs
            .get(catalog)
            .filter(|stored| stored.applications.contains_key(identifier))
            .map(|_| identifier.to_string())
    }

    /// The application `identifier` names, for a reporting operation.
    pub fn application(&self, catalog: &str, identifier: &str) -> Option<&BenefitApplication> {
        let id = self.resolve_application_id(catalog, identifier)?;
        self.catalogs.get(catalog)?.applications.get(&id)
    }

    /// The application `identifier` names, or the refusal a request naming one this store
    /// does not hold is answered with — the rule [`Self::catalog`] states, for the
    /// application resource.
    ///
    /// ONE resolution for every reporting operation that binds an application identifier:
    /// the lookup ([`Self::application`]) and the refusal it fails with are the same step,
    /// so no operation can pick a different resource kind for the message or quote a
    /// different identifier than the one the request named.
    pub fn require_application(
        &self,
        catalog: &str,
        identifier: &str,
    ) -> Result<&BenefitApplication, crate::ops::types::ResourceNotFoundException> {
        self.application(catalog, identifier)
            .ok_or_else(|| application_not_found(identifier))
    }

    /// The identity of the application `identifier` names, as a request fingerprint covers
    /// it ([`request_fingerprint`]).
    ///
    /// A fingerprint is the identity of the REQUEST, and an identifier member names the
    /// RECORD it resolves to rather than the spelling it was written with, so what the
    /// fingerprint carries is the stored `benappl-...` id: both forms the modelled
    /// identifier admits name one request, so a retry spelled the other way is the SAME
    /// request and is replayed. An identifier naming no stored application is its own
    /// identity, so such a request is a different request from any a token was spent on —
    /// refused as the reuse it is where the token is spent, and answered by the
    /// operation's own `ResourceNotFoundException` where it is not.
    pub fn application_identity(&self, catalog: &str, identifier: &str) -> String {
        self.resolve_application_id(catalog, identifier)
            .unwrap_or_else(|| identifier.to_string())
    }

    /// The identity of the benefit `identifier` names, as a request fingerprint covers it:
    /// the same rule [`Self::application_identity`] states, for the member whose modelled
    /// pattern is `^(arn:.+|ben-[0-9a-z]{14})$`. `region` is the one the request addressed,
    /// which is what an ARN spelling is resolved against ([`resolve_identifier`]).
    pub fn benefit_identity(&self, catalog: &str, region: &str, identifier: &str) -> String {
        self.catalogs
            .get(catalog)
            .and_then(|stored| stored.resolve_benefit_id(catalog, region, identifier))
            .unwrap_or_else(|| identifier.to_string())
    }

    /// The write `operation` already applied under `client_token`, WHEREVER in this store
    /// it landed, or the refusal that token was spent on a different request.
    ///
    /// A `ClientToken` identifies ONE request of an operation, not one request per record,
    /// so the token is looked up across every application this store holds rather than only
    /// on the one the request names: a request repeating a spent token with any different
    /// member — the application it names included — is a different request and is refused
    /// ([`replayed_write`]), while the same request is answered with what the first call
    /// reported. `Ok(None)` is a token no write of this operation holds, so the request is
    /// a new one.
    ///
    /// The walk is bounded by the store's own caps: at most
    /// [`MAX_APPLICATIONS_PER_CATALOG`] applications in each of the
    /// [`SEEDED_CATALOGS`], each holding at most [`MAX_APPLIED_WRITE_HISTORY`] writes.
    pub fn replayed_application_write(
        &self,
        operation: &str,
        client_token: &str,
        request_fingerprint: &str,
    ) -> Result<Option<ReplayedWrite>, crate::ops::types::ConflictException> {
        let Some((application, applied)) = self
            .catalogs
            .values()
            .flat_map(|stored| stored.applications.values())
            .find_map(|application| {
                application
                    .applied_write(operation, client_token)
                    .map(|applied| (application, applied))
            })
        else {
            return Ok(None);
        };
        let Some(applied) = replayed_write(Some(applied), request_fingerprint)? else {
            return Ok(None);
        };
        Ok(Some(ReplayedWrite {
            id: application.id.clone(),
            arn: application.arn.clone(),
            revision: applied.revision,
        }))
    }

    /// Whether `client_token` is the token of a recall still in effect on an application of
    /// this store — a retry of that recall, which must change nothing — or the refusal that
    /// token was spent on a DIFFERENT recall request.
    ///
    /// The store-wide lookup is the rule [`Self::replayed_application_write`] applies for
    /// the writes an application records: the token names one request, so a recall naming
    /// another application under a spent token is refused rather than served.
    pub fn replayed_recall(
        &self,
        client_token: &str,
        request_fingerprint: &str,
    ) -> Result<bool, crate::ops::types::ConflictException> {
        for application in self
            .catalogs
            .values()
            .flat_map(|stored| stored.applications.values())
        {
            if application.recall_retry(client_token, request_fingerprint)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The application the create that spent `client_token` minted, or the refusal that
    /// token was spent on a different create.
    ///
    /// `Ok(None)` is a token no create holds, so the request is a new one. The index the
    /// token is looked up in is the whole store's ([`Self::client_tokens`]), so the
    /// `Catalog` a create named is a member its fingerprint can differ on, exactly as the
    /// application an application-write names is.
    ///
    /// A token this index holds always names an application this store holds — the write
    /// path records the two together or records neither ([`Self::insert_application`]) — so
    /// `Ok(None)` cannot mean "a spent token whose application is gone".
    pub fn replayed_create(
        &self,
        client_token: &str,
        request_fingerprint: &str,
    ) -> Result<Option<&BenefitApplication>, crate::ops::types::ConflictException> {
        let Some(spent) = self.client_tokens.get(client_token) else {
            return Ok(None);
        };
        if spent.request_fingerprint != request_fingerprint {
            return Err(idempotency_conflict());
        }
        Ok(self
            .catalogs
            .get(&spent.catalog)
            .and_then(|stored| stored.applications.get(&spent.application_id)))
    }

    /// The benefit `identifier` names inside `catalog`, for a reporting operation.
    ///
    /// Resolved as [`Catalog::resolve_benefit_id`] does, so both forms the modelled
    /// `GetBenefit$Identifier` pattern admits (`^(arn:.+|ben-[0-9a-z]{14})$`) name the same
    /// record. `None` when the catalog does not exist or holds no such benefit — which is
    /// also the answer for an ARN this service does not issue for the record in `region`,
    /// one of another catalog, another service, another resource type, another partition or
    /// another account included.
    pub fn benefit(&self, catalog: &str, region: &str, identifier: &str) -> Option<&Benefit> {
        let stored = self.catalogs.get(catalog)?;
        let id = stored.resolve_benefit_id(catalog, region, identifier)?;
        stored.benefits.get(&id)
    }

    /// The `BenefitRequestSchema` of the benefit the application `identifier` names in
    /// `catalog` is for, CLONED — so an operation can judge the document it would store
    /// while holding that application for writing.
    ///
    /// `None` when no such application is stored, or when its benefit declares no schema:
    /// neither constrains the document at all (see [`benefit_schema_refusal`]).
    ///
    /// The benefit is reached by the id the application STORES, which is this service's own
    /// `ben-...` id rather than caller text, so no identifier resolution is involved.
    pub fn benefit_schema_for_application(
        &self,
        catalog: &str,
        identifier: &str,
    ) -> Option<Document> {
        let benefit_id = self.application(catalog, identifier)?.benefit_id.clone();
        self.catalogs
            .get(catalog)?
            .benefits
            .get(&benefit_id)?
            .benefit_request_schema
            .clone()
    }

    /// The allocation `identifier` names inside `catalog`, for a reporting operation.
    ///
    /// Resolved as [`Catalog::resolve_allocation_id`] does, so both forms the modelled
    /// `BenefitAllocationIdentifier` admits name the same record. `None` when the catalog
    /// does not exist or holds no such allocation.
    pub fn allocation(
        &self,
        catalog: &str,
        region: &str,
        identifier: &str,
    ) -> Option<&BenefitAllocation> {
        let stored = self.catalogs.get(catalog)?;
        let id = stored.resolve_allocation_id(catalog, region, identifier)?;
        stored.allocations.get(&id)
    }

    /// The application `identifier` names, for an operation the spec says mutates it.
    ///
    /// `id`, `arn` and `catalog` are this record's identity — the store is keyed on the
    /// id and `arns` on the ARN — so none of them may be reassigned through this handle.
    /// No operation needs to: the model binds no member that moves an application
    /// between catalogs.
    pub fn application_mut(
        &mut self,
        catalog: &str,
        identifier: &str,
    ) -> Option<&mut BenefitApplication> {
        let id = self.resolve_application_id(catalog, identifier)?;
        self.catalogs.get_mut(catalog)?.applications.get_mut(&id)
    }

    /// The application `identifier` names, for an operation the spec says mutates it, or
    /// the refusal a request naming one this store does not hold is answered with: what
    /// [`Self::require_application`] is for a reporting operation, over the writing handle
    /// [`Self::application_mut`] hands out.
    pub fn require_application_mut(
        &mut self,
        catalog: &str,
        identifier: &str,
    ) -> Result<&mut BenefitApplication, crate::ops::types::ResourceNotFoundException> {
        self.application_mut(catalog, identifier)
            .ok_or_else(|| application_not_found(identifier))
    }

    /// Store a newly created application and index it by ARN and by `ClientToken`.
    ///
    /// The token is indexed with the fingerprint of the create request it came on
    /// ([`request_fingerprint`]), so the create can tell a retry of that request from a
    /// different request carrying the same token, and the index is the store's own rather
    /// than one catalog's, because the `Catalog` the request named is one of the members
    /// the fingerprint covers (see [`Self::replayed_create`]).
    ///
    /// The record and both index entries are ONE step, taken only once the catalog the
    /// application lives in is known to be there: when it is not — which a handler has
    /// already refused — NOTHING is stored, neither the application nor the ARN entry nor
    /// the spent token. So an index of this store never names an application the store does
    /// not hold: `arns` and `client_tokens` keep at most one entry per stored application,
    /// the bound this store is finite by, and [`Self::replayed_create`] cannot read a
    /// stranded token as either a spent one or a new request.
    pub fn insert_application(
        &mut self,
        client_token: String,
        request_fingerprint: String,
        application: BenefitApplication,
    ) {
        let Some(catalog) = self.catalogs.get_mut(&application.catalog) else {
            return;
        };
        let catalog_name = application.catalog.clone();
        let (arn, id) = (application.arn.clone(), application.id.clone());
        catalog.applications.insert(id.clone(), application);
        self.arns.insert(arn, (catalog_name.clone(), id.clone()));
        self.client_tokens.insert(
            client_token,
            ClientTokenRecord {
                catalog: catalog_name,
                application_id: id,
                request_fingerprint,
            },
        );
    }

    /// Record an allocation AWS granted for a benefit, and report the id it minted.
    ///
    /// `None` when the catalog does not exist, when it holds no benefit `benefit_identifier`
    /// names, when `application_identifier` names no application of it, when that benefit
    /// offers no fulfillment type to be fulfilled with, or when the catalog already holds
    /// [`MAX_ALLOCATIONS_PER_CATALOG`] allocations. Both identifiers accept
    /// either form their modelled patterns allow (a bare id or an ARN), resolved exactly as
    /// every operation resolves them — and since a grant arrives on no request, an ARN is
    /// resolved against [`DEFAULT_REGION`], which is the region an ARN carries when no
    /// request names one ([`request_region`]).
    ///
    /// No modelled operation reaches this, and deliberately so: granting an allocation is
    /// what AWS's own fulfillment workflow does once it approves a partner's application,
    /// and this API models no operation that creates one — just as it models no partner
    /// registration, which is why [`Self::grant_partner_entitlement`] is reached the same
    /// way. So a grant arrives from OUTSIDE the API, which in this emulator means an
    /// in-process caller holding the shared engine; nothing on the wire can grant itself an
    /// allocation.
    ///
    /// The allocation runs for [`ALLOCATION_TERM_MILLIS`] from `now_millis`, and its
    /// description and applicable benefits are the benefit's own rather than values the
    /// caller supplies. It is fulfilled with the FIRST type its benefit offers — a benefit offering two is fulfilled with one of them and nothing here names
    /// which — for [`GRANTED_ALLOCATION_AMOUNT_MINOR`], of which nothing has been issued
    /// yet.
    pub fn grant_allocation(
        &mut self,
        catalog: &str,
        benefit_identifier: &str,
        application_identifier: Option<&str>,
        name: &str,
        now_millis: i64,
    ) -> Option<String> {
        let benefit_id = self
            .catalogs
            .get(catalog)?
            .resolve_benefit_id(catalog, DEFAULT_REGION, benefit_identifier)?;
        let benefit_application_id = match application_identifier {
            Some(identifier) => Some(self.resolve_application_id(catalog, identifier)?),
            None => None,
        };
        if self.catalogs.get(catalog)?.allocations.len() >= MAX_ALLOCATIONS_PER_CATALOG {
            return None;
        }
        let stored = self.catalogs.get(catalog)?;
        let benefit = stored.benefits.get(&benefit_id)?.clone();
        let applicable_benefit_ids = applicable_benefit_ids(&stored.benefits, &benefit);
        // An allocation carries the fulfillment it GOT, which is one of the types its
        // benefit offers ([`Benefit::offered_fulfillment_type`]), so a benefit offering none
        // can be fulfilled with nothing and grants no allocation at all — exactly as the
        // catalog seeds no allocation whose fulfillment type its benefit does not offer,
        // rather than recording a value no modelled `FulfillmentType` declares. Asked
        // BEFORE an id is minted, so a grant that records nothing spends no id.
        let fulfillment_type = benefit.offered_fulfillment_type()?.to_string();
        let id = self.mint_allocation_id(now_millis);
        let expires_at_millis = now_millis.saturating_add(ALLOCATION_TERM_MILLIS);
        let allocation = BenefitAllocation {
            id: id.clone(),
            catalog: catalog.to_string(),
            benefit_id,
            benefit_application_id,
            name: name.to_string(),
            description: Some(benefit.description.clone()),
            status: ALLOCATION_STATUS_ACTIVE.to_string(),
            status_reason: None,
            fulfillment: AllocationFulfillment::new(
                &fulfillment_type,
                GRANTED_ALLOCATION_AMOUNT_MINOR,
                0,
                &id,
                now_millis,
                expires_at_millis,
            ),
            fulfillment_type,
            applicable_benefit_ids,
            created_at_millis: now_millis,
            updated_at_millis: now_millis,
            starts_at_millis: now_millis,
            expires_at_millis,
        };
        self.catalogs
            .get_mut(catalog)?
            .allocations
            .insert(id.clone(), allocation);
        Some(id)
    }

    /// Materialise, in `catalog`, every allocation expiry `now_millis` has already made
    /// due — see [`BenefitAllocation::expire_due`] for why a reporting operation may.
    pub fn expire_allocations_due(&mut self, catalog: &str, now_millis: i64) {
        if let Some(catalog) = self.catalogs.get_mut(catalog) {
            for allocation in catalog.allocations.values_mut() {
                allocation.expire_due(now_millis);
            }
        }
    }

    /// The resource a tagging operation's `resourceArn` names, for a report of its tags.
    ///
    /// The tagging operations (`TagResource`, `UntagResource`, `ListTagsForResource`)
    /// name their resource by ARN alone — they bind no `Catalog` member — so the lookup
    /// goes through the same ARN index [`Self::resolve_application_id`] uses, and an ARN
    /// naming another partition, region or account resolves to nothing rather than to
    /// the resource that happens to share its id segment.
    ///
    /// The benefit application is the one resource of this service that holds a tag set:
    /// it is the only shape with a `Tags` member (`CreateBenefitApplicationInput$Tags`),
    /// while a benefit and a benefit allocation carry none on any input or output. An
    /// ARN of any other resource type therefore resolves to no taggable resource.
    pub fn taggable_resource(&self, resource_arn: &str) -> Option<&BenefitApplication> {
        let (catalog, id) = self.arns.get(resource_arn)?;
        self.catalogs.get(catalog)?.applications.get(id)
    }

    /// The resource a tagging operation's `resourceArn` names, for a call that changes
    /// its tag set. Resolved exactly as [`Self::taggable_resource`].
    pub fn taggable_resource_mut(
        &mut self,
        resource_arn: &str,
    ) -> Option<&mut BenefitApplication> {
        let (catalog, id) = self.arns.get(resource_arn)?.clone();
        self.catalogs.get_mut(&catalog)?.applications.get_mut(&id)
    }

    /// The resource a tagging operation's `resourceArn` names, or the refusal a call
    /// naming one this store does not hold is answered with — the rule [`Self::catalog`]
    /// states, for the tagging operations' resource.
    ///
    /// ONE resolution for every reporting tagging operation: the lookup
    /// ([`Self::taggable_resource`]) and the refusal it fails with are the same step,
    /// through the single construction [`taggable_resource_not_found`], so no tagging
    /// operation can word a failed lookup differently than its siblings.
    pub fn require_taggable_resource(
        &self,
        resource_arn: &str,
    ) -> Result<&BenefitApplication, crate::ops::types::ResourceNotFoundException> {
        self.taggable_resource(resource_arn)
            .ok_or_else(taggable_resource_not_found)
    }

    /// The resource a tagging operation's `resourceArn` names, for a call that changes its
    /// tag set, or the refusal a call naming one this store does not hold is answered
    /// with: what [`Self::require_taggable_resource`] is for a reporting operation, over
    /// the writing handle [`Self::taggable_resource_mut`] hands out.
    pub fn require_taggable_resource_mut(
        &mut self,
        resource_arn: &str,
    ) -> Result<&mut BenefitApplication, crate::ops::types::ResourceNotFoundException> {
        self.taggable_resource_mut(resource_arn)
            .ok_or_else(taggable_resource_not_found)
    }

    /// A `benappl-[0-9a-z]{14}` id no stored application is using.
    pub fn mint_application_id(&mut self, now_millis: i64) -> String {
        self.mint_id(
            APPLICATION_ID_PREFIX,
            now_millis,
            Self::holds_application,
            Self::application_count,
        )
    }

    /// A `benalloc-[0-9a-z]{14}` id no stored allocation is using.
    pub fn mint_allocation_id(&mut self, now_millis: i64) -> String {
        self.mint_id(
            ALLOCATION_ID_PREFIX,
            now_millis,
            Self::holds_allocation,
            Self::allocation_count,
        )
    }

    /// An id of `prefix`'s kind that no stored record of that kind is using, where `holds`
    /// answers whether one is and `count` how many there are.
    ///
    /// The sequence number makes two ids minted in the same millisecond differ, and the
    /// mixing step keeps them from being consecutive, so a caller cannot derive the next
    /// id from the last.
    fn mint_id(
        &mut self,
        prefix: &str,
        now_millis: i64,
        holds: fn(&Self, &str) -> bool,
        count: fn(&Self) -> usize,
    ) -> String {
        for _ in 0..1024 {
            self.id_sequence = self.id_sequence.wrapping_add(1);
            let seed = splitmix64((now_millis as u64) ^ self.id_sequence.rotate_left(17));
            let candidate = format!("{prefix}{}", id_body(seed));
            if !holds(self, &candidate) {
                return candidate;
            }
        }
        // Bounded fallback, so a pathological clock can never spin here: the
        // zero-padded decimals are also `[0-9a-z]{14}`, and there are always more of
        // them than there are stored records.
        (0u64..=count(self) as u64)
            .map(|n| format!("{prefix}{n:014}"))
            .find(|candidate| !holds(self, candidate))
            .unwrap_or_else(|| format!("{prefix}{:014}", count(self)))
    }

    fn holds_application(&self, id: &str) -> bool {
        self.catalogs
            .values()
            .any(|catalog| catalog.applications.contains_key(id))
    }

    fn application_count(&self) -> usize {
        self.catalogs
            .values()
            .map(|catalog| catalog.applications.len())
            .sum()
    }

    fn holds_allocation(&self, id: &str) -> bool {
        self.catalogs
            .values()
            .any(|catalog| catalog.allocations.contains_key(id))
    }

    fn allocation_count(&self) -> usize {
        self.catalogs
            .values()
            .map(|catalog| catalog.allocations.len())
            .sum()
    }
}

/// The `BenefitRequestSchema` document for a seeded benefit's field list, or `None` when
/// the benefit declares no fields.
///
/// A JSON-Schema-shaped object: `{"type":"object","properties":{"<field>":{"type":"<t>"}}}`.
fn benefit_request_schema(fields: &[(&str, &str)]) -> Option<Document> {
    if fields.is_empty() {
        return None;
    }
    let properties: HashMap<String, Document> = fields
        .iter()
        .map(|(name, json_type)| {
            let mut property = HashMap::new();
            property.insert(
                "type".to_string(),
                Document::String((*json_type).to_string()),
            );
            ((*name).to_string(), Document::Object(property))
        })
        .collect();
    let mut schema = HashMap::new();
    schema.insert("type".to_string(), Document::String("object".to_string()));
    schema.insert("properties".to_string(), Document::Object(properties));
    Some(Document::Object(schema))
}

/// A document rendered as one string, for the `map<String, String>` the application
/// summary models: a string is itself, a scalar is its literal text, and a nested array
/// or object is its compact JSON text.
pub fn document_text(value: &Document) -> String {
    match value {
        Document::String(text) => text.clone(),
        other => document_json(other, MAX_DETAILS_DEPTH),
    }
}

/// `value` as compact JSON text, with the levels this walk may still descend — the text
/// a CLIENT reads ([`document_text`]).
fn document_json(value: &Document, budget: usize) -> String {
    render_document(value, budget, json_number)
}

/// `value` as the CANONICAL text one value renders as and no other does, with the levels
/// this walk may still descend — the text a request's identity is taken over
/// ([`request_fingerprint`]).
///
/// It is compact JSON text with one difference: a number carries the JSON TYPE it was
/// read as beside its digits ([`canonical_number`]), because the digits alone do not tell
/// an integer from the float that equals it, while this crate's own notion of one value —
/// `Document` equality, which is what a write compares a supplied value with the stored
/// one by ([`same_value`]) — does.
fn document_canonical(value: &Document, budget: usize) -> String {
    render_document(value, budget, canonical_number)
}

/// A number as the JSON text that renders its value.
fn json_number(number: &Number) -> String {
    match number {
        Number::PosInt(n) => n.to_string(),
        Number::NegInt(n) => n.to_string(),
        // Written by this crate's JSON writer, so a float keeps the decimal point that
        // tells it from an integer (`1.0`, not the `1` its plain decimal text would be);
        // JSON has no literal for a non-finite float, which is what `null` is here.
        Number::Float(n) => ::serde_json::Number::from_f64(*n)
            .map_or_else(|| "null".to_string(), |number| number.to_string()),
    }
}

/// A number as the text [`document_canonical`] renders it: the JSON type it was read as,
/// then its exact value.
///
/// The type is part of the text because the digits are not enough: the plain decimal text
/// of an integer and of the float that equals it is the same text, so without the type two
/// documents this crate holds as different values would render alike and a request
/// carrying one would pass for a request carrying the other.
fn canonical_number(number: &Number) -> String {
    match number {
        Number::PosInt(n) => format!("u{n}"),
        Number::NegInt(n) => format!("i{n}"),
        // `{:?}` writes the shortest text that reads back as this float, so the value is
        // rendered exactly; `+ 0.0` first, so the negative zero — a float that EQUALS the
        // zero it is written against — renders as that zero does.
        Number::Float(n) => format!("f{:?}", n + 0.0),
    }
}

/// `value` as compact JSON text with `number` rendering each number, with the levels this
/// walk may still descend.
///
/// Bounded by `budget` so the recursion cannot exhaust the stack on a caller-supplied
/// document; a create or update already refused anything deeper than
/// [`MAX_DETAILS_DEPTH`], so the exhausted arm is unreachable for a stored document.
fn render_document(value: &Document, budget: usize, number: fn(&Number) -> String) -> String {
    let Some(next) = budget.checked_sub(1) else {
        return "null".to_string();
    };
    match value {
        Document::Null => "null".to_string(),
        Document::Bool(flag) => flag.to_string(),
        Document::Number(value) => number(value),
        Document::String(text) => {
            // `serde_json` is this crate's JSON writer; a string is the one scalar that
            // needs escaping.
            ::serde_json::Value::String(text.clone()).to_string()
        }
        Document::Array(items) => {
            let rendered: Vec<String> = items
                .iter()
                .map(|item| render_document(item, next, number))
                .collect();
            format!("[{}]", rendered.join(","))
        }
        Document::Object(members) => {
            // Sorted, so the rendering of one document is stable between reads.
            let mut rendered: Vec<String> = members
                .iter()
                .map(|(name, member)| {
                    format!(
                        "{}:{}",
                        ::serde_json::Value::String(name.clone()),
                        render_document(member, next, number)
                    )
                })
                .collect();
            rendered.sort();
            format!("{{{}}}", rendered.join(","))
        }
    }
}

/// A request member's value as the [`Document`] a request fingerprint covers.
///
/// One implementation per member type a `ClientToken`-carrying input of this service
/// carries, so a fingerprint covers every member a request supplied whatever that
/// member's shape is: a string, a document, or a list of either or of a modelled
/// structure.
pub trait FingerprintValue {
    /// This value as the document [`request_fingerprint`] renders.
    fn fingerprint_document(&self) -> Document;
}

impl FingerprintValue for String {
    fn fingerprint_document(&self) -> Document {
        Document::String(self.clone())
    }
}

impl FingerprintValue for Document {
    fn fingerprint_document(&self) -> Document {
        self.clone()
    }
}

impl FingerprintValue for BTreeMap<String, String> {
    /// A map as the OBJECT its entries denote: one member per key, so neither the order
    /// the entries were written in nor a key written twice is part of the value — exactly
    /// what a map stores. The canonical rendering sorts an object's members
    /// ([`request_fingerprint`]), so the document of one map is stable between reads.
    fn fingerprint_document(&self) -> Document {
        Document::Object(
            self.iter()
                .map(|(key, value)| (key.clone(), Document::String(value.clone())))
                .collect(),
        )
    }
}

impl FingerprintValue for Tag {
    fn fingerprint_document(&self) -> Document {
        fingerprint_object([("Key", Some(self.key.clone())), ("Value", Some(self.value.clone()))])
    }
}

impl FingerprintValue for Contact {
    fn fingerprint_document(&self) -> Document {
        fingerprint_object([
            ("Email", self.email.clone()),
            ("FirstName", self.first_name.clone()),
            ("LastName", self.last_name.clone()),
            ("BusinessTitle", self.business_title.clone()),
            ("Phone", self.phone.clone()),
        ])
    }
}

impl FingerprintValue for FileInput {
    fn fingerprint_document(&self) -> Document {
        fingerprint_object([
            ("FileURI", Some(self.file_uri.clone())),
            ("BusinessUseCase", self.business_use_case.clone()),
        ])
    }
}

impl FingerprintValue for Amendment {
    fn fingerprint_document(&self) -> Document {
        fingerprint_object([
            ("FieldPath", Some(self.field_path.clone())),
            ("NewValue", Some(self.new_value.clone())),
        ])
    }
}

impl<T: FingerprintValue> FingerprintValue for Vec<T> {
    /// A list in REQUEST ORDER: the order is part of what the caller sent (an
    /// amendment list is applied in it), so two requests differing only in it are
    /// different requests.
    fn fingerprint_document(&self) -> Document {
        Document::Array(
            self.iter()
                .map(FingerprintValue::fingerprint_document)
                .collect(),
        )
    }
}

/// The members of one modelled structure a request carried, as a document: a member the
/// structure did not carry is left out, exactly as [`request_fingerprint`] leaves out an
/// absent input member.
fn fingerprint_object<const N: usize>(members: [(&str, Option<String>); N]) -> Document {
    Document::Object(
        members
            .into_iter()
            .filter_map(|(name, value)| value.map(|value| (name.to_string(), Document::String(value))))
            .collect(),
    )
}

/// `value` as the document a request fingerprint covers — the one call a handler makes
/// per supplied member (`input.name.as_ref().map(fingerprint_value)`).
pub fn fingerprint_value<T: FingerprintValue>(value: &T) -> Document {
    value.fingerprint_document()
}

/// A supplied list of IDENTITIES as a request fingerprint covers it: the SET it denotes.
///
/// A member a handler CANONICALISES on the way into the store is fingerprinted by the
/// value it stores, not by the spelling it was written with — the same rule that covers
/// an identifier member by the record it names. Here the canonical value is
/// [`distinct_values`], so two requests whose stored record is identical fingerprint
/// alike and the second is the retry its `ClientToken` says it is.
///
/// `None` when the request omitted the member, so an omitted list and a supplied one
/// still never fingerprint alike (see [`request_fingerprint`]).
pub fn fingerprint_identity_list(values: Option<&[String]>) -> Option<Document> {
    values.map(|values| fingerprint_value(&distinct_values(Some(values.to_vec()))))
}

/// A supplied list of TAGS as a request fingerprint covers it: the key-to-value MAP it
/// denotes ([`tag_map`]), which is the value the store keeps.
///
/// The same canonicalisation rule [`fingerprint_identity_list`] states: a tag list and a
/// reordering of it, or one repeating a key the later entry already gives its value,
/// store the one map, so they are one request rather than a mismatch.
///
/// `None` when the request omitted the member.
pub fn fingerprint_tag_set(tags: Option<&Vec<Tag>>) -> Option<Document> {
    tags.map(|tags| fingerprint_value(&tag_map(Some(tags))))
}

/// Whether two values of one modelled shape are the SAME value, compared through the
/// canonical document each renders as ([`fingerprint_value`]) so the comparison does not
/// depend on how the value was spelled.
///
/// What a write needs to tell a change from a no-op: a member carrying the value the
/// record already holds changes nothing, and a change is what moves the revision (see
/// [`BenefitApplication::revise`]).
pub fn same_value<T: FingerprintValue>(stored: &T, supplied: &T) -> bool {
    stored.fingerprint_document() == supplied.fingerprint_document()
}

/// The fingerprint of one request's parameters: the identity a repeated `ClientToken` is
/// judged against, so a token already spent replays its first answer only for the SAME
/// request.
///
/// `members` are the input members the operation carries other than the token itself,
/// each as the document the request supplied for it ([`fingerprint_value`]) or `None`
/// when the request omitted it. An omitted member is left out of the digest, so omitting
/// a member and supplying one never fingerprint alike, and the rendering is canonical
/// ([`document_canonical`] sorts an object's members), so two spellings of one request
/// agree.
///
/// What the rendering is canonical FOR is the identity: one value renders as one text, and
/// two values this crate holds as different values ([`same_value`]) render as different
/// texts — which is why it is [`document_canonical`] and not the JSON text a client reads,
/// where a number's digits alone would let an integer stand for the float that equals it.
///
/// A member the operation CANONICALISES on the way into the store is passed as the value
/// it stores rather than as the text it was written with — an identifier as the record it
/// names, a list of identities as the set it denotes ([`fingerprint_identity_list`]), a
/// tag list as the map it denotes ([`fingerprint_tag_set`]) — so two requests the store
/// would keep identically are one request and the second replays the first's answer.
///
/// The digest is hex SHA-256: a fixed [`REQUEST_FINGERPRINT_LEN`] bytes however large
/// the request was, which is what lets a spent token be remembered without holding a
/// copy of the request.
pub fn request_fingerprint(members: Vec<(&str, Option<Document>)>) -> String {
    let supplied: HashMap<String, Document> = members
        .into_iter()
        .filter_map(|(name, value)| value.map(|value| (name.to_string(), value)))
        .collect();
    // One level deeper than a stored document may nest, so the wrapper object below
    // cannot cost the deepest member of a document the handlers already accepted.
    let canonical = document_canonical(&Document::Object(supplied), MAX_DETAILS_DEPTH + 1);
    hex::encode(Sha256::digest(canonical.as_bytes()))
}

/// The refusal a request that spends another request's `ClientToken` is answered with —
/// see [`IDEMPOTENT_PARAMETER_MISMATCH`]. One construction, so every operation that
/// takes a token refuses the reuse in the same words.
pub fn idempotency_conflict() -> crate::ops::types::ConflictException {
    crate::ops::types::ConflictException {
        message: IDEMPOTENT_PARAMETER_MISMATCH.to_string(),
    }
}

/// The write a request is a replay of, or the refusal its `ClientToken` was spent on a
/// different request.
///
/// `applied` is what the store holds for this operation under the request's token.
/// `Ok(Some(..))` is a true retry — same operation, same token, same parameters — which
/// the caller answers with what that write reported, applying nothing a second time.
/// `Ok(None)` is a token no write of this operation holds, so the request is a new one.
/// `Err` is the mismatch: the token identifies a request this one is not.
pub fn replayed_write<'a>(
    applied: Option<&'a AppliedWrite>,
    request_fingerprint: &str,
) -> Result<Option<&'a AppliedWrite>, crate::ops::types::ConflictException> {
    match applied {
        Some(applied) if applied.request_fingerprint == request_fingerprint => Ok(Some(applied)),
        Some(_) => Err(idempotency_conflict()),
        None => Ok(None),
    }
}

/// The requirement a `BenefitApplicationDetails` document nested past
/// [`MAX_DETAILS_DEPTH`] fails, worded like every other requirement this service states
/// ([`crate::service::member_clause`]).
///
/// One construction, so the depth bound reads the same whether the document was refused
/// by the guard ([`details_requirement`]) or by the schema walk that stops at the same
/// depth ([`schema_violation`]).
pub fn details_depth_requirement() -> String {
    format!("Member must nest no deeper than {MAX_DETAILS_DEPTH} levels")
}

/// The requirement a supplied or amended `BenefitApplicationDetails` fails, or `None`
/// when the document can be stored.
///
/// Three rules, each an emulator resource guard on a member the model leaves entirely
/// caller-controlled: the document must nest no deeper than [`MAX_DETAILS_DEPTH`], hold
/// no more than [`MAX_DETAILS_MEMBERS`] members over the whole tree, and carry no more
/// than [`MAX_DETAILS_TEXT_LEN`] of text. Each is stated as a REQUIREMENT on the member —
/// never as a sentence of its own — so [`details_refusal`] can render it through the
/// same clause and envelope every other refusal of this service goes through. The text
/// bound is a length, so it goes through the one length check this service makes for
/// itself ([`count_length_clause`]): it is counted in the unit that check counts in —
/// CHARACTERS — and worded by the words it words a length with, so the number the
/// refusal names is the number of characters the caller may send.
pub fn details_requirement(details: Option<&Document>) -> Option<String> {
    let document = details?;
    if !document_fits(document, MAX_DETAILS_DEPTH) {
        return Some(details_depth_requirement());
    }
    // Counted only after the depth check, so this walk's recursion is already bounded.
    let (members, characters) = document_weight(document);
    if members > MAX_DETAILS_MEMBERS {
        return Some(format!(
            "Member must hold no more than {MAX_DETAILS_MEMBERS} members"
        ));
    }
    count_length_clause(characters, 0, MAX_DETAILS_TEXT_LEN)
}

/// The refusal a request whose `BenefitApplicationDetails` breaks one of the document
/// guards is answered with, or `None` when the document can be stored.
///
/// `member` is the input member the document arrived on, spelled as the wire spells it,
/// so the clause names the member the caller must fix — which is why an operation binding
/// no document member of its own states the requirement ([`details_requirement`]) against
/// an input member its caller DID send instead of calling this. The exception is built by
/// the one construction every member refusal of this crate goes through
/// ([`member_validation_exception`], which renders the message with
/// [`crate::service::member_clause`] and [`crate::service::validation_envelope`]), so a
/// client that parses a clause to learn which member and which requirement failed reads
/// this refusal exactly as it reads one the centralized walk produced, and NOTHING of the
/// rejected document is echoed.
///
/// Every operation that BINDS the member applies this — the create and the update to the
/// document they were handed — so one rule is refused in one form wherever it holds;
/// `AmendBenefitApplication` applies the same rule ([`details_requirement`]) to the
/// document its amendments produced and names the amendment that produced it.
pub fn details_refusal(
    member: &str,
    details: Option<&Document>,
) -> Option<crate::ops::types::ValidationException> {
    let requirement = details_requirement(details)?;
    Some(member_validation_exception(
        member,
        &requirement,
        VALIDATION_REASON_FIELD,
    ))
}

/// How many members and how many CHARACTERS of text `value` holds, over the whole tree:
/// the two sizes [`details_requirement`] bounds.
///
/// A member is an object member or an array element; the text counted is the member names
/// and the string values, which is all of the document a caller chooses the SIZE of (a
/// number or a boolean has none). It is counted in characters, the unit every length this
/// service enforces is counted in ([`length_clause`]), so the bound
/// [`details_requirement`] states as a length is the length it enforces whatever the
/// UTF-8 width of the caller's text.
fn document_weight(value: &Document) -> (usize, usize) {
    match value {
        Document::Null | Document::Bool(_) | Document::Number(_) => (0, 0),
        Document::String(text) => (0, text.chars().count()),
        Document::Array(items) => items.iter().fold((0, 0), |(members, characters), item| {
            let (item_members, item_characters) = document_weight(item);
            (members + 1 + item_members, characters + item_characters)
        }),
        Document::Object(entries) => {
            entries
                .iter()
                .fold((0, 0), |(members, characters), (name, member)| {
                    let (member_members, member_characters) = document_weight(member);
                    (
                        members + 1 + member_members,
                        characters + name.chars().count() + member_characters,
                    )
                })
        }
    }
}

/// Whether `value` nests no deeper than `budget` levels.
fn document_fits(value: &Document, budget: usize) -> bool {
    let Some(next) = budget.checked_sub(1) else {
        return false;
    };
    match value {
        Document::Array(items) => items.iter().all(|item| document_fits(item, next)),
        Document::Object(members) => members.values().all(|member| document_fits(member, next)),
        _ => true,
    }
}

/// The input member the create and the update carry an application-details document on,
/// spelled as the wire spells it: the member their refusals of that document name. An
/// operation that binds no document member names an input member of its own instead.
pub const DETAILS_MEMBER: &str = "BenefitApplicationDetails";

/// The input member a refusal of the benefit a create applies for names, spelled as the
/// wire spells it (`CreateBenefitApplicationInput$BenefitIdentifier`).
pub const BENEFIT_IDENTIFIER_MEMBER: &str = "BenefitIdentifier";

/// The `BenefitRequestSchema` keyword naming the JSON type a position must have.
const SCHEMA_TYPE: &str = "type";

/// The `BenefitRequestSchema` keyword naming the fields an object position declares, each
/// mapped to the schema that field's value must satisfy.
const SCHEMA_PROPERTIES: &str = "properties";

/// Why a `BenefitApplicationDetails` document does not satisfy the requested benefit's
/// `BenefitRequestSchema`: the member path the mismatch sits at, and the requirement that
/// path failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaViolation {
    /// The input member the document arrived on, spelled as the wire spells it, followed by
    /// the document's own `.`-separated path to the field that does not satisfy the schema.
    ///
    /// It is the member path a refusal names where the operation BINDS the document (the
    /// create and the update): the leading input member is re-cased to the walk's own
    /// spelling where the refusal is rendered ([`member_validation_exception`]), so a caller
    /// of those operations reads it exactly as it reads a path the centralized walk
    /// reported. An operation that binds no document member reads the POSITION out of it
    /// instead, to name an input member its own caller sent.
    pub member: String,
    /// The requirement that field failed, worded like every other requirement this
    /// service states (see [`crate::service::member_clause`]).
    pub requirement: String,
}

/// The refusal a request whose `details` do not satisfy `schema` is answered with, or
/// `None` when they do.
///
/// `schema` is the stored [`Benefit::benefit_request_schema`] of the benefit the
/// application is for — the schema definition that describes the fields a request for
/// that benefit carries — so what it declares about a field is what the document must
/// put there. A benefit that declares no schema constrains the document not at all, and
/// neither does a position the schema says nothing about: a field the schema does not
/// declare is the caller's own.
///
/// The member is modelled optional wherever it is written, so a request that supplies NO
/// document has nothing to judge; a document it does supply is judged as a whole — which is
/// why the create and the update apply this to the document they would store, and
/// `AmendBenefitApplication` applies the rule itself ([`benefit_schema_violation`]) to the
/// document its amendments produced, naming the amendment that produced it.
pub fn benefit_schema_refusal(
    schema: Option<&Document>,
    details: Option<&Document>,
) -> Option<crate::ops::types::ValidationException> {
    let violation = benefit_schema_violation(schema, details)?;
    Some(member_validation_exception(
        &violation.member,
        &violation.requirement,
        VALIDATION_REASON_FIELD,
    ))
}

/// Why `details` does not satisfy `schema`, or `None` when it does: the rule
/// [`benefit_schema_refusal`] answers a request with.
pub fn benefit_schema_violation(
    schema: Option<&Document>,
    details: Option<&Document>,
) -> Option<SchemaViolation> {
    schema_violation(DETAILS_MEMBER, schema?, details?, MAX_DETAILS_DEPTH)
}

/// Why `value`, at member path `member`, does not satisfy `schema`.
///
/// The keywords read are the ones a `BenefitRequestSchema` of this catalog carries (see
/// [`benefit_request_schema`]): `type`, the JSON type the position must have, and
/// `properties`, the schema each declared field's value must satisfy. A keyword this walk
/// does not read constrains nothing, the way an unrecognised JSON Schema keyword does.
/// Bounded by `budget`, like every other walk over a caller-supplied document, so the
/// recursion cannot exhaust the stack.
fn schema_violation(
    member: &str,
    schema: &Document,
    value: &Document,
    budget: usize,
) -> Option<SchemaViolation> {
    // Nothing below the depth this service stores can be judged, so the exhausted walk
    // REFUSES rather than passing a document it did not read: the depth bound and this
    // walk cannot drift into letting one through. A document that deep is already refused
    // by [`details_requirement`], which every caller of this applies first.
    let Some(next) = budget.checked_sub(1) else {
        return Some(SchemaViolation {
            member: member.to_string(),
            requirement: details_depth_requirement(),
        });
    };
    let Document::Object(keywords) = schema else {
        return None;
    };
    if let Some(Document::String(declared)) = keywords.get(SCHEMA_TYPE) {
        if json_type_of(value) != declared.as_str() {
            return Some(SchemaViolation {
                member: member.to_string(),
                requirement: format!("Member must be of type {declared}"),
            });
        }
    }
    let (Some(Document::Object(properties)), Document::Object(supplied)) =
        (keywords.get(SCHEMA_PROPERTIES), value)
    else {
        return None;
    };
    // Sorted by field name, so which mismatch a document carrying two of them is refused
    // for does not depend on the schema map's iteration order.
    let mut declared: Vec<(&String, &Document)> = properties.iter().collect();
    declared.sort_by_key(|(field, _)| *field);
    declared.into_iter().find_map(|(field, field_schema)| {
        // A field the document does not carry is nothing to judge: the schema states what
        // a value put there must be, and no value was put there.
        let field_value = supplied.get(field)?;
        schema_violation(
            &format!("{member}.{field}"),
            field_schema,
            field_value,
            next,
        )
    })
}

/// The JSON type `value` has, named as JSON Schema's own `type` keyword names the types —
/// so every number is a `number`, whichever Smithy number it arrived as.
fn json_type_of(value: &Document) -> &'static str {
    match value {
        Document::Null => "null",
        Document::Bool(_) => "boolean",
        Document::Number(_) => "number",
        Document::String(_) => "string",
        Document::Array(_) => "array",
        Document::Object(_) => "object",
    }
}

/// The refusal an application for `benefit` is answered with when the catalog no longer
/// offers it, or `None` when it is a benefit a partner can still apply for.
///
/// `BenefitStatus` is `ACTIVE | INACTIVE`
/// (`smithy_query(query="shape",shape="BenefitStatus")`), and a benefit that is not
/// `ACTIVE` is no longer offered, so nothing can be allocated against it and no
/// application for it can be served — the same reading of the status that keeps such a
/// benefit out of an allocation's `ApplicableBenefitIds` (see [`applicable_benefit_ids`]).
/// `member` is the input member that named it, spelled as the wire spells it; the
/// requirement states the status the record is in, which is the service's own data, and
/// echoes nothing of the value the caller sent.
pub fn benefit_not_offered_refusal(
    member: &str,
    benefit: &Benefit,
) -> Option<crate::ops::types::ValidationException> {
    if benefit.status == BENEFIT_STATUS_ACTIVE {
        return None;
    }
    let requirement = format!(
        "Member must name a benefit the catalog offers, which a {} benefit is not",
        benefit.status
    );
    Some(member_validation_exception(
        member,
        &requirement,
        VALIDATION_REASON_BUSINESS,
    ))
}

/// The input member a benefit application's partner contacts arrive on, as the wire
/// spells it (`smithy_query(query="operation",operation="CreateBenefitApplication")`).
pub const PARTNER_CONTACTS_MEMBER: &str = "PartnerContacts";

/// The input member a benefit application's attachments arrive on, likewise.
pub const FILE_DETAILS_MEMBER: &str = "FileDetails";

/// The member of a `Contact` the model constrains not at all: `Phone` targets
/// `ContactPhone`, a bare string with no `@length`, while every other member of the shape
/// carries one (`smithy_query(query="shape",shape="Contact")`).
pub const CONTACT_PHONE_MEMBER: &str = "Phone";

/// The member of a supplied `FileInput` — and of the `FileDetail` it is stored in — the
/// model constrains not at all, for the same reason: `BusinessUseCase` targets a bare
/// `String` (`smithy_query(query="shape",shape="FileInput")`).
pub const FILE_BUSINESS_USE_CASE_MEMBER: &str = "BusinessUseCase";

/// The requirement a caller string this service PERSISTS fails when the model puts no
/// bound on it at all, or `None` when the value is one this service stores.
///
/// Such a value carries this service's own bound on a persisted caller string
/// ([`MAX_DETAILS_VALUE_LEN`], the longest string member the model lets a caller store on
/// a benefit application), applied through [`length_clause`] so it is counted and worded
/// exactly like a modelled length. The one function every path that writes such a member
/// goes through, so a value one path stores cannot be one another path refuses.
pub fn unconstrained_member_clause(value: &str) -> Option<String> {
    length_clause(value, 0, MAX_DETAILS_VALUE_LEN)
}

/// The path a refusal names for `member` of the element at `index` of the list member
/// `list`, spelled the way the centralized walk spells one: the list member, the element's
/// 1-based position and the collection step, then the member — through the framework's own
/// path helpers, so a handler's rule names a nested member exactly as the walk does.
pub fn element_member_path(list: &str, index: usize, member: &str) -> String {
    let element = ::aws_smithy_schema_server::constraints::path::element(index + 1);
    let leaf = ::aws_smithy_schema_server::constraints::path::member(member);
    format!("{list}.{element}.{leaf}")
}

/// The refusal a request supplying a `PartnerContacts` or `FileDetails` list is answered
/// with when one of the caller strings it would persist is past this service's own bound,
/// or `None` when every supplied value can be stored.
///
/// The members checked are the ones the model constrains not at all
/// ([`CONTACT_PHONE_MEMBER`], [`FILE_BUSINESS_USE_CASE_MEMBER`]); every other string of
/// either shape carries a modelled `@length` or `@pattern` the centralized walk has
/// already enforced, as do the two lists themselves (`PartnerContacts` `@length(max: 1)`,
/// `FileDetails` `@length(max: 30)` —
/// `smithy_query(query="operation",operation="UpdateBenefitApplication")`), so how many
/// elements one request may name is refused before this runs. The refusal is built by the
/// one construction every member refusal of this crate goes through
/// ([`member_validation_exception`]), naming the element and the member a caller must fix
/// ([`element_member_path`]) and echoing nothing of the value.
///
/// Applied by every operation that supplies these lists — the create and the update to the
/// lists they were handed, `AmendBenefitApplication` to the single value an amendment
/// writes — so one rule refuses one value in one form wherever it holds, and no read can
/// report a value no other write path would accept.
pub fn unconstrained_members_refusal(
    contacts: Option<&[Contact]>,
    files: Option<&[FileInput]>,
) -> Option<crate::ops::types::ValidationException> {
    let refused_contact = contacts
        .unwrap_or_default()
        .iter()
        .enumerate()
        .find_map(|(index, contact)| {
            let clause = unconstrained_member_clause(contact.phone.as_deref()?)?;
            Some((
                element_member_path(PARTNER_CONTACTS_MEMBER, index, CONTACT_PHONE_MEMBER),
                clause,
            ))
        });
    let refused_file = || {
        files
            .unwrap_or_default()
            .iter()
            .enumerate()
            .find_map(|(index, file)| {
                let clause = unconstrained_member_clause(file.business_use_case.as_deref()?)?;
                Some((
                    element_member_path(FILE_DETAILS_MEMBER, index, FILE_BUSINESS_USE_CASE_MEMBER),
                    clause,
                ))
            })
    };
    let (member, requirement) = refused_contact.or_else(refused_file)?;
    Some(member_validation_exception(
        &member,
        &requirement,
        VALIDATION_REASON_FIELD,
    ))
}

/// The input member a benefit application's resource links arrive on when one request
/// supplies the whole list, as the wire spells it
/// (`smithy_query(query="operation",operation="CreateBenefitApplication")`).
pub const ASSOCIATED_RESOURCES_MEMBER: &str = "AssociatedResources";

/// The refusal a request supplying a list of resource ARNs to link is answered with when
/// one of the ARNs it would persist is longer than this value's published bound
/// ([`MAX_ASSOCIATED_RESOURCE_ARN_LEN`]), or `None` when every supplied ARN can be stored.
///
/// The ELEMENTS of the modelled `Arns` list carry no length of their own, while the member
/// that names one such resource on the association operations carries the model's own
/// bound on the value, so this is where that bound is applied to the list form of the same
/// write — the same reason [`length_clause`] applies a `StatusReason`'s published bound on
/// the operation whose own input the model leaves unconstrained.
///
/// The refusal is stated at the POSITION the offending ARN sits at, the way the centralized
/// walk states a violation of an element that IS the collection's own element: collapsed
/// onto the collection member, carrying the element's requirement set
/// ([`crate::service::element_requirement`]) — and built by the one construction every
/// member refusal of this crate goes through ([`member_validation_exception`]), so nothing
/// of the ARN the caller sent is echoed.
pub fn associated_resources_refusal(
    resources: Option<&[String]>,
) -> Option<crate::ops::types::ValidationException> {
    let requirement = resources
        .unwrap_or_default()
        .iter()
        .find_map(|arn| length_clause(arn, 0, MAX_ASSOCIATED_RESOURCE_ARN_LEN))?;
    Some(member_validation_exception(
        ASSOCIATED_RESOURCES_MEMBER,
        &crate::service::element_requirement(&requirement),
        VALIDATION_REASON_FIELD,
    ))
}

/// The stored `FileDetail` for one supplied `FileInput`.
///
/// `FileURI` and `BusinessUseCase` are the caller's own values; `FileName` and `FileType`
/// are derived from the URI (see [`file_name_of`] and [`file_type_of`]); `CreatedBy` and
/// `CreatedAt` record who attached the file and when. `FileStatus`/`FileStatusReason`
/// stay absent: nothing in this emulator processes an attachment, so there is no status
/// to report.
pub fn file_detail(input: &FileInput, created_by: &str, now_millis: i64) -> FileDetail {
    FileDetail {
        file_uri: input.file_uri.clone(),
        business_use_case: input.business_use_case.clone(),
        file_name: file_name_of(&input.file_uri),
        file_status: None,
        file_status_reason: None,
        file_type: file_type_of(&input.file_uri),
        created_by: Some(created_by.to_string()),
        created_at: Some(timestamp(now_millis)),
    }
}

/// Whether `stored` already holds exactly the files `supplied` names, in the same order.
///
/// A file's caller-supplied content is its `FileURI` and `BusinessUseCase`, and the
/// `FileName`/`FileType` beside them follow the URI (see [`file_detail`]), so two
/// attachments naming the same URI and use case are the same attachment; `CreatedBy` and
/// `CreatedAt` record WHO attached that file and WHEN rather than what was attached, so
/// they are not part of what the caller can change. A request re-sending the files an
/// application already carries therefore changes nothing.
pub fn same_file_details(stored: &[FileDetail], supplied: &[FileInput]) -> bool {
    stored.len() == supplied.len()
        && stored
            .iter()
            .zip(supplied)
            .all(|(stored, supplied)| {
                stored.file_uri == supplied.file_uri
                    && stored.business_use_case == supplied.business_use_case
            })
}

/// The `FileDetail` list an application holds once a write supplies `supplied`.
///
/// `CreatedBy` and `CreatedAt` record WHO uploaded a file and WHEN ("The identifier of the
/// user who uploaded the file", "The timestamp when the file was uploaded" —
/// `smithy_query(query="shape",shape="FileDetail")`), so they belong to the FILE rather
/// than to the request that last wrote the list or to the position the file holds in it: a
/// supplied attachment naming a `FileURI` the application already carries keeps the
/// principal and instant the call that attached it recorded, and only a file the
/// application does not already carry records `created_by` and `now_millis`. A write that
/// adds, removes or reorders attachments therefore leaves the upload facts of every file
/// already attached exactly as they were, while the members the caller supplies — and the
/// `FileName`/`FileType` the URI governs — come from `supplied` through the one derivation
/// every stored attachment goes through ([`file_detail`]).
///
/// Each stored file is carried forward at most once, so two supplied entries naming one
/// stored file take that file's record for the first of them and record this call for the
/// second. The stored files are indexed by `FileURI` ONCE, so the merge costs one pass
/// over each list rather than a scan of the stored list per supplied entry — and both
/// lists are at most the modelled `FileDetails` `@length(max: 30)` the centralized walk
/// enforces before any write path reaches here
/// (`smithy_query(query="operation",operation="UpdateBenefitApplication")`).
pub fn merged_file_details(
    stored: &[FileDetail],
    supplied: &[FileInput],
    created_by: &str,
    now_millis: i64,
) -> Vec<FileDetail> {
    let mut already_attached: HashMap<&str, VecDeque<&FileDetail>> = HashMap::new();
    for file in stored {
        already_attached
            .entry(file.file_uri.as_str())
            .or_default()
            .push_back(file);
    }
    supplied
        .iter()
        .map(|input| {
            let attached = already_attached
                .get_mut(input.file_uri.as_str())
                .and_then(VecDeque::pop_front);
            let detail = file_detail(input, created_by, now_millis);
            match attached {
                Some(attached) => FileDetail {
                    created_by: attached.created_by.clone(),
                    created_at: attached.created_at,
                    ..detail
                },
                None => detail,
            }
        })
        .collect()
}

/// The last path segment of `uri`, which is the file's name, with any query or fragment
/// removed. `None` when the URI names no segment, or one outside the modelled
/// `FileDetail$FileName` length ([`MIN_FILE_NAME_LEN`]..[`MAX_FILE_NAME_LEN`]).
///
/// The bound goes through [`length_clause`], the one length check this service makes for
/// itself, so a name this crate accepts in a value it refuses is not possible and the
/// length means here what it means everywhere else in the crate.
pub fn file_name_of(uri: &str) -> Option<String> {
    let path = uri
        .split(['?', '#'])
        .next()
        .unwrap_or(uri)
        .trim_end_matches('/');
    let name = path.rsplit('/').next()?;
    length_clause(name, MIN_FILE_NAME_LEN, MAX_FILE_NAME_LEN)
        .is_none()
        .then(|| name.to_string())
}

/// The `FileType` the extension of `uri` names, or `None` for an extension
/// [`FILE_TYPES_BY_EXTENSION`] does not list.
pub fn file_type_of(uri: &str) -> Option<String> {
    let name = file_name_of(uri)?;
    let extension = name.rsplit_once('.')?.1.to_ascii_lowercase();
    FILE_TYPES_BY_EXTENSION
        .iter()
        .find(|(candidate, _)| *candidate == extension)
        .map(|(_, file_type)| (*file_type).to_string())
}

/// The id of the benefit named `name` in `catalog`: `ben-` + 14 `[0-9a-z]` characters
/// derived from both, so the id is stable across runs and distinct per catalog.
fn benefit_id(catalog: &str, name: &str) -> String {
    let seed = fnv1a64(format!("{catalog}/{name}").as_bytes());
    format!("{BENEFIT_ID_PREFIX}{}", id_body(seed))
}

/// The id of the seeded allocation named `name` in `catalog`, derived from both exactly as
/// a seeded benefit's is (see [`benefit_id`]), so it too is stable across runs and distinct
/// per catalog. A GRANTED allocation's id is minted instead, by
/// [`ComAmazonawsPartnercentralbenefitsPartnercentralbenefitsserviceState::mint_allocation_id`].
fn allocation_id(catalog: &str, name: &str) -> String {
    let seed = fnv1a64(format!("{catalog}/{name}").as_bytes());
    format!("{ALLOCATION_ID_PREFIX}{}", id_body(seed))
}

/// The `IssuanceDetail$IssuanceId` of the issuance made against the allocation `id`.
///
/// The member is an unconstrained `String` and AWS publishes no format for it, so the id is
/// DERIVED from the allocation's own id: stable across reads, and distinct per allocation
/// because the allocation ids are.
fn issuance_id(id: &str) -> String {
    format!("issuance-{}", id_body(fnv1a64(id.as_bytes())))
}

/// The `CreditCode$AwsCreditCode` of code `index` of the allocation `id`.
///
/// The member is an unconstrained `String` and AWS publishes no format for a promotional
/// credit code, so the code is DERIVED from the allocation's id and the code's index within
/// it: [`ID_BODY_LEN`] upper-case characters, stable across reads and distinct per code.
fn credit_code_text(id: &str, index: usize) -> String {
    let seed = fnv1a64(format!("{id}/{index}").as_bytes());
    id_body(seed).to_ascii_uppercase()
}

/// [`ID_BODY_LEN`] characters drawn from [`ID_ALPHABET`] by mixing `seed`.
fn id_body(seed: u64) -> String {
    let mut state = seed;
    (0..ID_BODY_LEN)
        .map(|_| {
            state = splitmix64(state);
            ID_ALPHABET[(state % ID_ALPHABET.len() as u64) as usize] as char
        })
        .collect()
}

/// FNV-1a, a standard 64-bit non-cryptographic hash. Used only to derive a seeded
/// benefit's id from its name, so it must be STABLE across processes — which rules out
/// `std`'s `DefaultHasher`, whose output is not specified.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// SplitMix64, a standard 64-bit mixing function: a bijection on `u64`, so distinct
/// inputs always give distinct outputs. Used only to scatter minted identifiers.
fn splitmix64(input: u64) -> u64 {
    let mut z = input.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The current wall-clock instant as epoch milliseconds.
pub fn now_millis() -> i64 {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(since_epoch.as_millis()).unwrap_or(i64::MAX)
}

/// The new `UpdatedAt` for a record last written at `previous` by a change that happened
/// at `applied_at`.
///
/// `applied_at` is WHEN the change happened: the request's clock for a write a caller
/// made, and the instant the change fell due for one the clock owed (see
/// [`BenefitAllocation::expire_due`]). That instant unless it is not past `previous`, in
/// which case one millisecond past `previous` — so an update is always observably later
/// than the write before it, which is what a caller comparing two reads relies on.
pub fn bump_millis(previous: i64, applied_at: i64) -> i64 {
    applied_at.max(previous.saturating_add(1))
}

/// Whether a term that ends at `expires_at_millis` has closed at `now_millis`.
///
/// The ONE place the expiry boundary is decided: `ExpiresAt` is the instant the model
/// documents a term "expires and is no longer usable" at, so the term is closed from that
/// instant on. Both terms this service holds are judged here — the allocation's
/// ([`BenefitAllocation::has_expired`]) and an issued credit code's
/// ([`AllocatedCreditCode::has_expired`]) — so the boundary cannot be stated one way for
/// one of them and another way for the other.
pub fn term_closed(expires_at_millis: i64, now_millis: i64) -> bool {
    now_millis >= expires_at_millis
}

/// Render a stored epoch-millisecond instant as the modelled `date-time` timestamp.
pub fn timestamp(millis: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// The region this request was signed for, for the ARNs the response carries.
///
/// A signed AWS SDK request names it in the SigV4 credential scope; an unsigned one
/// names none, so [`DEFAULT_REGION`] stands in. The scope is caller-controlled text and
/// the value lands inside an ARN whose modelled pattern allows only
/// `[a-z]{2}(-gov)?-[a-z]+-\d` there, so anything else is refused here and the default
/// used instead: a generated ARN can never carry text the model does not admit.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| is_modelled_region(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `region` is a value the modelled `Arn` pattern admits in its region field:
/// `([a-z]{2}(-gov)?-[a-z]+-\d{1})?`.
fn is_modelled_region(region: &str) -> bool {
    let mut fields: Vec<&str> = region.split('-').collect();
    let Some(digit) = fields.pop() else {
        return false;
    };
    if digit.len() != 1 || !digit.as_bytes()[0].is_ascii_digit() {
        return false;
    }
    let lowercase_alpha =
        |field: &str| !field.is_empty() && field.bytes().all(|b| b.is_ascii_lowercase());
    match fields.as_slice() {
        [prefix, area] => prefix.len() == 2 && lowercase_alpha(prefix) && lowercase_alpha(area),
        [prefix, gov, area] => {
            prefix.len() == 2 && lowercase_alpha(prefix) && *gov == "gov" && lowercase_alpha(area)
        }
        _ => false,
    }
}

/// The principal that attached a file, for `FileDetail$CreatedBy`.
///
/// Derived from the request's own SigV4 access key id, so the member reports who called
/// rather than a fabricated identity. That id is caller-controlled text, so it is first
/// reduced to the characters an access key id can hold; the rendering then satisfies the
/// member's modelled 1..255 length by construction, whatever the header said.
pub fn caller_principal(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    let raw = ::engine::extract_akid(&ctx.parts().headers);
    let filtered: String = raw
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(64)
        .collect();
    let akid = if filtered.is_empty() {
        "default"
    } else {
        filtered.as_str()
    };
    ::engine::emulator_arn("iam", "", &format!("user/{akid}"))
}

/// The ARN of the benefit application `id` of `catalog`, in `region`.
pub fn application_arn(region: &str, catalog: &str, id: &str) -> String {
    resource_arn(region, catalog, APPLICATION_RESOURCE, id)
}

/// The ARN of the benefit `id` of `catalog`, in `region` — account-less, because that is
/// the ARN form the `Benefit` resource itself declares ([`arn_omits_account`]).
pub fn benefit_arn(region: &str, catalog: &str, id: &str) -> String {
    resource_arn(region, catalog, BENEFIT_RESOURCE, id)
}

/// The ARN of the benefit allocation `id` of `catalog`, in `region`.
pub fn allocation_arn(region: &str, catalog: &str, id: &str) -> String {
    resource_arn(region, catalog, ALLOCATION_RESOURCE, id)
}

/// `arn:aws:partnercentral:{region}:{account}:catalog/{catalog}/{resource}/{id}`, where
/// `{account}` is the account field a resource of `resource`'s type declares: the emulator
/// account id, or EMPTY for a resource type that declares none ([`arn_omits_account`]).
///
/// The `catalog/...` resource shape is the one the modelled
/// `ListTagsForResource$resourceArn` pattern declares for this service
/// (`^arn:[\w+=/,.@-]+:partnercentral:[\w+=/,.@-]*:[0-9]{12}:catalog/([a-zA-Z]+)/[\w+=,.@-]+(/[\w+=,.@-]+)*$`),
/// so one resource-path form serves both the resource members and the tagging operations.
/// That pattern's `[0-9]{12}` account field binds the ARN of a TAGGABLE resource, which a
/// resource type declaring no account field never is — see [`arn_omits_account`].
fn resource_arn(region: &str, catalog: &str, resource: &str, id: &str) -> String {
    let path = format!("catalog/{catalog}/{resource}/{id}");
    if arn_omits_account(resource) {
        // `engine::emulator_arn` always fills the account field, so an ARN that declares
        // none is built from the same fields with that one left empty.
        return format!("arn:aws:{ARN_SERVICE}:{region}::{path}");
    }
    ::engine::emulator_arn(ARN_SERVICE, region, &path)
}

/// Whether the ARN of a resource of `resource`'s type carries an EMPTY account field.
///
/// Every resource of this service declares the ARN form it is named by, and a resource
/// whose ARN trait carries `noAccount` has no account field in it: `Benefit`
/// (`catalog/{CatalogName}/benefit/ben-{BenefitId}`, `noAccount: true`) is the one resource
/// of this service that declares it, while `BenefitApplication` and `BenefitAllocation`
/// declare no such thing and so carry the account the emulator presents — see
/// `smithy_query(query="shape",shape="Benefit,BenefitApplication,BenefitAllocation")`.
/// The modelled `Arn` pattern admits both forms, its account field being optional
/// (`^arn:aws:([a-zA-Z0-9\-])+:([a-z]{2}(-gov)?-[a-z]+-\d{1})?:(\d{12})?:(.+)$`), and a
/// resource type that declares no account field is never a taggable one — the benefit
/// application is the only resource of this service that holds a tag set, see
/// [`ComAmazonawsPartnercentralbenefitsPartnercentralbenefitsserviceState::taggable_resource`]
/// — so no ARN of such a type has to satisfy the tagging pattern's account field.
fn arn_omits_account(resource: &str) -> bool {
    resource == BENEFIT_RESOURCE
}

/// The `(catalog, id)` the ARN this service issues for a `resource` of `region` names, or
/// `None` when `arn` is not one.
///
/// An ARN names a resource only when it is the ARN this service GIVES that resource
/// ([`resource_arn`]) — compared whole, so every field is a field this service minted: an
/// ARN naming another partition, another region or another account names no resource here,
/// just as one naming another service, another resource type or another catalog does not.
/// For a resource type whose ARN declares no account field ([`arn_omits_account`]), an ARN
/// that carries one is just such a foreign ARN, and names nothing.
pub fn parse_resource_arn<'a>(
    arn: &'a str,
    region: &str,
    resource: &str,
) -> Option<(&'a str, &'a str)> {
    // The `catalog/<catalog>/<resource>/<id>` tail an ARN of this service carries, read
    // off the sixth colon-separated field; the equality below is what checks the five
    // fields ahead of it.
    let tail = arn.splitn(6, ':').nth(5)?;
    let mut segments = tail.split('/');
    if segments.next()? != "catalog" {
        return None;
    }
    let catalog = segments.next()?;
    if segments.next()? != resource {
        return None;
    }
    let id = segments.next()?;
    (arn == resource_arn(region, catalog, resource, id)).then_some((catalog, id))
}

/// AWS's "not found" message: the resource that was looked up, then the identifier the
/// request named it with, backtick-quoted — e.g. ``Catalog `Sandbox` not found``.
///
/// One construction for every lookup this service does, so a client reading the message
/// learns WHICH resource was missing from the one place the sentence is written.
/// `resource` is [`CATALOG_KIND`], [`BENEFIT_KIND`], [`APPLICATION_KIND`] or
/// [`ALLOCATION_KIND`].
pub fn not_found_message(resource: &str, identifier: &str) -> String {
    format!("{resource} `{identifier}` not found")
}

/// The refusal an application lookup fails with: [`APPLICATION_KIND`] paired with the
/// identifier the REQUEST named it with, through [`not_found_message`].
///
/// One construction for every operation that resolves an application, reached through
/// [`ComAmazonawsPartnercentralbenefitsPartnercentralbenefitsserviceState::require_application`]
/// and its writing counterpart, so the kind the sentence names and the identifier it
/// quotes are the same in all of them — the identifier as written by the request, not the
/// stored id it would have resolved to.
fn application_not_found(identifier: &str) -> crate::ops::types::ResourceNotFoundException {
    crate::ops::types::ResourceNotFoundException {
        message: not_found_message(APPLICATION_KIND, identifier),
    }
}

/// The refusal a tagging operation's resource lookup fails with.
///
/// One construction for every tagging operation (`TagResource`, `UntagResource`,
/// `ListTagsForResource`), reached through
/// [`ComAmazonawsPartnercentralbenefitsPartnercentralbenefitsserviceState::require_taggable_resource`]
/// and its writing counterpart, so a `resourceArn` resolving to no resource is answered in
/// the same words whichever of them was called. These operations bind no `Catalog` member,
/// so the lookup reports the catalog the request named: none — see
/// [`catalog_not_found_message`] for the recorded tagging answers that wording is built
/// from.
fn taggable_resource_not_found() -> crate::ops::types::ResourceNotFoundException {
    crate::ops::types::ResourceNotFoundException {
        message: catalog_not_found_message(None),
    }
}

/// The message a request refused because of the application's STATUS carries: the
/// application, the status it is in, and the thing that status does not allow — e.g.
/// ``benefit application benappl-… is IN_REVIEW, so it cannot be updated``.
///
/// One construction for every status refusal this service makes (`submitted`, `recalled`,
/// `updated`, `amended`, `canceled`), so a client reading a `ConflictException` learns
/// the current status from any of them in the same words. `action` is the past participle
/// of the refused change.
pub fn status_conflict_message(id: &str, status: &str, action: &str) -> String {
    format!("benefit application {id} is {status}, so it cannot be {action}")
}

/// The message an optimistic-concurrency refusal carries: the application, the revision
/// it is actually at, and the revision the request named — e.g. ``benefit application
/// benappl-… is at revision 3, not 1``.
///
/// One construction for every revision refusal this service makes
/// (`UpdateBenefitApplication`, `AmendBenefitApplication`), for the same reason
/// [`status_conflict_message`] is written once: a client reading a `ConflictException`
/// learns the current revision from any of them in the same words, and in the same
/// order — the stored revision first, the requested one second.
pub fn revision_conflict_message(id: &str, current: &str, requested: &str) -> String {
    format!("benefit application {id} is at revision {current}, not {requested}")
}

/// The entitlement gate: `Some(..)` when the calling account holds no active AWS partner
/// benefit, and the request must be refused with the modelled `AccessDeniedException`
/// (served with [`ACCESS_DENIED_STATUS`]) instead of being served.
///
/// A REPORTING operation applies this BEFORE it looks any resource up, so an un-entitled
/// caller reading the service is told it is not entitled rather than whether a resource
/// exists; the modelled constraints are refused earlier still, by the centralized request
/// walk, so a malformed request is a `ValidationException` whether the caller is entitled
/// or not.
///
/// An operation that WRITES answers its resource lookup first and applies the gate to a
/// request whose target resolved — see [`write_request_refusal`], which is what those
/// handlers call. The two tag-MUTATING operations (`TagResource`, `UntagResource`) go
/// further and do not apply the gate at all, answering only their own lookup — see
/// `crate::ops::tag_resource`.
///
/// The emulator presents exactly one account (`engine::EMULATOR_ACCOUNT_ID`), so that is
/// the account every request is made from and the only one the gate can be asked about.
/// A store that cannot be reached answers `None` — unknown is not "denied", and the
/// handler's own store check turns it into the modelled server error.
pub fn entitlement_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<crate::ops::types::AccessDeniedException> {
    service_state(engine)?.lock().entitlement_refusal()
}

/// What a WRITING operation is refused with before it does any work of its own, in the
/// order the service answers those two refusals in.
#[derive(Debug)]
pub enum WriteRequestRefusal {
    /// The catalog the request named is not one this service holds, so the write has no
    /// place to land and the missing catalog is the whole answer.
    MissingCatalog(crate::ops::types::ResourceNotFoundException),
    /// The catalog resolved, and the calling account holds no active AWS partner benefit.
    NotEntitled(crate::ops::types::AccessDeniedException),
}

/// The refusal an operation that WRITES answers a request with ahead of its own work: the
/// `Catalog` the request names is resolved FIRST ([`ComAmazonawsPartnercentralbenefitsPartnercentralbenefitsserviceState::catalog_refusal`]),
/// and the partner-entitlement gate applies only to a request whose catalog resolved.
///
/// ONE construction of that order for every writing operation, so a catalog this service
/// does not hold is reported as the missing resource it is whichever write named it, and
/// an un-entitled caller naming a real catalog is still denied. A store that cannot be
/// reached answers `None`, exactly as [`entitlement_refusal`] does, and the handler's own
/// store check turns that into the modelled server error.
pub fn write_request_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    catalog: &str,
) -> Option<WriteRequestRefusal> {
    let state = service_state(engine)?;
    let store = state.lock();
    if let Some(missing) = store.catalog_refusal(catalog) {
        return Some(WriteRequestRefusal::MissingCatalog(missing));
    }
    store.entitlement_refusal().map(WriteRequestRefusal::NotEntitled)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsPartnercentralbenefitsPartnercentralbenefitsserviceState>>
{
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsPartnercentralbenefitsPartnercentralbenefitsserviceState>)
}

/// A caller-supplied list of IDENTITIES, as the store keeps it: each distinct value once,
/// in the order the request first named it.
///
/// An application's `AssociatedResources` (resource ARNs) and `FulfillmentTypes`
/// (`FulfillmentType` enum values) are lists whose elements are identities, so membership
/// is the whole of what one carries: naming the same resource or the same fulfillment type
/// twice says nothing a single entry does not. The service itself maintains them that way —
/// `AssociateBenefitApplicationResource` adds an ARN the record already carries exactly
/// zero times, `DisassociateBenefitApplicationResource` removes the resource the request
/// names, and [`MAX_ASSOCIATED_RESOURCES_PER_APPLICATION`] bounds how many resources one
/// application is LINKED to — so the create stores the set its list denotes rather than the
/// repetitions it was written with, and every read, filter and bound then counts distinct
/// values whichever path stored them.
///
/// Order is first appearance, not sorted: the stored list keeps the order the caller gave,
/// exactly as [`BenefitApplication::associated_resources`] keeps the order associations
/// were added in.
pub fn distinct_values(values: Option<Vec<String>>) -> Vec<String> {
    let mut distinct: Vec<String> = Vec::new();
    for value in values.unwrap_or_default() {
        if !distinct.contains(&value) {
            distinct.push(value);
        }
    }
    distinct
}

/// The tags a create supplied, as the store keeps them.
pub fn tag_map(tags: Option<&Vec<Tag>>) -> BTreeMap<String, String> {
    tags.map(|tags| {
        tags.iter()
            .map(|tag| (tag.key.clone(), tag.value.clone()))
            .collect()
    })
    .unwrap_or_default()
}

/// The tag set `additions` leaves behind when merged onto `current`: a key `additions`
/// carries takes its value there (`TagResource` "adds or updates tags"), and every other
/// key keeps the value it had.
pub fn merged_tags(
    current: &BTreeMap<String, String>,
    additions: &[Tag],
) -> BTreeMap<String, String> {
    let mut merged = current.clone();
    for tag in additions {
        merged.insert(tag.key.clone(), tag.value.clone());
    }
    merged
}

/// The tag-key prefix AWS reserves for its own system tags.
///
/// AWS-wide tagging rule: a key in this namespace is AWS's own — a caller can neither
/// create, overwrite nor remove one through a public tagging API — so a request supplying
/// such a key is refused with the modelled `ValidationException` instead of being served.
/// The modelled `TagKey` pattern (`^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`, see
/// `smithy_query(query="shape",shape="Tag")`) admits `:`, so the rule cannot be expressed
/// as a modelled constraint and the centralized walk cannot enforce it: it lives here, and
/// every operation that takes caller-supplied tag keys applies it.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` lies in the reserved tag namespace ([`RESERVED_TAG_KEY_PREFIX`]).
///
/// The prefix is matched ASCII-case-insensitively: the namespace belongs to AWS whatever
/// case a caller spells it in, so no case variant can slip a key into it. The comparison
/// is over BYTES, so a key opening with a multi-byte character (the modelled pattern
/// admits `\p{L}`) is simply not reserved rather than a slice on a char boundary.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.as_bytes()
        .get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX.as_bytes()))
}

/// The requirement a caller-supplied tag key fails when it names the reserved namespace,
/// worded like the requirements the centralized walk states
/// ([`crate::service::constraint_requirement`]).
pub fn reserved_tag_key_requirement() -> String {
    format!("Member must not use the reserved tag key prefix: {RESERVED_TAG_KEY_PREFIX}")
}

/// The member of a supplied `Tag` that carries the tag key, as the wire spells it
/// (`Tag$Key`, `smithy_query(query="shape",shape="Tag")`).
pub const TAG_KEY_MEMBER: &str = "Key";

/// Where in a request's input member the tag keys sit — which is what decides the POSITION
/// a refusal of one is reported at.
///
/// The two forms the model gives a caller for supplying tag keys: a list whose ELEMENTS are
/// the keys (`UntagResourceInput$tagKeys`, a `TagKeyList` of `TagKey`) and a list of `Tag`
/// STRUCTURES, each carrying the key in its own member (`TagResourceInput$tags` and
/// `CreateBenefitApplicationInput$Tags`, both `Tags` lists of `Tag`); see
/// `smithy_query(query="shape",shape="TagKeyList,Tag")`.
#[derive(Debug, Clone, Copy)]
pub enum SuppliedTagKeys<'a> {
    /// The keys are the collection's own elements.
    Keys(&'a [String]),
    /// Each key is the [`TAG_KEY_MEMBER`] of one structure element of the collection.
    Tags(&'a [Tag]),
}

impl SuppliedTagKeys<'_> {
    /// The member path and the requirement a refusal states for the first reserved key
    /// among these, or `None` when every key is the caller's own.
    ///
    /// Both are decided by the POSITION the offending key sits at, stated the way the
    /// centralized walk states a violation there (see
    /// [`crate::service::constraint_clause`]): a key that IS an element of the collection
    /// collapses onto the collection member and carries the element's requirement set
    /// ([`crate::service::element_requirement`]), while a key held by a member INSIDE a
    /// structure element keeps that member's own full path
    /// ([`element_member_path`]) and states its requirement flat.
    fn reserved(&self, member: &str) -> Option<(String, String)> {
        let requirement = reserved_tag_key_requirement();
        match self {
            Self::Keys(keys) => keys.iter().any(|key| is_reserved_tag_key(key)).then(|| {
                (
                    member.to_string(),
                    crate::service::element_requirement(&requirement),
                )
            }),
            Self::Tags(tags) => tags
                .iter()
                .position(|tag| is_reserved_tag_key(&tag.key))
                .map(|index| {
                    (
                        element_member_path(member, index, TAG_KEY_MEMBER),
                        requirement,
                    )
                }),
        }
    }
}

/// The refusal a request supplying a reserved tag key is answered with, or `None` when
/// every key it carries is the caller's own.
///
/// `member` is the input member those keys came from, spelled as the wire spells it
/// (`tags` / `tagKeys` on the tagging operations, `Tags` on a create), so the clause names
/// the member the caller must fix. The exception is built by the one construction every
/// member refusal of this crate goes through ([`member_validation_exception`], which
/// renders the message with [`crate::service::member_clause`] and
/// [`crate::service::validation_envelope`]) and echoes NOTHING of the rejected value.
///
/// WHERE inside that member the key sat is what the refusal reports, and it reports it the
/// way the centralized walk reports a violation at the same position — see
/// [`SuppliedTagKeys::reserved`] — so one input member is described one way whether the
/// walk refused it on a modelled constraint or this rule refused it on the namespace.
/// Several reserved keys in one member are the one refusal the first of them states, as
/// [`unconstrained_members_refusal`] reports the first element it refuses.
///
/// A caller gets this answer whether the keys would be created, overwritten or removed:
/// one rule, applied wherever a request carries tag keys.
pub fn reserved_tag_key_refusal(
    member: &str,
    supplied: SuppliedTagKeys<'_>,
) -> Option<crate::ops::types::ValidationException> {
    let (path, requirement) = supplied.reserved(member)?;
    Some(member_validation_exception(
        &path,
        &requirement,
        VALIDATION_REASON_FIELD,
    ))
}

/// A stored tag set as the modelled `Tags` list, in tag-key order.
///
/// A resource carrying no tags reports an EMPTY list rather than omitting the member: the
/// listing reports the set it found, and an empty set is a fact about the resource.
pub fn tag_list(tags: &BTreeMap<String, String>) -> Vec<Tag> {
    tags.iter()
        .map(|(key, value)| Tag {
            key: key.clone(),
            value: value.clone(),
        })
        .collect()
}

/// The `ResourceNotFoundException` message a failed catalog lookup carries, built from
/// the `Catalog` the request named — the one construction [`not_found_message`] writes.
///
/// `None` is the tagging operations' case — they bind no `Catalog` member — and renders
/// as [`CATALOG_ABSENT`], which is what the service reports for a tagging call whose
/// `resourceArn` resolves to no resource: recorded behaviour (corpus
/// `simparity-behavioral-aws-v1`, units
/// `partnercentral-benefits.tag-resource-malformed-arn-rejected` and
/// `partnercentral-benefits.untag-resource-malformed-arn-rejected`), where a well-formed
/// tagging call naming a resource that does not exist is answered ``(
/// "ResourceNotFoundException", "Catalog `null` not found")``. The absent catalog is the
/// SERVICE's own wording for a path that carries no catalog, not an unset emulator value
/// rendered by accident.
pub fn catalog_not_found_message(catalog: Option<&str>) -> String {
    not_found_message(CATALOG_KIND, catalog.unwrap_or(CATALOG_ABSENT))
}

/// A list member, omitted from the wire when nothing was ever stored for it.
pub fn optional_list<T: Clone>(values: &[T]) -> Option<Vec<T>> {
    (!values.is_empty()).then(|| values.to_vec())
}

/// The entries a list filter the caller SUPPLIED resolves to through `resolve`, or
/// `None` when the caller supplied no filter at all.
///
/// An absent or empty list selects everything (`None` here), which is what an omitted
/// list filter means on the wire; a list the caller DID supply selects only the records
/// matching one of its resolved entries, so entries that resolve to nothing — an
/// identifier no record of this catalog carries, a member whose optional parts name no
/// value this listing can compare — narrow the listing to nothing rather than widening
/// it to everything.
pub fn supplied_filter<T, R>(
    wanted: Option<&Vec<T>>,
    resolve: impl FnMut(&T) -> Option<R>,
) -> Option<Vec<R>> {
    wanted.and_then(|wanted| {
        (!wanted.is_empty()).then(|| wanted.iter().filter_map(resolve).collect())
    })
}

/// One filter from two wire members that name the same thing — a member and the
/// `@deprecated` member it replaced: the union of the entries each supplied, and `None`
/// only when the caller supplied neither, so a request that supplies either member is
/// filtered by it.
pub fn union_filters<R>(left: Option<Vec<R>>, right: Option<Vec<R>>) -> Option<Vec<R>> {
    match (left, right) {
        (None, None) => None,
        (Some(entries), None) | (None, Some(entries)) => Some(entries),
        (Some(mut left), Some(right)) => {
            left.extend(right);
            Some(left)
        }
    }
}

/// Whether `values` carries at least one entry of `filter`. An empty or absent filter
/// matches everything, which is what an omitted list filter means on the wire.
pub fn matches_any(filter: Option<&Vec<String>>, values: &[String]) -> bool {
    match filter {
        None => true,
        Some(filter) if filter.is_empty() => true,
        Some(filter) => filter.iter().any(|wanted| values.contains(wanted)),
    }
}

/// The `ValidationExceptionReason` a refusal of one input member carries; a value of the
/// model's own `ValidationExceptionReason` enum (see
/// `crate::ops::types::VALIDATION_EXCEPTION_REASON_VALUES`).
pub const VALIDATION_REASON_FIELD: &str = "fieldValidationFailed";

/// The `ValidationExceptionReason` a refusal carries when the request is well formed and
/// a rule of the service's own refuses what it asks for — a benefit the catalog no
/// longer offers, say. Also a value of the model's own `ValidationExceptionReason` enum
/// (see `crate::ops::types::VALIDATION_EXCEPTION_REASON_VALUES`).
pub const VALIDATION_REASON_BUSINESS: &str = "BUSINESS_VALIDATION_FAILED";

/// The `ValidationExceptionField$Code` for a member whose value is not one this service
/// accepts; a value of the model's own `ValidationExceptionErrorCode` enum (see
/// `crate::ops::types::VALIDATION_EXCEPTION_ERROR_CODE_VALUES`).
pub const VALIDATION_CODE_INVALID_VALUE: &str = "INVALID_VALUE";

/// A one-member `FieldList` for a `ValidationException`, so a client can see WHICH
/// member it must fix rather than only the message.
pub fn validation_field(
    name: &str,
    message: &str,
    code: &str,
) -> Vec<crate::ops::types::ValidationExceptionField> {
    vec![crate::ops::types::ValidationExceptionField {
        name: name.to_string(),
        message: message.to_string(),
        code: Some(code.to_string()),
    }]
}

/// Whether `value` satisfies the `@length(min: min, max: max)` of the member it is
/// written to or stored in.
///
/// The one length check every handler of this service makes for itself goes through, so a
/// bound a handler enforces cannot be worded — or counted — two ways. The requirement is
/// worded by [`crate::service::constraint_requirement`], the same words the centralized
/// walk uses for a length, and the length is counted in CHARACTERS, as the walk counts a
/// `@length` on a string.
///
/// `None` when the value satisfies the bound.
pub fn length_clause(value: &str, min: usize, max: usize) -> Option<String> {
    count_length_clause(value.chars().count(), min, max)
}

/// Whether a COUNT satisfies the `@length(min: min, max: max)` of the collection it is the
/// length of — the elements of a list, the entries of a map.
///
/// The same wording [`length_clause`] gives a string's length, for the same reason: a
/// length bound a handler enforces for itself (one the model leaves off the collection, so
/// both of its ends live in the handler) states the requirement
/// [`crate::service::constraint_requirement`] words for that violation class, at BOTH ends,
/// rather than a sentence of its own that can drift from the one the centralized walk
/// states for the other end of the same member.
///
/// `None` when the count satisfies the bound.
pub fn count_length_clause(count: usize, min: usize, max: usize) -> Option<String> {
    let kind = if count < min {
        ::smithy_server_runtime::ViolationKind::MinLength(min as i64)
    } else if count > max {
        ::smithy_server_runtime::ViolationKind::MaxLength(max as i64)
    } else {
        return None;
    };
    crate::service::constraint_requirement(&kind)
}

/// This service's modelled `ValidationException` refusing ONE input member: the clause
/// that names the member and the requirement it failed, under the AWS validation
/// envelope, with the one-member `FieldList` that says which member a client must fix.
///
/// The single construction every in-handler refusal of a member goes through — the
/// clause ([`crate::service::member_clause`]), the envelope
/// ([`crate::service::validation_envelope`], whose one-clause form always renders, the
/// bare clause standing in only for the case it declines) and the field list
/// ([`validation_field`]) — so the message, the reason and the field list of a refusal
/// cannot drift between the operations that answer one. `reason` is the
/// `ValidationExceptionReason` the refusal class carries: [`VALIDATION_REASON_FIELD`] for
/// a member whose value this service does not accept, [`VALIDATION_REASON_BUSINESS`] for
/// a well-formed request a rule of the service's own refuses.
///
/// `member` is the input member as the wire spells it, optionally followed by the steps a
/// handler addressed below it; the PATH the clause and the `ValidationExceptionField$Name`
/// state is spelled here by [`crate::service::walk_member_path`], so one input member is
/// named one way whether it was refused by the centralized walk or by a handler's own
/// rule.
pub fn member_validation_exception(
    member: &str,
    requirement: &str,
    reason: &str,
) -> crate::ops::types::ValidationException {
    let path = crate::service::walk_member_path(member);
    let clause = crate::service::member_clause(&path, requirement, false);
    crate::ops::types::ValidationException {
        message: crate::service::validation_envelope(std::slice::from_ref(&clause))
            .unwrap_or(clause),
        reason: reason.to_string(),
        field_list: Some(validation_field(
            &path,
            requirement,
            VALIDATION_CODE_INVALID_VALUE,
        )),
    }
}

/// The page size a list operation uses, from the `MaxResults` the request asked for.
///
/// `MaxResults` is modelled `range(1..100)`, so the centralized walk is the enforcement
/// point and has already refused anything outside it; the modelled maximum stands in when
/// the request names none. The clamp is belt-and-braces for that unreachable case, so a
/// page can never be zero-sized (an empty page carrying a `NextToken` would make a paging
/// caller loop forever).
pub fn page_size(max_results: Option<i32>) -> usize {
    let requested = max_results.unwrap_or(MAX_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE);
    usize::try_from(requested).unwrap_or(1)
}

/// What separates the catalog a cursor was issued for from the record id it names.
///
/// Neither side can hold it: a catalog name is `^[A-Za-z0-9_-]+$` (the modelled
/// `CatalogName` pattern) and a record id is one of the id prefixes followed by
/// [`ID_BODY_LEN`] `[0-9a-z]` characters, so splitting on the first occurrence recovers
/// both halves unambiguously.
const PAGE_TOKEN_SEPARATOR: char = ':';

/// A pagination token naming the record the NEXT page starts at, for a listing of
/// `catalog`.
///
/// The modelled `NextToken` is opaque, so the cursor is hex-encoded rather than handed
/// back in the clear — opaque to a caller, not secret: hex is trivially reversible and the
/// cursor carries nothing beyond the catalog it was issued for and a record id. Carrying
/// the catalog is what lets [`decode_page_token`] refuse a cursor minted for another
/// catalog's listing. The characters it produces sit well inside the member's modelled
/// 1..2048 length.
pub fn encode_page_token(catalog: &str, id: &str) -> String {
    ::hex::encode(format!("{catalog}{PAGE_TOKEN_SEPARATOR}{id}"))
}

/// The record id a pagination token names, or `None` when the token is not one
/// [`encode_page_token`] minted for `catalog`'s listing of `prefix`'s record kind.
pub fn decode_page_token(token: &str, prefix: &str, catalog: &str) -> Option<String> {
    let decoded = ::hex::decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())?;
    let (issued_for, id) = decoded.split_once(PAGE_TOKEN_SEPARATOR)?;
    if issued_for != catalog {
        return None;
    }
    let body = id.strip_prefix(prefix)?;
    (body.len() == ID_BODY_LEN
        && body
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()))
    .then(|| id.to_string())
}

/// Why a list operation refuses a `NextToken`: it is not a cursor this service issued for
/// the collection being paged. Each list operation turns this into its OWN modelled
/// `ValidationException` through [`foreign_page_token_refusal`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForeignPageToken;

/// The input member a refusal of a pagination cursor names, spelled as the wire spells it
/// (`ListBenefitsInput$NextToken` and the same member on the other two listings).
pub const NEXT_TOKEN_MEMBER: &str = "NextToken";

/// The requirement a `NextToken` fails when it is not a cursor `operation` issued: a
/// pagination token is a handle the service minted, so the requirement on the member is
/// that it be one of `operation`'s own.
pub fn issued_page_token_requirement(operation: &str) -> String {
    format!("Member must be a pagination token issued by {operation}")
}

/// The refusal a listing answers a `NextToken` it never issued with ([`ForeignPageToken`]),
/// for the operation that issues the cursors of that listing.
///
/// Built by the one construction every in-handler refusal of a member goes through
/// ([`member_validation_exception`]), so it reads exactly like a refusal the centralized
/// walk produced — the envelope over the clause, naming the same member path the walk
/// reports for this member — and echoes NOTHING of the token the request carried. The one
/// construction of this refusal, so the three listings cannot word it three ways.
pub fn foreign_page_token_refusal(operation: &str) -> crate::ops::types::ValidationException {
    member_validation_exception(
        NEXT_TOKEN_MEMBER,
        &issued_page_token_requirement(operation),
        VALIDATION_REASON_FIELD,
    )
}

/// The key a list operation resumes `records` at, given the `NextToken` the request
/// carried — `None` when it carried none, so the page starts at the front.
///
/// A pagination token is a handle the service itself minted: it resumes a listing only
/// when it decodes as a cursor [`encode_page_token`] issued for this catalog and this
/// record kind AND still names a record this collection holds. Anything else — a shape
/// the encoding never produces, a cursor of another catalog or another record kind, or an
/// id no record of this collection carries — is `Err(ForeignPageToken)`, never a position
/// to resume from.
pub fn resume_from<Record>(
    token: Option<&str>,
    prefix: &str,
    catalog: &str,
    records: &BTreeMap<String, Record>,
) -> Result<Option<String>, ForeignPageToken> {
    let Some(token) = token else {
        return Ok(None);
    };
    let id = decode_page_token(token, prefix, catalog).ok_or(ForeignPageToken)?;
    if !records.contains_key(&id) {
        return Err(ForeignPageToken);
    }
    Ok(Some(id))
}

/// One record a `@paginated` listing of this service pages over.
///
/// Every collection a listing pages is keyed by its record's own id, and that id is the
/// whole of what a cursor resuming the listing names (see [`encode_page_token`]) — so this
/// is all [`page_with_cursor`] needs of a record, whichever kind it is paging.
pub trait PagedRecord {
    /// The id this record is stored under, which is the key a cursor naming it resumes at.
    fn record_id(&self) -> &str;
}

impl PagedRecord for Benefit {
    fn record_id(&self) -> &str {
        &self.id
    }
}

impl PagedRecord for BenefitApplication {
    fn record_id(&self) -> &str {
        &self.id
    }
}

impl PagedRecord for BenefitAllocation {
    fn record_id(&self) -> &str {
        &self.id
    }
}

/// The page a `@paginated` listing reports and the `NextToken` it hands back, from
/// `matching` — the records of `catalog`, in the collection's own key order, that the
/// request's filters selected.
///
/// The single construction of the lookahead every listing of this service pages with, so
/// the number of records a page reports and the record its cursor resumes at cannot drift
/// between the listings: exactly ONE record past the page is taken from `matching` to
/// decide whether a further page exists, the cursor is issued for that extra record, and
/// the extra record itself is NOT reported. `None` as the cursor is what says the page
/// carried the last matching record.
///
/// Beside the other steps of the same rule — [`page_size`] for how many records a page
/// holds, [`encode_page_token`] for the cursor's encoding and [`resume_from`] for the key
/// a cursor resumes at.
pub fn page_with_cursor<'records, Record, Matching>(
    matching: Matching,
    page_size: usize,
    catalog: &str,
) -> (Vec<&'records Record>, Option<String>)
where
    Record: PagedRecord + 'records,
    Matching: Iterator<Item = &'records Record>,
{
    let mut page: Vec<&'records Record> = matching.take(page_size.saturating_add(1)).collect();
    let next_token = page
        .get(page_size)
        .map(|record| encode_page_token(catalog, record.record_id()));
    page.truncate(page_size);
    (page, next_token)
}
