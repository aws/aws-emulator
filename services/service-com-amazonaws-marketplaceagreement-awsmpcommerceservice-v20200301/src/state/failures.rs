// SPDX-License-Identifier: CC0-1.0

//! The modelled failures of this service: the `ValidationException` envelope
//! every refused member is stated in, the not-found, conflict and quota bodies,
//! and the `requestId` each of them carries.

use super::*;

/// The source of the `requestId` every error body of this service carries.
///
/// Process-wide rather than a field of the store: an error is also answered
/// where the store is not reached at all — the boundary refusal
/// ([`validation_exception_for`], raised from `crate::service` before any
/// handler runs) and [`store_unavailable`] — and a refused READ must not write
/// to the state it refused.
pub(super) static ERROR_REQUEST_IDS: AtomicU64 = AtomicU64::new(1);

/// The `requestId` for ONE error body: "The unique identifier associated with
/// the error" (`smithy_query(query="shape", shape="ValidationException")`),
/// which every exception shape this service models carries
/// (`ValidationException`, `ResourceNotFoundException`, `ConflictException`,
/// `ThrottlingException`, `AccessDeniedException`, `InternalServerException`).
///
/// `RequestId` is `@length(min: 1, max: 128)` `^[A-Za-z0-9-]+$`
/// (`smithy_query(query="shape", shape="RequestId")`), so the value is rendered
/// as a 36-character hyphenated UUID — the form AWS request identifiers take.
/// The low bits are a monotonic counter rather than randomness, which makes the
/// value unique for the life of the process without deriving anything from the
/// request it reports on: nothing a caller sends can predict or collide with
/// another error's id.
pub fn next_request_id() -> String {
    let minted = ERROR_REQUEST_IDS.fetch_add(1, Ordering::Relaxed);
    format!("00000000-0000-4000-8000-{minted:012x}")
}

/// The `ValidationExceptionReason` for a refusal this service cannot place on a
/// named member of the request: the modelled catch-all.
pub const REASON_OTHER: &str = "OTHER";

/// `ValidationException`'s own documentation, which is what the envelope states
/// when the reason names neither a member nor the resource whose state refused
/// the call ("The input fails to satisfy the constraints specified by the
/// service.", `smithy_query(query="shape", shape="ValidationException")`).
pub const VALIDATION_MESSAGE_FALLBACK: &str =
    "The input fails to satisfy the constraints specified by the service.";

/// What a `ValidationExceptionField` states about the member it names.
///
/// Every sentence is fixed: `name` already says WHICH member failed and
/// `reason` says what about it was wrong, and a refusal must not echo a
/// request-chosen value back into the response (the boundary validator caps its
/// own echo for the same reason). So the entry states only HOW the member
/// failed — it carried a value the service does not accept, it carried an
/// identifier that resolves to nothing, or it carried no value at all, which is
/// the distinction `ValidationExceptionReason`'s own `INVALID_…`/`MISSING_…`
/// pairs draw.
///
/// One sentence per kind, and the SAME sentence whichever member and whichever
/// operation the entry is about: the machine-readable part a client branches on
/// is `reason` and `fields[].name`, which this service does state per member,
/// so there is no per-member wording to guess.
///
/// Each sentence is reproduced as AWS words it for its class, punctuation
/// included, and the class is decided by WHAT was wrong with the member and
/// never by WHERE the refusal was raised: ONE unacceptable-value sentence serves
/// a value that misses a constraint the model declares (refused by the boundary
/// walk, before any handler runs) and a well-formed value outside the set a rule
/// of the service itself admits (refused inside a handler, as an unknown
/// `catalog`, party type, filter name or sort attribute is) alike. The texts are
/// not one template with a switch: they are fixed strings, so none can be
/// derived from another.
pub const FIELD_INVALID: &str = "Member is invalid.";
/// See [`FIELD_INVALID`]. The class for a member whose value this service can
/// read but cannot RESOLVE: a well-formed identifier naming a resource the
/// account does not hold — the agreement a payment request is sent under, the
/// term it charges, the charge an association is made against. Nothing about the
/// VALUE is wrong, so the class states its own sentence, which is how AWS words
/// it for this service.
pub const FIELD_INVALID_REFERENCE: &str = "Member is invalid";
/// See [`FIELD_INVALID`]. The absent-member class is worded as the requirement
/// the member is under rather than as the value it did not carry, which is how
/// AWS states it for this service.
pub const FIELD_MISSING: &str = "Member is required.";
/// See [`FIELD_INVALID`]. The class for a member that was absent and is
/// required only because of the `intent` the request stated: the text names the
/// conditional rule, so a caller can tell the member became required through the
/// intent it sent rather than being required of every request.
pub const FIELD_MISSING_FOR_INTENT: &str = "Member must not be null for the provided intent.";

