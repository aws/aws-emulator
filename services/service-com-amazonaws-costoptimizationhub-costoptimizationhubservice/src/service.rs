// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-costoptimizationhub-costoptimizationhubservice` service: how it claims requests and how it words the
//! failures the framework hands back.
//!
//! EDIT THIS FILE. It is emitted once with working defaults and is never
//! regenerated, so a choice made here survives adding another service or
//! re-running the generator.
//!
//! `EmulatedService` is what the router mounts (`EmulatedService::new(engine)`).
//! It wraps the generated [`ServiceDefinition`], which carries the model-derived half
//! of this service — its identity, its wire protocol, and its operation routing
//! table — built from the Smithy model at compile time. You do not edit the
//! entry; you edit the behavioral seams below.
//!
//! Five opt-in seams are yours to change; every one defaults to the framework's
//! own behaviour, so an unedited service still routes and answers correctly:
//!
//! - `can_handle` — which requests this service claims. The default matches the
//!   request's resolved route key against this service's signing name. Edit it
//!   for anything the model does not express: a signing name shared by several
//!   services, or routing that keys on the access-key id.
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. EDITED HERE: the key and the handler stay the generated table's,
//!   but every operation is served through [`InModelledEnvelope`], so a request
//!   refused for a modelled input constraint is answered in this service's own
//!   structured `ValidationException` instead of the framework's pre-dispatch
//!   envelope, which carries a `message` and nothing else.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.
//!
//! # This service's `ValidationException` is STRUCTURED
//!
//! `com.amazonaws.costoptimizationhub#ValidationException` is not a bare message:
//! besides the required `message` it carries `reason`
//! (`ValidationExceptionReason`: `FieldValidationFailed` | `Other`) and `fields`,
//! a list of `ValidationExceptionDetail{fieldName, message}`
//! (`smithy_query(query="shape", shape="ValidationException")`). A client maps a
//! refusal back onto the member that caused it by reading those two members, so a
//! 400 that carries only prose is unusable to it.
//!
//! The framework's pre-dispatch renderer gets the status (400) and the error code
//! (`ValidationException`) right, but it can only choose a code and a sentence —
//! it has no way to fill a modelled member. So [`InModelledEnvelope`] collects
//! the refusal itself, from the framework's own deserializer and constraint walk,
//! and answers it through the model's `ValidationException` schema with `reason`
//! set and one `fields` entry per invalid member. A refusal a handler raises for a rule
//! the model cannot express (an unknown `nextToken`, an `orderBy` dimension that
//! names no member) fills the same two members through
//! [`stated_validation_exception`], the one place that envelope is built. The
//! model's other reason (`Other`) names a refusal no field accounts for. Every
//! rule a handler of this service enforces over the REQUEST — including the
//! organization rules over a preference write — rejects a MEMBER the request
//! carries, so each of those refusals is attributed to one; a PRECONDITION on
//! the calling account rather than on the request is not, and carries neither
//! member ([`compute_optimizer_opt_in_required`]).
//!
//! # What the top-level `message` of a refusal says
//!
//! Three classes of refusal, three messages, because what is known about the
//! request differs:
//!
//! - a member outside a constraint the MODEL declares (a `@pattern`, a `@range`,
//!   an enum value set) is answered with [`FIELD_VALIDATION_SUMMARY`], the fixed
//!   summary that points at the field list the constraint walk produced
//!   ([`field_validation_exception`]);
//! - a precondition on the calling ACCOUNT that no member of the request can
//!   satisfy — the AWS Compute Optimizer opt-in a read of that service's
//!   analysis needs ([`compute_optimizer_opt_in_required`]) — states the
//!   operation called and what to enable, and carries no field list, because a
//!   well-formed request earns it and there is no member for a client to
//!   correct;
//! - a refusal a HANDLER raises for a rule of the operation the model cannot
//!   express — a filter member the operation does not support, a dimension that
//!   names no member, a pagination token this service never issued, a
//!   preference only the organization's management account may set — is built
//!   FROM the request, so its message states the rejected member and the rule
//!   that rejects it ([`stated_validation_exception`]), with the same per-member
//!   diagnosis still in `fields`. An SDK error surface commonly shows only
//!   `message`, so the actionable part of such a refusal belongs there. A rule
//!   every operation of a class enforces is written once here — the pagination
//!   token every paginated read judges is [`page`] — so the operations
//!   cannot drift apart in how they word one refusal.
//!
//! # A member's own value set or bound is the one its operation enforces
//!
//! What a member declares is not always what the generated schema carries: a
//! `@range` written on the member overrides the one on the shape it targets, and
//! the value set an enum member is judged against is the set the SERVICE accepts
//! on the wire, which can be wider than the published enum. Such a member's
//! traits are kept off the schema (`skip_validation`) and the operation checks
//! it through [`out_of_range`] or [`not_in_enum`], so the refusal is worded
//! exactly as the boundary walk would word it.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::serde::SerdeError;
use aws_smithy_schema_server::constraints;
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext,
    OperationRoutingTable, Refusal, Refused, ServerHttpProtocol, Service, Violation, ViolationKind,
};
use std::sync::Arc;

