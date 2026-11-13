// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-cloudtrail-cloudtrail-20131101` service: how it claims requests and how it words the
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
//! - `can_handle` — which requests this service claims. EDITED HERE: the framework's
//!   default (the request's resolved route key equals this service's signing name),
//!   plus a request whose `X-Amz-Target` names this service, which the framework's
//!   routing-key fallback misreads for this service's dotted target prefix.
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. The default returns `None`, so every request resolves through the
//!   generated routing table. Return `Some(op)` to route one yourself. THIS service
//!   does: its `X-Amz-Target` prefix is the fully qualified Coral service name
//!   (`com.amazonaws.cloudtrail.v20131101.CloudTrail_20131101`), so the operation is
//!   the segment after the LAST dot of the target rather than after the first. The
//!   operation that segment names is served through [`InModelledEnvelope`] when its
//!   input carries a tag list, so a tag outside its modelled length is refused in
//!   this service's own error shape rather than in the framework's envelope.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. EDITED HERE: a tag key or value outside its modelled
//!   length is `InvalidTagParameterException`; a value inside an event selector outside
//!   its modelled constraint is `InvalidEventSelectorsException`, the error the model
//!   declares for that case; and every other input-constraint refusal is the
//!   `ValidationException` envelope with the violated bound stated WITHOUT echoing the
//!   offending value, which the framework's own clause renderer cannot do. See the
//!   inline notes on that method.
//!   That seam carries only a code and a message, not a SHAPE, so a refusal leaving
//!   through it still carries its prose in the framework's pre-dispatch envelope, whose
//!   key is a lowercase `message`; CloudTrail's error shapes carry `Message`. Every
//!   operation whose input carries a tag list is therefore ALSO served through
//!   [`InModelledEnvelope`] below, which states the tag refusal in the modelled
//!   `InvalidTagParameterException` shape — see `override_operation`.
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

pub mod authorization;
pub mod management_events;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::constraints;
use service_common::audited_operation::AuditedDefinition;
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, ErasedServiceDefinition, Operation,
    OperationContext, Refusal, Refused, ServerHttpProtocol, Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The error code this service refuses a tag outside its modelled length with: the
/// shape `com.amazonaws.cloudtrail#InvalidTagParameterException`, "thrown when the
/// specified tag key or values are not valid"
/// (`smithy_query(query="shape", shape="InvalidTagParameterException")`).
const INVALID_TAG_PARAMETER_EXCEPTION: &str = "InvalidTagParameterException";

/// The HTTP status that shape's `@httpError` declares.
const INVALID_TAG_PARAMETER_EXCEPTION_STATUS: u16 = 400;

/// The error code this service refuses a `MaxResults` outside its modelled range
/// with: the shape `com.amazonaws.cloudtrail#InvalidMaxResultsException`, "thrown if
/// the limit specified is not valid"
/// (`smithy_query(query="shape", shape="InvalidMaxResultsException")`).
const INVALID_MAX_RESULTS_EXCEPTION: &str = "InvalidMaxResultsException";

/// The HTTP status that shape's `@httpError` declares.
const INVALID_MAX_RESULTS_EXCEPTION_STATUS: u16 = 400;

/// The member path of `LookupEventsRequest$EventCategory`, as the framework spells a
/// member path (lowerCamel, dotted from the request's root). The match is on the WHOLE
/// path, not its last segment: `AggregationConfiguration` carries an `EventCategory`
/// member too, and that one belongs to another operation's refusal.
const EVENT_CATEGORY_PATH: &str = "eventCategory";

/// The member path prefix of `LookupEventsRequest$LookupAttributes` — the list, its
/// elements and their members, which no other request shape carries.
const LOOKUP_ATTRIBUTES_PATH: &str = "lookupAttributes";

/// The wire error code of the modelled `InvalidEventCategoryException`: "Occurs if an
/// event category that is not valid is specified as a value of EventCategory."
const INVALID_EVENT_CATEGORY_CODE: &str = "InvalidEventCategoryException";

/// The wire error code of the modelled `InvalidLookupAttributesException`: "Occurs when
/// a lookup attribute is specified that is not valid."
const INVALID_LOOKUP_ATTRIBUTES_CODE: &str = "InvalidLookupAttributesException";