/// How a member was wrong: it carried a value the service does not accept, it
/// carried an identifier that names no resource of the account, it carried no
/// value at all, or it carried none and the request's `intent` is what made it
/// required.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalKind {
    /// The member carried a value this service does not accept — one that misses
    /// a constraint the model declares, or a well-formed one outside the set a
    /// rule of the service itself admits. ONE kind, because the two are one
    /// class of failure to the caller.
    Invalid,
    /// The member carried a well-formed identifier that resolves to no resource
    /// of the account, so the value names nothing this service can act on.
    UnresolvedReference,
    /// The member was absent from the request.
    Missing,
    /// The member was absent from a request whose `intent` required it — the
    /// requirement the model states in prose rather than with `@required`, so
    /// it holds for some values of `intent` and not for others.
    MissingForIntent,
}

impl RefusalKind {
    /// The `ValidationExceptionReason` prefix this kind reports under.
    pub(super) fn reason_prefix(self) -> &'static str {
        match self {
            Self::Invalid | Self::UnresolvedReference => "INVALID",
            Self::Missing | Self::MissingForIntent => "MISSING",
        }
    }

    /// The `ValidationExceptionField$message` this kind states.
    pub(super) fn field_message(self) -> &'static str {
        match self {
            Self::Invalid => FIELD_INVALID,
            Self::UnresolvedReference => FIELD_INVALID_REFERENCE,
            Self::Missing => FIELD_MISSING,
            Self::MissingForIntent => FIELD_MISSING_FOR_INTENT,
        }
    }

    /// The kind `reason` reports, read back off the reason a handler chose, so a
    /// refusal raised with an explicit reason states the same field message a
    /// refusal derived from a violation does.
    ///
    /// The reason does not tell the two `INVALID_…` classes apart — the same
    /// `INVALID_AGREEMENT_ID` reports a malformed id and one that resolves to no
    /// agreement — so a refusal that KNOWS it failed to RESOLVE an identifier
    /// states its kind itself ([`unresolved_reference`]), and every other
    /// refusal of a value reads back as [`RefusalKind::Invalid`].
    pub(super) fn of_reason(reason: &str) -> Self {
        if reason.starts_with("MISSING_") {
            Self::Missing
        } else {
            Self::Invalid
        }
    }
}

/// `lowerCamel` as the `SCREAMING_SNAKE` word a `ValidationExceptionReason`
/// value is spelled in.
pub(super) fn screaming_snake(member: &str) -> String {
    let mut out = String::with_capacity(member.len() + 4);
    for (position, character) in member.char_indices() {
        if character.is_uppercase() && position > 0 {
            out.push('_');
        }
        out.extend(character.to_uppercase());
    }
    out
}

/// A container member named in the singular, which is how a reason names the
/// element of a list (`filters` -> `FILTER_NAME`, not `FILTERS_NAME`).
pub(super) fn singular(member: &str) -> &str {
    member.strip_suffix('s').unwrap_or(member)
}

/// The member-name segments of a dotted request path, dropping the positions and
/// collection steps the path spells between them
/// ([`::aws_smithy_schema_server::constraints::path`]).
pub(super) fn path_members(path: &str) -> Vec<&str> {
    path.split('.')
        .filter(|segment| *segment != "member" && !segment.chars().all(|c| c.is_ascii_digit()))
        .collect()
}