use crate::ops::types::shared_types::{
    ValidationException, ValidationExceptionDetail, SCHEMA_VALIDATION_EXCEPTION,
};

/// The wire error code this service answers a refused request with: the shape
/// `com.amazonaws.costoptimizationhub#ValidationException`, which every operation
/// of this service lists.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The HTTP status that shape's `@httpError` declares.
const VALIDATION_EXCEPTION_STATUS: u16 = 400;

/// The `ValidationExceptionReason` a member outside its declared constraint is
/// reported under — one of the two values the model's enum holds, the other
/// (`Other`) being for a refusal no field accounts for.
pub(crate) const REASON_FIELD_VALIDATION_FAILED: &str = "FieldValidationFailed";

/// The top-level `message` of a refusal whose detail is carried in `fields`: the
/// per-member diagnosis lives in the field list, so the summary points the reader
/// at it rather than restating one of the entries.
pub(crate) const FIELD_VALIDATION_SUMMARY: &str =
    "Input validation failed. Please check the field list for more details.";

/// The path segment the constraint walk spells a step from a collection into one
/// of its elements with (`aws_smithy_schema_server::constraints::path`), so
/// `filter.accountIds.1.member` is the first element of the `accountIds` list.
const COLLECTION_STEP: &str = "member";

/// The `com-amazonaws-costoptimizationhub-costoptimizationhubservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`InModelledEnvelope`] so a request refused before its handler runs is
    /// answered in this service's structured `ValidationException`. Keyed by the
    /// routing key the protocol resolves — the key the generated table uses — and
    /// built from the SAME shared engine, so a wrapped operation reads and writes
    /// the one store.
    in_modelled_envelope: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`InModelledEnvelope`] and keyed by its operation shape name — the routing key
/// `ServerHttpProtocol::routing_key` resolves and the generated table is keyed by.
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                stringify!($operation),
                ::std::boxed::Box::new(InModelledEnvelope::new(
                    crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
                )) as ::std::boxed::Box<dyn ErasedOperation>,
            );
        )+
    };
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        // Every operation of the service, not only the ones a recording exercised:
        // the refusal a modelled constraint owes is the same refusal whichever
        // operation declared the member, and each of these eight lists
        // `ValidationException`.
        let mut in_modelled_envelope = OperationRoutingTable::new();
        wrap_operations!(
            in_modelled_envelope,
            engine,
            get_preferences::GetPreferences,
            get_recommendation::GetRecommendation,
            list_efficiency_metrics::ListEfficiencyMetrics,
            list_enrollment_statuses::ListEnrollmentStatuses,
            list_recommendation_summaries::ListRecommendationSummaries,
            list_recommendations::ListRecommendations,
            update_enrollment_status::UpdateEnrollmentStatus,
            update_preferences::UpdatePreferences,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            in_modelled_envelope,
        }
    }
}