/// The target rule in isolation, as THIS service's prefix forms exercise it: which
/// operation an `X-Amz-Target` names, over the prefix forms a client can write.
///
/// The rule itself lives in `service_common::rpc_target`, which states it once for every
/// service whose published prefix carries dots; this table pins the answers for this
/// service's own shape name, and the wire behaviour they produce is asserted end-to-end
/// in `tests/all/target_prefix_dispatch.rs`.
#[cfg(test)]
mod rpc_target;

/// The error code this service answers an input-constraint violation with. It is also
/// the code the framework would have chosen, restated here because `override_refusal`
/// replaces the code and the message together.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// Render one violation the way CloudTrail states it.
///
/// A clause of a class this service has a RECORDED answer for names the member path and
/// the requirement it failed, and NOTHING of the value that was rejected:
///
/// * a violated `@pattern` states the pattern as the MODEL writes it, anchors included —
///   the recorded answer to a `ResourceArn` outside its pattern is `1 validation error
///   detected: Value at 'resourceArn' failed to satisfy constraint: Member must satisfy
///   regular expression pattern: ^[a-zA-Z0-9._/\-:]+$`;
/// * a violated `@range` states the bound it broke — the recorded answer to an
///   `UpdateEventDataStore` `RetentionPeriod` above the modelled maximum is `1
///   validation error detected: Value at 'retentionPeriod' failed to satisfy constraint:
///   Member must have value less than or equal to 3653`;
/// * a value outside an enumerated type's value set states the set the model declares —
///   the recorded answer to a `ListInsightsData` `DataType` outside its enum is `1
///   validation error detected: Value at 'dataType' failed to satisfy constraint: Member
///   must satisfy enum value set: [InsightsEvents]`. An enum violation INSIDE an event
///   selector never reaches here: [`invalid_event_selectors`] is read first and answers
///   it in the error that model declares.
///
/// Every other class keeps the framework's own sentence ([`Violation::message`], value
/// and all): these three are the classes this service has recorded wording for, and a
/// class with no recorded wording is better left as the framework states it than
/// reworded on a guess.
///
/// Why neither shared seam serves this service, which is what the duplication below
/// costs and what would retire it:
///
/// * `service_common::validation_message::aws_validation_refusal` is the shared
///   `override_refusal` body, but it states a pattern requirement UNANCHORED
///   (`service_common::validation::unanchored`) and KEEPS the rejected value — the two
///   things CloudTrail's recorded answers do not do, and the exact `$.message`
///   divergence this fix removed. A service whose recorded answer differs on both
///   counts cannot call it.
/// * `aws_smithy_schema_server::constraints::envelope` IS public, so the envelope could
///   come from there — but it renders every clause through [`Violation::message`], whose
///   only value-free form is `Value null at '<path>'` (an ABSENT member), not the
///   recorded `Value at '<path>'`. There is no public seam for "this value, unquoted",
///   because `Violation::requirement` is private and `vendor/` is read-only here.
///
/// MIGRATION NOTE (the one `services/common/src/validation.rs` asks every new inliner
/// to leave): the same value-free clause rendering is inlined in
/// `services/service-com-amazonaws-b2bi-b2bi/src/service.rs`,
/// `services/service-com-amazonaws-emrserverless-awstoledowebservice/src/service.rs`
/// and `services/service-com-amazonaws-personalizeruntime-amazonpersonalizeruntime/src/service.rs`,
/// over the same `N validation error(s) detected: …` envelope. The fix that owns the
/// migration adds the one missing seam — a public `Violation::message_without_value()`,
/// or a `services/common` clause renderer taking the requirement from the framework —
/// and replaces every copy, this one included, with a call to it.
fn clause(violation: &Violation) -> String {
    match &violation.kind {
        ViolationKind::Pattern(pattern) => format!(
            "Value at '{}' failed to satisfy constraint: Member must satisfy regular \
             expression pattern: {pattern}",
            violation.path
        ),
        ViolationKind::MinValue(min) => format!(
            "Value at '{}' failed to satisfy constraint: Member must have value greater \
             than or equal to {min}",
            violation.path
        ),
        ViolationKind::MaxValue(max) => format!(
            "Value at '{}' failed to satisfy constraint: Member must have value less than \
             or equal to {max}",
            violation.path
        ),
        ViolationKind::EnumValue(allowed) => format!(
            "Value at '{}' failed to satisfy constraint: Member must satisfy enum value \
             set: [{}]",
            violation.path,
            allowed.join(", ")
        ),
        _ => violation.message(),
    }
}