/// The `ValidationExceptionReason` that names the member at `path` failing in
/// `kind`.
///
/// `ValidationExceptionReason` names its members: every value is
/// `INVALID_<MEMBER>` or `MISSING_<MEMBER>` for a member of some operation's
/// input (`INVALID_AGREEMENT_ID`, `MISSING_FILTER_VALUES`,
/// `INVALID_SORT_BY`, …, `smithy_query(query="shape",
/// shape="ValidationExceptionReason")`). So the reason for a refused member is
/// resolved FROM the member: the longest tail of the path's member names that
/// spells a modelled value wins — which is what distinguishes the `name` of a
/// `filters` element (`INVALID_FILTER_NAME`) from a top-level `name`
/// (`INVALID_NAME`) — and a member no value names reports the modelled
/// catch-all [`REASON_OTHER`].
pub fn member_reason(path: &str, kind: RefusalKind) -> String {
    let members = path_members(path);
    let prefix = kind.reason_prefix();
    for take in (1..=members.len()).rev() {
        let tail = &members[members.len() - take..];
        // A container is named in the singular by a reason, so both spellings
        // are tried, the singular first: `filters.1.member.name` is
        // `INVALID_FILTER_NAME`, while `filters` alone is `INVALID_FILTERS`.
        for shorten_containers in [true, false] {
            let stem: Vec<String> = tail
                .iter()
                .enumerate()
                .map(|(position, member)| {
                    let container = position + 1 < tail.len();
                    screaming_snake(if shorten_containers && container {
                        singular(member)
                    } else {
                        member
                    })
                })
                .collect();
            let candidate = format!("{prefix}_{}", stem.join("_"));
            if crate::ops::types::VALIDATION_EXCEPTION_REASON_VALUES
                .contains(&candidate.as_str())
            {
                return candidate;
            }
        }
    }
    REASON_OTHER.to_string()
}

/// The reasons that name the STATE a resource this service resolved is in,
/// rather than a fault in the value a member carried: the member was read, the
/// resource it named was found, and the service will not act on it in the state
/// it is in.
///
/// `ValidationExceptionReason` spells every one of them `<STATE>_<RESOURCE>`
/// (`INACTIVE_AGREEMENT`, `EXPIRED_AGREEMENT_PROPOSAL`,
/// `SUPERSEDED_AGREEMENT_PROPOSAL`, `PENDING_APPROVAL_AGREEMENT_REQUEST`,
/// `CLOSED_AGREEMENT_REQUEST`, `CLOSED_AGREEMENT_APPROVAL`,
/// `QUEUED_AGREEMENT_APPROVAL`, `smithy_query(query="shape",
/// shape="ValidationExceptionReason")`), so the state is the leading word and
/// the resource is the rest — which is what [`validation_message`] words the
/// class from.
const RESOURCE_STATES: &[&str] = &[
    "INACTIVE",
    "EXPIRED",
    "SUPERSEDED",
    "PENDING_APPROVAL",
    "CLOSED",
    "QUEUED",
];

/// The `message` a `ValidationException` carrying `reason` states.
///
/// One sentence per reason, built from the reason itself — so the member the
/// request got wrong is what the sentence names, and every refusal of the same
/// member reads alike whether the boundary validator or a handler found it. A
/// reason that names neither a member nor a resource states
/// `ValidationException`'s own documentation.
pub fn validation_message(reason: &str) -> String {
    /// `INVALID_AGREEMENT_ID` → `agreement id`.
    fn words(stem: &str) -> String {
        stem.to_lowercase().replace('_', " ")
    }

    /// `MISSING_AGREEMENT_ID` → `Agreement id`.
    fn sentence_case(stem: &str) -> String {
        let mut words = words(stem);
        if let Some(first) = words.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        words
    }

    /// `INACTIVE_AGREEMENT` → (`INACTIVE`, `AGREEMENT`): the state a
    /// [`RESOURCE_STATES`] reason names, and the resource it is the state of.
    fn resource_state(reason: &str) -> Option<(&'static str, &str)> {
        RESOURCE_STATES.iter().find_map(|state| {
            reason
                .strip_prefix(state)
                .and_then(|rest| rest.strip_prefix('_'))
                .map(|resource| (*state, resource))
        })
    }

    if let Some(stem) = reason.strip_prefix("INVALID_") {
        format!("Provided {} is invalid.", words(stem))
    } else if let Some(stem) = reason.strip_prefix("MISSING_") {
        // The absent-member class states the requirement the member is under,
        // naming the member the reason names: `MISSING_AGREEMENT_ID` is
        // "Agreement id is required." — the sentence AWS states for this
        // service, for whichever member of whichever operation was left out.
        format!("{} is required.", sentence_case(stem))
    } else if let Some((state, resource)) = resource_state(reason) {
        // The resource-state class states the RESOURCE the request named and
        // the state it is in, which is how AWS words it for this service: a
        // proposal that is not active is refused "Provided agreement proposal
        // is inactive.", so the resource this service does resolve and will not
        // act on — `INACTIVE_AGREEMENT` — reads "Provided agreement is
        // inactive." The sentence is about the state, so a caller is told WHY
        // the resource it named was refused rather than being handed
        // `ValidationException`'s own documentation, which says nothing about
        // it.
        format!("Provided {} is {}.", words(resource), words(state))
    } else {
        VALIDATION_MESSAGE_FALLBACK.to_string()
    }
}