/// One modelled operation, served with the refusals it owes stated in this
/// service's own structured `ValidationException`.
///
/// The wrapper collects the refusal itself — the very two classes the framework's
/// own walk collects, from the framework's own deserializer and constraint walk —
/// and renders it through the model's `ValidationException` schema, so `reason`
/// and `fields` are filled from the structured [`Violation`]s. Nothing else is
/// re-implemented: a request this service does not refuse is handed to the
/// generated `ErasedOperation::run`, which owns deserialize -> validate -> handle
/// -> serialize, telemetry and every modelled error.
struct InModelledEnvelope<O> {
    operation: O,
}

impl<O> InModelledEnvelope<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for InModelledEnvelope<O>
where
    O: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // Read the protocol out before the await: `&dyn ErasedServiceDefinition`
            // is not `Sync`, and this future is. The protocol itself is.
            let protocol: &dyn ServerHttpProtocol = svc.definition().protocol();
            let violations = refused_by::<O>(protocol, req);
            if let Some(refusal) = validation_exception(protocol, &violations) {
                return refusal;
            }
            self.operation.run(svc, req, ctx).await
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not
    /// change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// Every violation that refuses `req` before `O`'s handler runs: a `@required`
/// member absent, or a value outside a declared constraint. Empty when the request
/// is valid, and empty for a failure that is NOT a refusal — a body the codec
/// cannot read is a serialization failure, which the framework answers in its own
/// envelope.
///
/// Both classes come from the framework's own machinery (the protocol's
/// deserializer and `constraints::validate` over the built input), so this cannot
/// find a different set of violations from the walk that follows it.
fn refused_by<O: Operation>(protocol: &dyn ServerHttpProtocol, req: &RtRequest) -> Vec<Violation> {
    let Ok(mut deserializer) = protocol.deserialize_input(req, O::INPUT_SCHEMA) else {
        return Vec::new();
    };
    match <O::Input as DeserializableStruct>::deserialize(&mut *deserializer) {
        Ok(input) => constraints::validate(&input, O::INPUT_SCHEMA),
        // Absence is found while READING the request and a bad value while walking
        // what was read; to a caller they are one refusal, so they are one here.
        Err(SerdeError::MissingMember { member_name }) => vec![Violation {
            path: member_name,
            kind: ViolationKind::MissingRequiredMember,
            value: None,
            sensitive: false,
        }],
        Err(_) => Vec::new(),
    }
}

/// This service's answer to `violations`: HTTP 400 `ValidationException` with
/// `reason` = `FieldValidationFailed` and one `fields` entry per invalid member,
/// serialized through the model's own `ValidationException` schema.
///
/// `None` when there is nothing to refuse, when a violation names a constraint
/// class this file cannot state as a field entry, and when the error cannot be
/// serialized — each of which leaves the framework's walk to refuse the request in
/// its own envelope rather than answering a 400 that drops part of the diagnosis.
fn validation_exception(
    protocol: &dyn ServerHttpProtocol,
    violations: &[Violation],
) -> Option<RtResponse> {
    let fields = field_details(violations)?;
    let refused = field_validation_exception(fields);
    protocol
        .serialize_error(
            &refused,
            &SCHEMA_VALIDATION_EXCEPTION,
            VALIDATION_EXCEPTION,
            VALIDATION_EXCEPTION_STATUS,
        )
        .ok()
}