/// Whether `violation` is a class this service rewords value-free: a violated `@pattern`,
/// a violated `@range` bound, or a value outside an enumerated type's value set (see
/// [`clause`] for the recorded answer behind each).
fn is_value_free(violation: &Violation) -> bool {
    matches!(
        violation.kind,
        ViolationKind::Pattern(_)
            | ViolationKind::MinValue(_)
            | ViolationKind::MaxValue(_)
            | ViolationKind::EnumValue(_)
    )
}

/// The `ValidationException` message for a refused request: the AWS validation envelope
/// (`N validation error(s) detected: `) over one [`clause`] per violation.
///
/// `None` declines the refusal, leaving the framework's wording, for a request that
/// broke no class this service rewords ([`is_value_free`]) — there is nothing to reword
/// there. A request that mixes one of those classes with another IS reworded, so the
/// reworded clause drops its value while every other clause reads exactly as the
/// framework would have stated it.
fn value_free_envelope(violations: &[Violation]) -> Option<String> {
    if !violations.iter().any(is_value_free) {
        return None;
    }
    let clauses: Vec<String> = violations.iter().map(clause).collect();
    match clauses.as_slice() {
        [] => None,
        [only] => Some(format!("1 validation error detected: {only}")),
        many => Some(format!(
            "{} validation errors detected: {}",
            many.len(),
            many.join("; ")
        )),
    }
}

/// The part of a tag a constraint-violation path names — `key` or `value` — or `None`
/// for a path that does not walk into a tag at all.
///
/// The validating walk spells a path in lowerCamel, with a `<1-based-index>.member`
/// step from a list into one of its elements, so the value of the second tag of a
/// tagging request's `TagsList` is `tagsList.2.member.value`, and a key in
/// `CreateChannel`'s `Tags` is `tags.1.member.key`. Those two member names are every
/// tag list this service's model declares.
fn tag_member(path: &str) -> Option<&'static str> {
    let collection = path.split('.').next()?;
    if !matches!(collection, "tagsList" | "tags") {
        return None;
    }
    match path.rsplit('.').next()? {
        "key" => Some("key"),
        "value" => Some("value"),
        _ => None,
    }
}

/// How CloudTrail states a tag key or value that broke its modelled `@length`
/// maximum, or `None` for a violation that is not a tag's length.
///
/// CloudTrail does not answer a tag outside the modelled `TagKey` / `TagValue` length
/// with the framework's generic `ValidationException`: a tag key or value outside its
/// bound is `InvalidTagParameterException`, the shape the model declares for "the
/// specified tag key or values are not valid", worded from the bound the value broke.
/// The recorded corpus pins it — `AddTags` with an over-long tag value answers HTTP 400
/// `InvalidTagParameterException` "Tag value length cannot be more than 256
/// characters." — and the rule holds for every operation that takes a tag, not only the
/// tagging pair.
///
/// This is the ONE wording of that refusal, read by both places it can leave from: the
/// modelled envelope [`InModelledEnvelope`] serializes, and the framework's
/// pre-dispatch envelope [`invalid_tag_parameter`] rewords for anything that wrapper
/// could not read. The bound itself comes from the violation, so the sentence cannot
/// drift from the model's `TagKey` / `TagValue` lengths.
fn tag_length_refusal(violation: &Violation) -> Option<String> {
    let ViolationKind::MaxLength(max) = violation.kind else {
        return None;
    };
    let part = tag_member(&violation.path)?;
    Some(format!(
        "Tag {part} length cannot be more than {max} characters."
    ))
}