/// A `ValidationException` naming the member that was wrong and why: the
/// modelled `reason` a client branches on, and one `fields` entry naming the
/// offending member's dotted request path.
///
/// `reason` is one of `ValidationExceptionReason`'s values
/// (`VALIDATION_EXCEPTION_REASON_VALUES` in `types`); `path` is the member's
/// path, which for a member of a list element carries the element's position
/// (`filters.1.member.name`).
pub fn validation_exception(reason: &str, path: impl Into<String>) -> ValidationException {
    let path = path.into();
    validation_exception_naming(reason, &[path.as_str()])
}

/// A `ValidationException` for a member whose well-formed value names no
/// resource this account holds: the agreement a child was sent under, the term a
/// charge is requested on, the charge an association is made against.
///
/// The envelope is the one [`validation_exception`] builds — the same
/// `INVALID_<MEMBER>` reason, the same `message`, the same single `fields` entry
/// naming the member's own path — and only the field text differs, because the
/// member failed in the unresolved-reference class rather than in the
/// unacceptable-value one ([`FIELD_INVALID_REFERENCE`]). One writer, so every
/// operation of this service that RESOLVES an identifier refuses it alike,
/// whichever resource the identifier names.
pub fn unresolved_reference(reason: &str, path: impl Into<String>) -> ValidationException {
    let path = path.into();
    validation_exception_in_kind(reason, &[path.as_str()], RefusalKind::UnresolvedReference)
}

/// A `ValidationException` naming SEVERAL members, for a rule that is about the
/// members together rather than about one of them being wrong on its own.
///
/// The envelope carries one `reason` — the rule's own — and one `fields` entry
/// per member named, which is the same shape [`validation_exception_for`] gives
/// a request that broke constraints on several members: the entries say which
/// members to look at, and nothing about the request is echoed back.
pub fn validation_exception_naming(reason: &str, paths: &[&str]) -> ValidationException {
    validation_exception_in_kind(reason, paths, RefusalKind::of_reason(reason))
}

/// A `ValidationException` whose `fields` entries state a kind the `reason`
/// alone does not tell apart.
///
/// The absent-member classes report the same `MISSING_<MEMBER>` reason whether
/// the member is required of every request or became required through another
/// member's value, and they are worded differently
/// ([`FIELD_MISSING`]/[`FIELD_MISSING_FOR_INTENT`]) — so the caller that KNOWS
/// which rule it enforced says so here, and every other refusal keeps reading
/// the kind back off the reason ([`validation_exception_naming`]).
pub fn validation_exception_in_kind(
    reason: &str,
    paths: &[&str],
    kind: RefusalKind,
) -> ValidationException {
    ValidationException {
        request_id: Some(next_request_id()),
        message: Some(validation_message(reason)),
        reason: Some(reason.to_string()),
        fields: Some(
            paths
                .iter()
                .map(|path| ValidationExceptionField {
                    name: (*path).to_string(),
                    message: kind.field_message().to_string(),
                })
                .collect(),
        ),
    }
}

/// The refusal of a member a request left out that the `intent` it stated is
/// what made required — the conditional requirement the model states in prose
/// (`CreateAgreementRequest$agreementProposalIdentifier`:
/// "required if the intent is not AMEND";
/// `CreateAgreementRequest$sourceAgreementIdentifier`: "required for all non-NEW
/// intents") rather than with `@required`, so no boundary check can raise it.
///
/// The envelope is the absent-member one — the `MISSING_<MEMBER>` reason the
/// member resolves to and one `fields` entry naming it — and only the field text
/// differs from an unconditional absence: it states the intent-conditional rule
/// ([`FIELD_MISSING_FOR_INTENT`]), so one writer serves every member of this
/// service whose requirement comes from the request's `intent`.
pub fn intent_conditional_member_missing(reason: &str, path: &str) -> ValidationException {
    validation_exception_in_kind(reason, &[path], RefusalKind::MissingForIntent)
}

/// The `ValidationExceptionReason` for a request whose `agreementId` names an
/// agreement that is no longer active — the one reason the model publishes for
/// that condition.
pub const REASON_INACTIVE_AGREEMENT: &str = "INACTIVE_AGREEMENT";