/// This service's `ValidationException` for a member outside a constraint the
/// MODEL declares: the fixed summary ([`FIELD_VALIDATION_SUMMARY`]), `reason` =
/// `FieldValidationFailed`, and the per-member diagnosis in `fields`.
///
/// This is the envelope of a refusal whose whole diagnosis is the field list —
/// the constraint walk's own, and a bound or value set a handler enforces in the
/// walk's place ([`out_of_range`], [`not_in_enum`]) — so a handler-enforced
/// constraint reads exactly like a walk-enforced one.
pub(crate) fn field_validation_exception(
    fields: Vec<ValidationExceptionDetail>,
) -> ValidationException {
    stated_validation_exception(FIELD_VALIDATION_SUMMARY.to_string(), fields)
}

/// This service's `ValidationException` for a request a HANDLER refused for a
/// rule of the OPERATION the model cannot express: `statement` is the top-level
/// `message`, which names the rejected member and the rule that rejects it
/// because the refusal is built from the request, and `fields` carries the same
/// diagnosis attributed to that member.
///
/// The ONE place this service's `ValidationException` is built, so every refusal
/// of this service — whichever layer raised it — fills `reason` and `fields` the
/// same way; only the summary differs, and the note on `message` above says why.
pub(crate) fn stated_validation_exception(
    statement: String,
    fields: Vec<ValidationExceptionDetail>,
) -> ValidationException {
    ValidationException {
        message: statement,
        // Every refusal built here is attributed to a member of the request,
        // which is exactly what this reason names.
        reason: Some(REASON_FIELD_VALIDATION_FAILED.to_string()),
        fields: Some(fields),
    }
}

/// How this service words the refusal of a request for Cost Optimization Hub
/// data about accounts other than the caller's: one fixed sentence naming the
/// member accounts of the organization and the reason the caller cannot have
/// their data — it is not the organization's management account.
///
/// The sentence is the same for every read that refuses, whichever data was
/// asked for, so it takes no parameter: the refusal class is one class, and the
/// message of the modelled `AccessDeniedException` every organization-scoped
/// reader answers with is built here once.
pub(crate) fn organization_view_denied() -> String {
    "Cannot get enrollment statuses for member accounts of the organization. This is because \
     your account is not the management account of the organization."
        .to_string()
}

/// How this service words the refusal of a read whose figures come from AWS
/// Compute Optimizer's analysis when the calling account is not opted into that
/// service: the operation cannot be served at all until the account opts in, so
/// the message names the operation that was called and the service to enable.
///
/// `operation` is the operation serving the read — its [`Operation::NAME`] — and
/// is the only part of the sentence that varies: no account id, period or other
/// per-call value appears in it, because the precondition is the same whatever
/// the request asked for.
///
/// This refusal is attributed to NO member: the request can be perfectly
/// well-formed and still be refused, so it carries the statement alone rather
/// than the `reason`/`fields` diagnosis a rejected member earns
/// ([`stated_validation_exception`]).
pub(crate) fn compute_optimizer_opt_in_required(operation: &str) -> ValidationException {
    ValidationException {
        message: format!(
            "{operation} requires your account to be opted into AWS Compute Optimizer. Please \
             enable Compute Optimizer in the AWS Compute Optimizer console or via the Compute \
             Optimizer API."
        ),
        reason: None,
        fields: None,
    }
}

/// This service's refusal of a numeric member outside the range the calling
/// OPERATION declares for it, or `None` when the value is inside that range (and
/// when the member was absent, which is no value to bound).
///
/// A member's own `@range` is what bounds it, which for `maxResults` is the
/// operation's own page-size bound rather than the one the shared `MaxResults`
/// shape carries
/// (`smithy_query(query="operation", operation="ListRecommendations")`: `range`
/// min 0 max 1000). The clause the entry states comes from the same
/// [`requirement`] wording the constraint walk uses, so a handler-enforced
/// bound reads like a walk-enforced one.
pub(crate) fn out_of_range(
    member: &str,
    value: Option<i32>,
    min: i64,
    max: i64,
) -> Option<ValidationException> {
    let value = i64::from(value?);
    let kind = if value < min {
        ViolationKind::MinValue(min.to_string())
    } else if value > max {
        ViolationKind::MaxValue(max.to_string())
    } else {
        return None;
    };
    // `requirement` words both numeric kinds; the fallback is unreachable, and
    // is here only because `ViolationKind` is `#[non_exhaustive]`.
    let message = requirement(&kind)
        .unwrap_or_else(|| format!("Member must have value between {min} and {max}"));
    Some(field_validation_exception(vec![ValidationExceptionDetail {
        field_name: member.to_string(),
        message,
    }]))
}