/// The refusal a tag outside its modelled `@length` is answered with, if `violations`
/// carries one — the [`Refusal`] seam's form of [`tag_length_refusal`].
///
/// The seam carries a code and a message, not the error SHAPE the envelope is rendered
/// from, so a refusal leaving through here still carries its prose in the framework's
/// lowercase `message` key. [`InModelledEnvelope`] is what answers in the modelled
/// shape, and it covers every operation whose input carries a tag list; what is left
/// for this reader is a request that wrapper could not read as an input at all, whose
/// code a client branches on stays right rather than degrading to a shape this model
/// does not declare.
fn invalid_tag_parameter(violations: &[Violation]) -> Option<Refused> {
    violations.iter().find_map(|violation| {
        Some(Refused {
            code: INVALID_TAG_PARAMETER_EXCEPTION.to_string(),
            message: tag_length_refusal(violation)?,
        })
    })
}

/// How CloudTrail states a `MaxResults` that broke its modelled `@range`, or `None`
/// for a violation that is not that member's bound.
///
/// A paged operation whose model declares `InvalidMaxResultsException` does not answer
/// a limit outside its range with the framework's generic `ValidationException`: the
/// shape's own doc is "this exception is thrown if the limit specified is not valid",
/// and `ValidationException` is not an error those operations declare, so a client
/// catching the modelled error would miss the refusal altogether. The message is built
/// from the request — the member and the bound it broke, the bound read off the
/// violation so the sentence cannot drift from the model's range.
///
/// Only the operation's own page-size member is read here ([`PAGE_SIZE_MEMBER`]): a
/// page size is the one value this error speaks for, and a bounded integer the
/// operation does not declare as its page size keeps the generic envelope.
fn max_results_refusal(violation: &Violation) -> Option<String> {
    if violation.path != PAGE_SIZE_MEMBER {
        return None;
    }
    let bound = match &violation.kind {
        ViolationKind::MinValue(min) => format!("greater than or equal to {min}"),
        ViolationKind::MaxValue(max) => format!("less than or equal to {max}"),
        // Every other class is not a limit outside its range, so it is not what this
        // shape speaks for and keeps the framework's own refusal.
        _ => return None,
    };
    Some(crate::state::limit_not_valid_message(
        &violation.path,
        &bound,
    ))
}

/// How the validating walk spells the page-size member of the operations wired to
/// [`ModelledRefusal::InvalidMaxResults`]: lowerCamel, at the top level of the input.
///
/// It is the member each of those operations declares as its `@paginated` `pageSize`
/// (`smithy_query(query="operation", operation="ListQueries,ListEventDataStores")`),
/// which is the value "the limit specified" names. A bounded integer an operation does
/// NOT declare as its page size — `GetQueryResults`' `MaxQueryResults`, whose
/// `@paginated` trait names only the token members — is not that limit, and keeps the
/// generic input-validation envelope.
const PAGE_SIZE_MEMBER: &str = "maxResults";

/// Which event-selector member a constraint-violation path walks into, or `None` for a
/// path that walks into neither.
///
/// The validating walk spells a path in lowerCamel, with a `<1-based-index>.member`
/// step from a list into one of its elements, so an out-of-set `ReadWriteType` on the
/// first basic selector of a `PutEventSelectors` request is
/// `eventSelectors.1.member.readWriteType`. `EventSelectors` and
/// `AdvancedEventSelectors` are the two members that carry a trail's event selectors,
/// and every operation the model binds them on declares
/// `InvalidEventSelectorsException`.
fn event_selector_member(path: &str) -> Option<&'static str> {
    match path.split('.').next()? {
        "eventSelectors" => Some("EventSelectors"),
        "advancedEventSelectors" => Some("AdvancedEventSelectors"),
        _ => None,
    }
}