/// The refusal EVERY operation of this service that acts only on an active
/// agreement answers for one that has already reached a terminal status
/// ([`STATUS_CANCELLED`], [`STATUS_REPLACED`], or any other value of
/// `AgreementStatus` outside [`STATUS_ACTIVE`]).
///
/// Such a request is an input fault on the member that named the agreement, not
/// a conflict over the resource: the status is what the caller is told to read
/// the refusal for, and `ValidationException` is the shape that carries a
/// machine-readable `reason` — [`REASON_INACTIVE_AGREEMENT`] — and a `fields`
/// entry naming `agreementId`, which `ConflictException` has no member for. One
/// writer, so `CancelAgreement`, `SendAgreementCancellationRequest` and
/// `SendAgreementPaymentRequest` cannot disagree on the class, the status or the
/// member for the same condition.
pub fn inactive_agreement() -> ValidationException {
    validation_exception(REASON_INACTIVE_AGREEMENT, "agreementId")
}

/// The `ValidationExceptionReason` a request whose `agreementProposalIdentifier`
/// names no ACTIVE agreement proposal is refused under.
///
/// `ValidationExceptionReason` publishes no value for the state of a proposal
/// that cannot be resolved at all: of the proposal it names, only
/// `EXPIRED_AGREEMENT_PROPOSAL` and `SUPERSEDED_AGREEMENT_PROPOSAL` — two
/// particular states of a proposal this service would have to hold to tell apart
/// ([`crate::state::agreement_proposals`] resolves one proposal, not its
/// history). So the refusal reports the value the model does publish for this
/// member, which is the one a client branches on for it
/// (`smithy_query(query="shape", shape="ValidationExceptionReason")`).
pub const REASON_INACTIVE_AGREEMENT_PROPOSAL: &str = "INVALID_AGREEMENT_PROPOSAL_IDENTIFIER";

/// The sentence AWS states when the agreement proposal a quote names is not an
/// active proposal.
///
/// Recorded behavior, and fixed text: it interpolates no value of the request,
/// no identifier and no instant — the member that carried the identifier is
/// named by the `fields` entry beside it. It is stated here rather than derived
/// from [`REASON_INACTIVE_AGREEMENT_PROPOSAL`] ([`validation_message`]) because
/// the class is about the PROPOSAL's state and that reason names the member: the
/// same reason also reports an identifier that misses the member's own modelled
/// pattern, which AWS words as the member being invalid.
pub const INACTIVE_AGREEMENT_PROPOSAL_MESSAGE: &str = "Provided agreement proposal is inactive.";

/// The refusal of an `agreementProposalIdentifier` that names no agreement
/// proposal this emulator can resolve as active.
///
/// The envelope is the unresolved-reference one every other identifier of this
/// service that resolves to nothing is refused in ([`unresolved_reference`]) —
/// the member's modelled reason and one `fields` entry naming it — carrying the
/// sentence AWS states for this class.
pub fn inactive_agreement_proposal() -> ValidationException {
    ValidationException {
        message: Some(INACTIVE_AGREEMENT_PROPOSAL_MESSAGE.to_string()),
        ..unresolved_reference(
            REASON_INACTIVE_AGREEMENT_PROPOSAL,
            "agreementProposalIdentifier",
        )
    }
}

/// Refuse a selector whose value lies outside the fixed set this service
/// publishes for that member, BEFORE anything is selected.///
/// A selector of this service is RESOLVED, not merely matched: `catalog` names a
/// catalog AWS Marketplace exposes — it exposes exactly one, [`CATALOG`] — and
/// `agreementType` one of the types the model documents
/// ([`AGREEMENT_TYPE_VALUES`]). A value outside either set names nothing this
/// service could ever hold, so it is not a filter that happens to select
/// nothing: it is a value the member does not admit, and the modelled refusal is
/// the `INVALID_CATALOG` / `INVALID_AGREEMENT_TYPE` reason a client branches on.
/// Both shapes are pattern-constrained strings rather than enums, so nothing
/// before a handler refuses them.
///
/// Neither check reads the store, so the answer is the same whatever the store
/// holds, including nothing at all. A value INSIDE the set is left to the
/// listing to match, so an `agreementType` no stored agreement carries still
/// reports an empty page.
pub fn selector_value_refusal(
    agreement_type: Option<&str>,
    catalog: Option<&str>,
) -> Option<ValidationException> {
    if catalog.is_some_and(|catalog| catalog != CATALOG) {
        return Some(validation_exception("INVALID_CATALOG", "catalog"));
    }
    if agreement_type.is_some_and(|kind| !AGREEMENT_TYPE_VALUES.contains(&kind)) {
        return Some(validation_exception("INVALID_AGREEMENT_TYPE", "agreementType"));
    }
    None
}