/// This service's refusal of a string member whose value is outside the set the
/// service ACCEPTS for it, or `None` when the value is one of them.
///
/// `accepted` is the set the service takes on the wire, which is not always the
/// published enum's: `UpdateEnrollmentStatus.status` admits a terminal state the
/// model's `EnrollmentStatus` does not list
/// (`crate::state::ENROLLMENT_STATUS_WIRE_VALUES`). The accepted set is also the
/// set the entry advertises, and the clause comes from the same [`requirement`]
/// wording the constraint walk uses, so a handler-enforced value set reads like
/// a walk-enforced one.
pub(crate) fn not_in_enum(
    member: &str,
    value: &str,
    accepted: &[&str],
) -> Option<ValidationException> {
    if accepted.contains(&value) {
        return None;
    }
    let kind = ViolationKind::EnumValue(accepted.iter().map(|v| (*v).to_string()).collect());
    // `requirement` words this kind; the fallback is unreachable, and is here
    // only because `ViolationKind` is `#[non_exhaustive]`.
    let message = requirement(&kind).unwrap_or_else(|| {
        format!(
            "Member must satisfy enum value set: [{}]",
            accepted.join(", ")
        )
    });
    Some(field_validation_exception(vec![ValidationExceptionDetail {
        field_name: member.to_string(),
        message,
    }]))
}

/// The member a refused pagination token is attributed to: the one carrying the
/// token, so it is the member a caller has to change to get the request served.
const NEXT_TOKEN_MEMBER: &str = "nextToken";

/// The page a paginated read of `kind` answers out of `all`, for every paginated
/// operation of this service: no token asks for the first page, a token this
/// service issued for that list resumes where that list's previous page stopped
/// ([`crate::state::paginate`]), and anything else is refused, because a token
/// this list did not issue — one the service cannot read, or one carrying an
/// offset no page of the list ever ended at — names no row to resume at, and
/// answering it with a page would serve an invalid request as a listing.
///
/// `operation` is the operation serving the read — its [`Operation::NAME`] —
/// which is the list the token has to have been issued by, so the refusal states
/// which operation would have issued an acceptable one.
///
/// The one place this rule's two halves — resume or refuse — are written, so
/// every paginated operation of this service judges a token and words its
/// refusal the same way.
pub(crate) fn page<'a, T>(
    operation: &str,
    kind: &str,
    all: &'a [T],
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Result<crate::state::Page<'a, T>, ValidationException> {
    crate::state::paginate(kind, all, next_token, max_results).map_err(
        |crate::state::TokenNotIssued| {
            stated_validation_exception(
                format!("Invalid nextToken specified. The token was not issued by {operation}."),
                vec![ValidationExceptionDetail {
                    field_name: NEXT_TOKEN_MEMBER.to_string(),
                    message: format!("the value is not a pagination token issued by {operation}"),
                }],
            )
        },
    )
}