/// The refusal a value inside an event selector that is outside its modelled
/// constraint is answered with, if `violations` carries one.
///
/// CloudTrail does not answer such a value with the framework's generic
/// `ValidationException`: the model's own `InvalidEventSelectorsException` doc names
/// this case as one of its own — "Specify a valid value for a parameter. For example,
/// specifying the `ReadWriteType` parameter with a value of `read-only` is not valid."
/// — and `ValidationException` is not an error any operation carrying those members
/// declares, so a client catching the modelled error would miss this refusal
/// altogether. The rule holds for every value inside either selector member, not only
/// `ReadWriteType`, and for every operation that carries them (`PutEventSelectors`,
/// `UpdateTrail`, `CreateEventDataStore`, `UpdateEventDataStore`), each of which
/// declares that error.
///
/// The message is built from the request: a value outside the SET its member admits — an
/// enumerated member's value set here — is refused in this service's one wording for that
/// class ([`crate::state::value_not_valid_message`]), naming the parameter and echoing the
/// value as the request sent it. Every other class keeps the framework's own sentence,
/// which already names the member and the bound it broke.
fn invalid_event_selectors(violations: &[Violation]) -> Option<Refused> {
    violations.iter().find_map(|violation| {
        let member = event_selector_member(&violation.path)?;
        let message = match &violation.kind {
            ViolationKind::EnumValue(_) => crate::state::value_not_valid_message(
                &refused_parameter(&violation.path, member),
                violation.value.as_deref().unwrap_or_default(),
            ),
            _ => clause(violation),
        };
        Some(Refused {
            code: "InvalidEventSelectorsException".to_string(),
            message,
        })
    })
}

/// How a refusal names the parameter a constraint-violation path points at, as CloudTrail
/// names one: the parameter's API path INSIDE the selector it sits in
/// (`ReadWriteType`, `DataResources.Type`), in the model's own casing.
///
/// Two things are dropped from the walk's path, because neither is part of the parameter's
/// name: the top-level list member the selectors arrived in (`top_level`, which is what the
/// message is about anyway) and each step from a collection into one of its entries — the
/// position and the [`COLLECTION_STEP`] marker — since the parameter is the same one
/// whichever entry carried the value. What remains is re-cased from the walk's lowerCamel
/// spelling to the model's own, which is the first character and nothing else. A path that
/// names the list member itself leaves nothing inside it, and is named by that member.
fn refused_parameter(path: &str, top_level: &'static str) -> String {
    let segments: Vec<&str> = path.split('.').collect();
    let mut named: Vec<String> = Vec::new();
    for (position, segment) in segments.iter().enumerate().skip(1) {
        let collection_step = *segment == COLLECTION_STEP
            || segments.get(position + 1) == Some(&COLLECTION_STEP);
        if !collection_step {
            named.push(modelled_member_name(segment));
        }
    }
    if named.is_empty() {
        return top_level.to_string();
    }
    named.join(".")
}

/// The segment the validating walk writes between a collection and one of its entries
/// ([`aws_smithy_schema_server::constraints::path`]), which no parameter is named by.
const COLLECTION_STEP: &str = "member";

/// A member name as the MODEL spells it, from the lowerCamel spelling a violation path
/// carries it in: the walk lower-cases the first character of the Smithy name and changes
/// nothing else, so upper-casing that one character recovers the model's name.
fn modelled_member_name(segment: &str) -> String {
    let mut characters = segment.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
        None => String::new(),
    }
}

/// The `com-amazonaws-cloudtrail-cloudtrail-20131101` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    /// The generated entry, routing through this service's AUDITED operations: every call
    /// it serves is a management event of this service, recorded in the account's event
    /// history where `LookupEvents` reads it (see [`management_events`]). Identity, the
    /// protocol and every framework error renderer stay the generated entry's.
    entry: AuditedDefinition<crate::ServiceDefinition<Eng>>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        // Every operation of this service, served through the audit recorder — and, for
        // the operations that need it, through [`InModelledEnvelope`] INSIDE that
        // recorder. Which operations are wrapped in which modelled refusal is stated
        // once, in [`management_events::audited_table`]:
        //
        // * the operations whose input carries a tag list — the `TagsList` of the
        //   tagging pair and of the three creates that seed tags, and `CreateChannel`'s
        //   `Tags` (`smithy_query(query="digest")`). An operation that takes no tag
        //   cannot break a tag's length, so it is not wrapped;
        // * the operations whose model declares `InvalidMaxResultsException` for the
        //   member it declares as its `@paginated` `pageSize`
        //   (`smithy_query(query="operation", operation="ListQueries,ListEventDataStores")`).
        //   Three paged operations are deliberately absent there:
        //
        //   - a paged operation whose model does NOT declare the shape keeps the
        //     framework's own refusal, because answering a code its model does not carry
        //     would be the same defect in the other direction;
        //   - `GetQueryResults`, whose `MaxQueryResults` its `@paginated` trait does not
        //     name as a page size at all, so a value outside that member's range is the
        //     generic `ValidationException` envelope the framework renders;
        //   - `LookupEvents`, whose maximum is stated of the results RETURNED ("The
        //     default number of results returned is 50, with a maximum of 50 possible"),
        //     so a larger limit is served with a capped page instead of being refused —
        //     the member carries `skip_validation` and the handler caps the page.
        let audited = management_events::audited_table(&engine);
        Self {
            entry: AuditedDefinition::new(crate::ServiceDefinition::new(engine), audited),
        }
    }

    /// This service's model shape id, as the generated identity states it
    /// (`com.amazonaws.cloudtrail#CloudTrail_20131101`), whose LOCAL name is the last
    /// segment of every `X-Amz-Target` prefix a client writes for it. Read off the model
    /// rather than written out, so it cannot drift from it.
    fn shape_id(&self) -> &str {
        self.entry.identity().shape_id()
    }
}