/// The `ValidationExceptionReason` a period no instant can fall in is reported
/// under: the ONE value `ValidationExceptionReason` publishes about a period at
/// all (`smithy_query(query="shape", shape="ValidationExceptionReason")`). The
/// catch-all [`REASON_OTHER`] would leave a client nothing to branch on, and no
/// other value names the condition.
///
/// So it is the reason for EVERY pair of bounds this service refuses as a
/// malformed period, whichever instant the pair bounds — the creation of a
/// billing adjustment request and the issuing of an invoice alike: the model
/// publishes no reason naming either of those periods, and one condition must
/// not read as two reasons depending on which listing was called.
pub const REASON_INVALID_PERIOD: &str = "INVALID_INVOICE_ADJUSTMENT_PERIOD";

/// Refuse a request that names BOTH ends of one instant's window with its lower
/// bound at or after its upper bound, BEFORE anything is selected.
///
/// An `after`/`before` pair of members bounds ONE instant of the records
/// reported, each bound exclusive — "after the specified timestamp" and "before
/// the specified timestamp" — so a lower bound at or after the upper one spans
/// no instant at all. Such a pair is a malformed period rather than a selector
/// that happens to select nothing: no record of any store could satisfy it,
/// including a store holding everything, so the request states a period rather
/// than selecting on one and is answered with the modelled
/// [`REASON_INVALID_PERIOD`] naming both members that bound it.
///
/// A window that spans any instant at all, and a request naming only one of the
/// two bounds (an open-ended period), stay selectors: they are left to the
/// listing to match, so an empty page remains a legitimate answer.
///
/// Nothing here reads the store, so the answer is the same whatever it holds.
pub fn time_window_refusal(
    after: Option<f64>,
    before: Option<f64>,
    after_member: &str,
    before_member: &str,
) -> Option<ValidationException> {
    let (Some(after), Some(before)) = (after, before) else {
        return None;
    };
    (after >= before).then(|| {
        validation_exception_naming(REASON_INVALID_PERIOD, &[after_member, before_member])
    })
}

/// The `ValidationException` for constraint violations the boundary validator
/// found in the request, BEFORE any handler ran.
///
/// Every modelled constraint — a required member left out, a value outside a
/// declared length, range, pattern or value set, at any depth of the input —
/// arrives here as a [`::smithy_server_runtime::Violation`] carrying the
/// offending member's dotted path, and is stated in this service's own modelled
/// envelope: `reason` from the FIRST violation's member (the exception carries
/// one reason, and that is the member it is about) and one `fields` entry per
/// offending MEMBER. A member that misses two constraints at once (an empty
/// string is both shorter than its minimum and outside its pattern) is one
/// member and is named once: the entry says which member to fix, so a second
/// identical entry carries nothing. The count is already bounded by the
/// validator's own walk, which stops collecting at its `MAX_VIOLATIONS`.
pub fn validation_exception_for(
    violations: &[::smithy_server_runtime::Violation],
) -> ValidationException {
    let reason = violations
        .first()
        .map(|violation| member_reason(&violation.path, violation_kind(violation)))
        .unwrap_or_else(|| REASON_OTHER.to_string());
    let mut named: Vec<&str> = Vec::with_capacity(violations.len());
    let mut fields: Vec<ValidationExceptionField> = Vec::with_capacity(violations.len());
    for violation in violations {
        if named.contains(&violation.path.as_str()) {
            continue;
        }
        named.push(&violation.path);
        fields.push(ValidationExceptionField {
            name: violation.path.clone(),
            message: violation_kind(violation).field_message().to_string(),
        });
    }
    ValidationException {
        request_id: Some(next_request_id()),
        message: Some(validation_message(&reason)),
        reason: Some(reason),
        fields: Some(fields),
    }
}

/// Which class a violation the boundary walk raised belongs to: a member that
/// carried no value at all, or one whose value misses a constraint the model
/// declares.
pub(super) fn violation_kind(violation: &::smithy_server_runtime::Violation) -> RefusalKind {
    match violation.kind {
        ::smithy_server_runtime::ViolationKind::MissingRequiredMember => RefusalKind::Missing,
        _ => RefusalKind::Invalid,
    }
}