/// One `fields` entry per invalid MEMBER — `fields` is "the list of fields that
/// are invalid", so a member is named once however many of its values failed —
/// in the order the walk first reached each member.
///
/// `None` when there is no violation at all, or when any one of them states a
/// constraint class [`requirement`] cannot word: a partial field list would tell a
/// client the request is fine in a member that is not.
fn field_details(violations: &[Violation]) -> Option<Vec<ValidationExceptionDetail>> {
    if violations.is_empty() {
        return None;
    }
    let mut invalid: Vec<InvalidField> = Vec::new();
    for violation in violations {
        let (field_name, on_element) = attributed_member(&violation.path);
        let requirement = requirement(&violation.kind)?;
        match invalid.iter_mut().find(|held| held.name == field_name) {
            None => invalid.push(InvalidField {
                name: field_name,
                on_element,
                requirements: vec![requirement],
            }),
            Some(held) => {
                held.on_element |= on_element;
                // Two values of one list failing the same rule state that rule
                // once: the entry says what the member must satisfy, not how many
                // of its values did not.
                if !held.requirements.contains(&requirement) {
                    held.requirements.push(requirement);
                }
            }
        }
    }
    Some(invalid.into_iter().map(InvalidField::detail).collect())
}

/// One invalid member on its way to a `fields` entry: the member reported, and
/// every distinct requirement its values failed.
struct InvalidField {
    name: String,
    /// Whether any of the requirements was failed by an ELEMENT of the member
    /// rather than by the member itself.
    on_element: bool,
    requirements: Vec<String>,
}

impl InvalidField {
    /// The `fields` entry for this member.
    fn detail(self) -> ValidationExceptionDetail {
        // A requirement failed by an ELEMENT of the member, and a member that
        // failed several, are both stated as the bracketed list of what the member
        // must satisfy; a member that failed exactly one requirement of its own
        // states that requirement directly.
        let message = if self.on_element || self.requirements.len() > 1 {
            format!(
                "Member must satisfy constraint: [{}]",
                self.requirements.join(", ")
            )
        } else {
            self.requirements.join(", ")
        };
        ValidationExceptionDetail {
            field_name: self.name,
            message,
        }
    }
}

/// The member a violation is reported against, and whether it was found on an
/// element of that member rather than on the member itself.
///
/// A constraint walk reports a position, not a member: `accountId` for a scalar,
/// `filter.accountIds.1.member` for the first element of a list nested in a
/// structure. A field entry names a MEMBER, so the collection steps are dropped
/// (the `member` segment and the index or key before it) and the deepest remaining
/// segment — the deepest member the path names — is the one reported. A map key
/// holding a `.` is spelled by the caller and could not be told from a member
/// boundary; such a key keeps the segment it looks like, which still names a
/// member of the request.
fn attributed_member(path: &str) -> (String, bool) {
    let mut named: Vec<&str> = Vec::new();
    let mut on_element = false;
    for segment in path.split('.') {
        if segment == COLLECTION_STEP {
            // The segment before it is the element's position (a list) or its key
            // (a map): neither is a member name.
            named.pop();
            on_element = true;
            continue;
        }
        named.push(segment);
        on_element = false;
    }
    match named.last() {
        Some(member) => ((*member).to_string(), on_element),
        // Nothing but collection steps: report the position as the walk spelled it
        // rather than an empty field name.
        None => (path.to_string(), false),
    }
}

/// What a violated constraint requires, as a field entry states it. `None` for a
/// class this file does not word — [`ViolationKind`] is `#[non_exhaustive]`, so one
/// can be added upstream, and a clause invented for it would be wrong rather than
/// merely missing.
pub(crate) fn requirement(kind: &ViolationKind) -> Option<String> {
    Some(match kind {
        ViolationKind::MissingRequiredMember => "Member must not be null".to_string(),
        ViolationKind::MinLength(min) => {
            format!("Member must have length greater than or equal to {min}")
        }
        ViolationKind::MaxLength(max) => {
            format!("Member must have length less than or equal to {max}")
        }
        ViolationKind::MinValue(min) => {
            format!("Member must have value greater than or equal to {min}")
        }
        ViolationKind::MaxValue(max) => {
            format!("Member must have value less than or equal to {max}")
        }
        // The expression is stated without the anchors the model spells it with:
        // the clause already says the whole member must satisfy it, so the anchors
        // would be restating the sentence.
        ViolationKind::Pattern(pattern) => format!(
            "Member must satisfy regular expression pattern: {}",
            unanchored(pattern)
        ),
        ViolationKind::EnumValue(allowed) => {
            format!("Member must satisfy enum value set: [{}]", allowed.join(", "))
        }
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        _ => return None,
    })
}