/// One modelled operation, served with the tag refusal that precedes its handler
/// stated in the shape CloudTrail states it in.
///
/// A tag key or value outside the modelled `TagKey` / `TagValue` length is refused
/// before any handler runs, by the framework's own constraint walk. The framework's
/// pre-dispatch envelope gets the status right (400) but names the error type
/// `ValidationException` — a shape this model does not declare — and writes the prose
/// into a lowercase `message` key, while every error shape CloudTrail declares carries
/// `Message`. [`Service::override_refusal`] can replace that code and that message,
/// but not the SHAPE the envelope is rendered from, so the one way to answer in the
/// modelled shape is to refuse ahead of that walk — which is what this does.
///
/// The violations are the very ones the framework's own walk collects, from the
/// framework's own deserializer and constraint walk, so this cannot find a different
/// set from the walk that follows it. Nothing else is re-implemented: a request this
/// wrapper does not refuse is handed to the generated [`ErasedOperation::run`], which
/// owns deserialize -> validate -> handle -> serialize, telemetry and every modelled
/// error.
struct InModelledEnvelope<O> {
    operation: O,
    /// Which refusal this operation states in a shape its own model declares.
    refusal: ModelledRefusal,
}

/// A refusal that precedes a handler and that CloudTrail answers in a shape the
/// operation's own model declares, rather than in the framework's generic
/// `ValidationException`.
///
/// One variant per such shape. Which operations each is wired to is stated once, where
/// they are wrapped in [`EmulatedService::new`]: an operation is wrapped for a variant
/// only when its model declares that error, so this can never answer a code the
/// operation does not carry.
#[derive(Debug, Clone, Copy)]
enum ModelledRefusal {
    /// A tag key or value outside the modelled `TagKey` / `TagValue` length — see
    /// [`tag_length_refusal`].
    InvalidTagParameter,
    /// A `MaxResults` outside the modelled range of the operation's page-size member
    /// — see [`max_results_refusal`].
    InvalidMaxResults,
}