/// The HTTP status this service answers a reference to an id that names no
/// resource with, for EVERY resource it holds.
///
/// `ResourceNotFoundException` is the shape such a reference is answered with —
/// it is the one that carries `resourceId` and `resourceType`, naming what was
/// looked for — and the model declares it `@httpError(404)`. The status this
/// service actually sends with that body is 400: the recorded AWS responses for
/// `AWSMPCommerceService_v20200301` answer an unknown agreement and an unknown
/// agreement request with HTTP 400 carrying the `ResourceNotFoundException`
/// body, so the model's `@httpError` understates what the wire does. The status
/// belongs to the SERVICE and not to those two resources — one condition cannot
/// read as two status classes depending on which of this service's resources was
/// looked for — so every lookup in this crate answers through [`not_found`].
/// Only the STATUS is this service's; the body and the error code stay the
/// model's, which is exactly what
/// [`crate::scaffolded::Scaffolded::ModeledWithStatus`] renders.
pub const NOT_FOUND_STATUS: u16 = 400;

/// Lift a not-found refusal into an operation's error channel at
/// [`NOT_FOUND_STATUS`], so every lookup that found nothing answers the one
/// status this service answers them with.
pub fn not_found<E>(error: E) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::modeled_with_status(error, NOT_FOUND_STATUS)
}

/// How this service words a reference to a resource that does not exist, for
/// the resource noun its message names.
///
/// One sentence per resource kind, naming the KIND and not the id — the id the
/// caller supplied is reported in `resourceId`, which is the modelled place for
/// it, so nothing about the request is lost.
pub(super) fn does_not_exist(resource: &str) -> String {
    format!("Provided {resource} does not exist.")
}

/// How this service states a request whose filters cannot hold together with
/// the rest of the request.
///
/// Not derived from a member the way [`validation_message`] derives its
/// sentence: no single member of such a request is the wrong one, so the
/// sentence is about the combination.
pub const UNSUPPORTED_FILTER_COMBINATION_MESSAGE: &str =
    "Provided combination of filters is not supported.";

/// The `ValidationExceptionReason` for filters that are each acceptable but
/// cannot hold together with the rest of the request
/// (`smithy_query(query="shape", shape="ValidationExceptionReason")` lists
/// `UNSUPPORTED_FILTERS`).
pub const REASON_UNSUPPORTED_FILTERS: &str = "UNSUPPORTED_FILTERS";

/// A `ValidationException` for a request whose members are each acceptable but
/// cannot hold together — the modelled `UNSUPPORTED_FILTERS` reason.
///
/// `path` is the member whose pairing with the filters is unsupported, so a
/// client still reads WHICH part of the request to change from `fields`, while
/// `message` states the combination rather than one bad member.
pub fn unsupported_filter_combination(path: impl Into<String>) -> ValidationException {
    ValidationException {
        request_id: Some(next_request_id()),
        message: Some(UNSUPPORTED_FILTER_COMBINATION_MESSAGE.to_string()),
        reason: Some(REASON_UNSUPPORTED_FILTERS.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: path.into(),
            message: RefusalKind::Invalid.field_message().to_string(),
        }]),
    }
}

/// How this service states a request for something it does not do for the
/// caller at all.
///
/// Not derived from a member the way [`validation_message`] derives its
/// sentence: no member of such a request is the wrong one — the request is
/// well formed and the action itself is unavailable — so the sentence is about
/// the action.
pub const UNSUPPORTED_ACTION_MESSAGE: &str = "This action is not supported.";

/// The `ValidationExceptionReason` for an action the service does not perform
/// for the caller (`smithy_query(query="shape",
/// shape="ValidationExceptionReason")` lists `UNSUPPORTED_ACTION`).
pub const REASON_UNSUPPORTED_ACTION: &str = "UNSUPPORTED_ACTION";

/// A `ValidationException` for a request the service will not act on however
/// the request is written — the modelled `UNSUPPORTED_ACTION` reason.
///
/// No `fields` entry: the refusal is of the whole request rather than of a
/// member of it, so there is no member for a caller to change, and the member
/// is optional in the modelled shape
/// (`smithy_query(query="shape", shape="ValidationException")`).
pub fn unsupported_action() -> ValidationException {
    ValidationException {
        request_id: Some(next_request_id()),
        message: Some(UNSUPPORTED_ACTION_MESSAGE.to_string()),
        reason: Some(REASON_UNSUPPORTED_ACTION.to_string()),
        fields: None,
    }
}