/// A `@pattern` expression without the anchors a model may spell it with. A `$`
/// escaped by a backslash is a literal dollar sign inside the expression, not the
/// end anchor, so it stays.
fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        Some(trimmed) => {
            let backslashes = trimmed.len() - trimmed.trim_end_matches('\\').len();
            if backslashes % 2 == 0 {
                trimmed
            } else {
                body
            }
        }
        None => body,
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // Claim a request whose resolved route key is this service's signing
        // name. Edit this for anything the model does not express — a signing
        // name shared by several services, or access-key-id-based routing.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // NOT a re-route: the key is the one the generated table's own protocol
        // resolves, and the operation it names is the one served — wrapped in
        // `InModelledEnvelope` so a request refused for a modelled constraint is
        // answered in this service's structured `ValidationException` rather than
        // in the framework's pre-dispatch envelope, which can carry no `reason`
        // and no `fields`. A key the model does not name resolves to `None` here
        // exactly as it does in the generated table, so the router's
        // routing-error path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.in_modelled_envelope.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // `InModelledEnvelope` answers a constraint refusal itself, in the
        // modelled envelope, so this seam would only reword what the FRAMEWORK
        // still renders: a refusal whose violation class `requirement` cannot
        // state, or an error the modelled shape could not be serialized into.
        // Both keep the framework's wording deliberately — it names the member
        // and the constraint in prose, which is strictly more than a summary
        // pointing at a `fields` list this path cannot build.
        let _ = refusal;
        None
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
        // word that routing failure in your own envelope instead.
        let _ = req;
        None
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. Returning `None` keeps the entry's protocol-correct
        // rendering. Return `Some(resp)` to reword that failure in your own
        // envelope. The error is borrowed, so the default renderer can still
        // consume it on `None`.
        let _ = error;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The member a violation is attributed to is the deepest member its path
    /// names, with the collection steps the walk spells (`<index>.member`,
    /// `<key>.member`) dropped — and a violation found on an element is marked as
    /// such, because the clause that states it names the holding member.
    #[test]
    fn attribution_names_the_deepest_member_of_the_path() {
        let cases: [(&str, &str, bool); 6] = [
            ("accountId", "accountId", false),
            ("status", "status", false),
            ("filter.accountIds.1.member", "accountIds", true),
            ("filter.accountIds", "accountIds", false),
            ("filter.tags.2.member.key", "key", false),
            ("member", "member", false),
        ];
        for (path, expected_member, expected_on_element) in cases {
            let (member, on_element) = attributed_member(path);
            assert_eq!(
                (member.as_str(), on_element),
                (expected_member, expected_on_element),
                "case {path:?}: expected ({expected_member:?}, {expected_on_element}), \
                 got ({member:?}, {on_element})"
            );
        }
    }

    /// A `@pattern` is stated without the anchors that make the match total, and a
    /// `$` the expression escapes is kept because it is a literal dollar sign.
    #[test]
    fn a_pattern_is_stated_without_its_anchors() {
        let cases: [(&str, &str); 5] = [
            ("^[0-9]{12}$", "[0-9]{12}"),
            ("[0-9]{12}", "[0-9]{12}"),
            ("^[a-z]+", "[a-z]+"),
            ("^cost\\$$", "cost\\$"),
            ("^a\\\\$", "a\\\\"),
        ];
        for (pattern, expected) in cases {
            let actual = unanchored(pattern);
            assert_eq!(
                actual, expected,
                "case {pattern:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