impl<O> InModelledEnvelope<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O, refusal: ModelledRefusal) -> Self {
        Self { operation, refusal }
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
            if let Some(refused) = refused_before_handling::<O>(protocol, req, self.refusal) {
                return refused;
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

/// The modelled error `refusal` states for `req`, when the request breaks the rule
/// that refusal speaks for; `None` when it does not, so the request reaches the
/// handler.
///
/// `None` also when the request cannot be read at all — a body the codec rejects, or
/// one missing a required member, carries no value to hold to its constraint, and the
/// framework's own walk words that refusal — and when the chosen error cannot be
/// serialized, which leaves that walk to refuse the request rather than answering with
/// no reason at all.
///
/// A request this does not refuse is deserialized and walked a SECOND time by the
/// generated `ErasedOperation::run`. That is the cost of stating the refusal in the
/// modelled shape today: the framework's refusal seam carries a [`Refused`] code and
/// message, which cannot name the error SHAPE the envelope is rendered from. Should
/// `Refused` ever carry a shape, this layer can be dropped for `override_refusal` and
/// the second walk goes away with it.
fn refused_before_handling<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
    refusal: ModelledRefusal,
) -> Option<RtResponse> {
    let mut deserializer = protocol.deserialize_input(req, O::INPUT_SCHEMA).ok()?;
    let input = <O::Input as DeserializableStruct>::deserialize(&mut *deserializer).ok()?;
    drop(deserializer);
    let violations = constraints::validate(&input, O::INPUT_SCHEMA);
    match refusal {
        ModelledRefusal::InvalidTagParameter => {
            let message = violations.iter().find_map(tag_length_refusal)?;
            protocol
                .serialize_error(
                    &crate::ops::types::shared_types::InvalidTagParameterException {
                        message: Some(message),
                    },
                    &crate::ops::types::shared_types::SCHEMA_INVALID_TAG_PARAMETER_EXCEPTION,
                    INVALID_TAG_PARAMETER_EXCEPTION,
                    INVALID_TAG_PARAMETER_EXCEPTION_STATUS,
                )
                .ok()
        }
        ModelledRefusal::InvalidMaxResults => {
            let message = violations.iter().find_map(max_results_refusal)?;
            protocol
                .serialize_error(
                    &crate::ops::types::shared_types::InvalidMaxResultsException {
                        message: Some(message),
                    },
                    &crate::ops::types::shared_types::SCHEMA_INVALID_MAX_RESULTS_EXCEPTION,
                    INVALID_MAX_RESULTS_EXCEPTION,
                    INVALID_MAX_RESULTS_EXCEPTION_STATUS,
                )
                .ok()
        }
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
        if smithy_server_runtime::resolve_route_key(parts)
            .is_some_and(|key| key == self.entry.identity().signing_name())
        {
            return true;
        }
        // A request that carries no SigV4 credential scope is routed by its
        // `X-Amz-Target` before its `Host`, and the framework reads that signal as the
        // segment before the FIRST dot — which, for this service's fully qualified
        // target prefix, is a namespace label (`com`) that names no service. The target
        // still names THIS service, so read it by the same rule the operation is read
        // by: `service_common::rpc_target`, this emulator's one statement of the target
        // rule (the service is the last segment of the prefix, the operation the segment
        // after it).
        service_common::rpc_target::target_operation_of(&parts.headers, self.shape_id())
            .is_some()
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        //
        // This service's `X-Amz-Target` prefix is its fully qualified Coral service
        // name, so the prefix itself contains dots and the framework's first-dot split
        // leaves the whole `<namespace>.<Service>.<Operation>` tail as the routing key,
        // which matches no modelled operation and refuses every call with 501
        // `NotImplemented`. `service_common::rpc_target` states the rule once for every
        // service in this emulator whose prefix carries dots: the operation is the
        // segment after the LAST dot, which dispatches the target an SDK actually sends
        // and is the same answer for a single-identifier prefix. A target naming another
        // service, or an operation this model does not bind, still falls through to the
        // framework's own routing (and its routing error).
        //
        // The operation that name resolves to is the AUDITED table's own (see
        // [`management_events`]) — the same table this service's definition routes every
        // other request through, so no request reaches a path that records nothing. For
        // an operation whose input carries a tag list the audited call sits inside
        // `InModelledEnvelope`, which refuses a tag outside its modelled length in this
        // service's own error shape and hands every other request to the very same
        // generated operation. Nothing is re-routed.
        let operation = service_common::rpc_target::requested_operation(req, self.shape_id())?;
        self.entry.routing_table().get(operation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it. Four classes are reworded, each from the model or a recorded
        // answer, and in this order because a request can break several:
        //
        // * a tag key or value outside its modelled length is
        //   `InvalidTagParameterException`, not a `ValidationException`
        //   ([`invalid_tag_parameter`]) — for a request `InModelledEnvelope` could not
        //   read as an input, which is what reaches this seam now that the wrapper
        //   answers that refusal in the modelled shape;
        // * a value inside `EventSelectors` / `AdvancedEventSelectors` outside its
        //   modelled constraint is `InvalidEventSelectorsException`, the error the
        //   model declares for exactly that case ([`invalid_event_selectors`]);
        // * a `LookupEvents` member for which THIS service models an error of its own AND
        //   which no other request shape carries at its root — `EventCategory` and the
        //   `LookupAttributes` list — is that modelled error
        //   (`InvalidEventCategoryException` / `InvalidLookupAttributesException`). Both
        //   the code and the message are replaced, because a client branches on the code;
        //   the message is built from the request (the value it sent and the rule it
        //   broke);
        // * a violated `@pattern`, a value outside a modelled `@range` bound, and a value
        //   outside an enumerated type's value set are a `ValidationException` whose
        //   clause names the member and the requirement WITHOUT the value that broke it
        //   ([`value_free_envelope`]). The code stays the one the framework would have
        //   answered — a client branches on the code, so only the wording changes.
        //
        // NOT reworded HERE: a page-size member (`MaxResults` / `MaxQueryResults`). A
        // dozen operations carry one and they model DIFFERENT errors for a bad limit
        // (`InvalidMaxResultsException`, `InvalidParameterException`, or none at all),
        // and this seam is not told which operation was called — so the `@range` clause
        // above stays here, which is also the envelope the recorded answer to an
        // out-of-range `GetQueryResults` `MaxQueryResults` carries. The operations whose
        // model declares `InvalidMaxResultsException` for the member they declare as
        // their page size answer it through the per-operation seam instead
        // ([`ModelledRefusal::InvalidMaxResults`], wired in [`InModelledEnvelope`]), which
        // knows which operation it wraps. Every other refusal, authentication included,
        // keeps the framework's wording because every reader declines it.
        let Refusal::Invalid(violations) = refusal else {
            return None;
        };
        if let Some(refused) = invalid_tag_parameter(violations) {
            return Some(refused);
        }
        if let Some(refused) = invalid_event_selectors(violations) {
            return Some(refused);
        }
        let modelled = violations.iter().find_map(|violation| {
            if violation.path == EVENT_CATEGORY_PATH {
                Some((INVALID_EVENT_CATEGORY_CODE, violation))
            } else if is_under(&violation.path, LOOKUP_ATTRIBUTES_PATH) {
                Some((INVALID_LOOKUP_ATTRIBUTES_CODE, violation))
            } else {
                None
            }
        });
        if let Some((code, violation)) = modelled {
            return Some(Refused {
                code: code.to_string(),
                message: refusal_message(violation),
            });
        }
        Some(Refused {
            code: VALIDATION_EXCEPTION.to_string(),
            message: value_free_envelope(violations)?,
        })
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

/// Whether `path` names `member` itself or something INSIDE it.
///
/// Anchored at a path separator, so a sibling member whose name merely starts with the
/// same characters (`lookupAttributesLimit`, say — in this model revision or a later one)
/// is not read as part of `member` and does not inherit its error code.
fn is_under(path: &str, member: &str) -> bool {
    path == member
        || path
            .strip_prefix(member)
            .is_some_and(|rest| rest.starts_with('.'))
}

/// How this service states a refusal it words itself: the value the request sent, the
/// member it sent it at, and the rule that value broke — enough for a caller to fix the
/// request from the message alone.
fn refusal_message(violation: &Violation) -> String {
    let sent = violation.value.clone().unwrap_or_default();
    let requirement = match &violation.kind {
        ViolationKind::MissingRequiredMember => "the member is required".to_string(),
        ViolationKind::EnumValue(allowed) => {
            format!("the supported values are [{}]", allowed.join(", "))
        }
        ViolationKind::MinLength(min) => {
            format!("the value must be at least {min} characters long")
        }
        ViolationKind::MaxLength(max) => format!("the value must be at most {max} characters long"),
        ViolationKind::MinValue(min) => format!("the value must be {min} or more"),
        ViolationKind::MaxValue(max) => format!("the value must be {max} or less"),
        ViolationKind::Pattern(pattern) => {
            format!("the value must match the pattern {pattern}")
        }
        ViolationKind::UniqueItems => "the values must be unique".to_string(),
        // `ViolationKind` is `#[non_exhaustive]`: a constraint the framework adds later
        // is still stated, just without its own sentence.
        _ => "the value is not valid".to_string(),
    };
    format!(
        "Value '{sent}' at '{}' is not valid: {requirement}",
        violation.path
    )
}