/// How this service states that a store already holds the most records this
/// emulator keeps: the maximum, the kind of record (a `ResourceType` value) and
/// what the caller can do about it.
///
/// The crate's ONE wording for that condition, whatever channel reports it — the
/// modelled exception a bounded create answers ([`store_full`]) and the
/// per-entry refusal a batch reports on its own `errors` channel state the same
/// sentence, differing only in the number and the noun of the store that filled
/// up.
pub fn store_full_message(resource_type: &str, quota: usize) -> String {
    format!(
        "this emulator already holds the maximum of {quota} {resource_type} records; \
         release some before creating another"
    )
}

/// The `ThrottlingException` a create answers when its store already holds the
/// most records the emulator keeps: the request is declined, and nothing is
/// stored, rather than letting state grow without end.
pub fn store_full(resource_type: &str, quota: usize) -> ThrottlingException {
    ThrottlingException {
        request_id: Some(next_request_id()),
        message: Some(store_full_message(resource_type, quota)),
    }
}

/// The `ResourceNotFoundException` for an agreement id that names no agreement.
pub fn agreement_not_found(id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        request_id: Some(next_request_id()),
        message: Some(does_not_exist("agreement")),
        resource_id: Some(id.to_string()),
        resource_type: Some(RESOURCE_TYPE_AGREEMENT.to_string()),
    }
}

/// The `ResourceNotFoundException` for an agreement request id that names none.
pub fn agreement_request_not_found(id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        request_id: Some(next_request_id()),
        message: Some(does_not_exist("agreement request")),
        resource_id: Some(id.to_string()),
        resource_type: Some(RESOURCE_TYPE_AGREEMENT_REQUEST.to_string()),
    }
}

/// The `ResourceNotFoundException` for an agreement and billing adjustment
/// request id pair that names no stored request.
pub fn billing_adjustment_not_found(id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        request_id: Some(next_request_id()),
        message: Some(does_not_exist("billing adjustment request")),
        resource_id: Some(id.to_string()),
        resource_type: Some(RESOURCE_TYPE_BILLING_ADJUSTMENT_REQUEST.to_string()),
    }
}

/// The `ResourceNotFoundException` for an agreement and payment request id pair
/// that names no stored payment request.
pub fn payment_request_not_found(id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        request_id: Some(next_request_id()),
        message: Some(does_not_exist("payment request")),
        resource_id: Some(id.to_string()),
        resource_type: Some(RESOURCE_TYPE_PAYMENT_REQUEST.to_string()),
    }
}

/// The `ResourceNotFoundException` for an agreement and cancellation request id
/// pair that names no stored cancellation request.
pub fn agreement_cancellation_request_not_found(id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        request_id: Some(next_request_id()),
        message: Some(does_not_exist("agreement cancellation request")),
        resource_id: Some(id.to_string()),
        resource_type: Some(RESOURCE_TYPE_AGREEMENT_CANCELLATION_REQUEST.to_string()),
    }
}

/// A `ConflictException` for an operation the resource's current state refuses.
pub fn conflict(resource_type: &str, id: &str, detail: impl Into<String>) -> ConflictException {
    ConflictException {
        request_id: Some(next_request_id()),
        message: Some(detail.into()),
        resource_id: Some(id.to_string()),
        resource_type: Some(resource_type.to_string()),
    }
}

/// The audit record a state-changing operation owes: which operation ran, what
/// it did, the resource it did it to, and the access key id the caller
/// presented (an identifier, never a secret).
///
/// Emitted under the `audit` target so a deployment can route these records
/// apart from the emulator's own diagnostics.
pub fn audit(operation: &str, outcome: &str, resource: &str, parts: &::http::request::Parts) {
    ::tracing::info!(
        target: "audit",
        operation,
        outcome,
        resource,
        principal = %::engine::extract_akid(&parts.headers),
        "marketplace agreement state change"
    );
}

/// The `InternalServerException` a handler answers when the engine does not
/// carry this service's store — never a freshly built engine.
pub fn store_unavailable() -> InternalServerException {
    InternalServerException {
        request_id: Some(next_request_id()),
        message: Some("the marketplace agreement store is unavailable".to_string()),
    }
}
