// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-codepipeline-codepipeline-20150709` service: how it claims requests and how it words the
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
//!   correctly. The default returns `None`, so every request resolves through the
//!   generated routing table. Return `Some(op)` to route one yourself.
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
//! One thing about that entry IS this file's: the routing table it answers with is the
//! AUDITED one ([`management_events`]), so every call this service serves is recorded in
//! the account's event history where CloudTrail's `LookupEvents` reports it. Recording is
//! a property of serving a call rather than of any one operation, which is why it is wired
//! here once instead of in 44 handlers.
//!
//! ## The webhook trigger plane
//!
//! `ListWebhookItem$url` is an address a THIRD PARTY dials: "when a POST request is made to
//! this URL, the defined pipeline is started as long as the body of the post request
//! satisfies the defined authentication and filtering". That POST names no modelled
//! operation and carries no routing key, so three things above are what serve it:
//!
//! - [`Service::can_handle`] also claims a request addressed to that plane
//!   ([`webhook_trigger::claims_parts`]), which is what routes it here at all;
//! - the definition this service answers with serves a request addressed to that plane
//!   whatever wire protocol it names ([`request_protocol`]), because such a request names
//!   none of the two this model declares and the framework refuses one that names none
//!   before any operation is asked for it;
//! - [`Service::override_operation`] serves it, through
//!   [`webhook_trigger::WebhookTriggerPlane`].
//!
//! What is served there, and why that plane is unsigned, is documented in
//! [`webhook_trigger`].

/// The serializer that renders this service's responses in the wire form the protocol
/// gives them: the awsJson shape discriminator on every structure that carries one.
pub mod action_type_wire_form;
/// WHICH structures carry that discriminator, and the shape name each one carries.
pub mod shape_discriminator;
pub mod management_events;
/// WHICH of the two protocols this service's model declares serves one request — and what
/// serves the one request of this service that names neither of them.
pub mod request_protocol;
pub mod webhook_trigger;

use service_common::audited_operation::AuditedDefinition;
use smithy_server_runtime::{
    http, ErasedOperation, ErasedServiceDefinition, Refusal, Refused, Service, Violation,
    ViolationKind,
};
use std::sync::Arc;

/// The error code CodePipeline answers an input-constraint violation with: the modelled
/// `ValidationException`, which every operation of this service declares with
/// `@httpError 400` (the framework's own default code for a refusal, restated here
/// because `override_refusal` replaces code and message together).
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// One map member of this service's input surface whose ENTRIES carry constraints.
///
/// A constraint a map's key or value SHAPE declares is a property of the map member,
/// not of the one entry that happened to violate it, so the refusal names the map
/// member and states that shape's whole constraint set. The key bounds are the model's:
/// `smithy_query query="shape" shape="ActionConfigurationMap"` reports
/// `key ActionConfigurationKey length 1..50`; the value bounds are the ones the service
/// STATES ([`CONFIGURATION_VALUE_STATED_LENGTH`]).
///
/// Hand-maintained, and only as far as it can be: `Violation` (the framework's
/// constraint-walk output, `vendor/smithy-schema-server/src/constraints.rs`) carries a
/// path, a kind and the rejected value — NOT which shape of the map the constraint came
/// from, nor that shape's other constraints — so a service cannot derive either from
/// the violation it is handed. That is the gap filed as
/// `issues/things-that-do-not-work/20261022T133000Z-gap-smithy-schema-server-constraint-violation-stores-only-the-elided-rejected-value.md`'s
/// neighbourhood in the frozen crate; until a violation says so, the table is the only
/// place that knows. Consequences, stated rather than hidden:
///
/// * a map NOT listed here keeps the framework's wording, reported at the entry's own
///   path. The other maps this service's inputs carry are `artifactStores`
///   (`key AWSRegionName length 4..30`, value a structure) and the job-result
///   `outputVariables` (`key OutputVariablesKey`, pattern only); neither appears in a
///   recorded divergence, so neither is listed until one does.
/// * key-vs-value is inferred from the bound the violation names, so a map whose key
///   and value share an upper bound would be reported as a key violation.
struct ConstrainedMap {
    /// The member name the map is bound to, at any depth in the input.
    member: &'static str,
    /// The key shape's `@length` bounds.
    key_length: (i64, i64),
    /// The value shape's `@length` bounds.
    value_length: (i64, i64),
}

/// The `@length` bounds CodePipeline STATES for an action configuration value in a
/// validation message.
///
/// The upper bound is the service's, not the model's: `ActionConfigurationValue` carries
/// `@length(min: 1, max: 1000)` (`smithy_query query="shape"
/// shape="ActionConfigurationValue"`), while the service states this figure as the
/// configuration-value maximum in the message it refuses an out-of-bounds value with. The
/// message is the contract a client reads the limit from, so it states the service's
/// figure.
///
/// The two therefore disagree about the values BETWEEN them, and deliberately: what the
/// request is CHECKED against is the model's own constraint, applied by the framework's
/// boundary walk before any handler of this service runs, so a value longer than the
/// model's maximum is refused here while the service that published this figure would
/// take it. Closing that gap means the model's constraint, which this crate does not
/// author; stating the service's figure in the message is what this constant is for.
pub const CONFIGURATION_VALUE_STATED_LENGTH: (i64, i64) = (1, 50_000);

/// Every constrained-entry map this service's inputs carry.
const CONSTRAINED_MAPS: &[ConstrainedMap] = &[ConstrainedMap {
    member: "configuration",
    key_length: (1, 50),
    value_length: CONFIGURATION_VALUE_STATED_LENGTH,
}];

/// The map member a violation path names an ENTRY of, if any.
///
/// The framework walks into the entry, so the path it reports ends in the caller's own
/// map KEY (`….actions.1.member.configuration.CustomData`). A map member followed by
/// exactly one further segment is therefore an entry of that map, and the path AWS
/// states is the map member's own.
///
/// The path is REQUEST-derived text (that last segment is the caller's key, which may be
/// empty or non-ASCII), so it is taken apart with `split`/`rsplit_once` only: no byte
/// index is computed from it, and a path this rule does not match yields `None`, which
/// keeps the framework's own wording.
fn map_entry(path: &str) -> Option<(&'static ConstrainedMap, String)> {
    let (head, key) = path.rsplit_once('.')?;
    if key.contains('.') {
        return None;
    }
    let map = CONSTRAINED_MAPS
        .iter()
        .find(|map| head == map.member || head.ends_with(&format!(".{}", map.member)))?;
    Some((map, head.to_string()))
}

/// The requirement clause for a constraint violated by one entry of `map`.
///
/// Both bounds of the entry shape are listed, because the requirement reported is that
/// shape's constraint set rather than the single bound this value fell outside of. A
/// violation naming the KEY shape's upper bound is a key violation; everything else is
/// reported against the value, whose lower bound the key's happens to share.
fn map_entry_requirement(map: &ConstrainedMap, kind: &ViolationKind) -> Option<String> {
    let names_key = matches!(kind, ViolationKind::MaxLength(max) if *max == map.key_length.1);
    let (subject, (min, max)) = if names_key {
        ("Map key", map.key_length)
    } else {
        ("Map value", map.value_length)
    };
    matches!(
        kind,
        ViolationKind::MinLength(_) | ViolationKind::MaxLength(_)
    )
    .then(|| {
        format!(
            "{subject} must satisfy constraint: [Member must have length less than or \
             equal to {max}, Member must have length greater than or equal to {min}]"
        )
    })
}

/// Render one constraint violation the way CodePipeline states it: the member path and
/// the violated constraint, and NOTHING of the value that was rejected.
///
/// Returns `None` for a violation class this service has no wording for, which lets
/// [`constraint_envelope`] decline the whole refusal and keep the framework's wording
/// rather than invent a sentence for it ([`ViolationKind`] is `#[non_exhaustive]`, so a
/// class can be added upstream).
pub(crate) fn constraint_clause(violation: &Violation) -> Option<String> {
    // A constraint the entry shape of a map declares is reported against the map
    // member, with that shape's whole constraint set.
    if let Some((map, path)) = map_entry(&violation.path) {
        if let Some(requirement) = map_entry_requirement(map, &violation.kind) {
            return Some(format!(
                "Value at '{path}' failed to satisfy constraint: {requirement}"
            ));
        }
    }
    let requirement = match &violation.kind {
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
        // The requirement reported is the expression itself, without the `^` / `$` the
        // model spells it with: AWS states the character class a value must satisfy,
        // not the anchoring the model uses to make the match total. The rendering is
        // the shared one (`service_common::validation::unanchored`), which is the
        // staged home for this rule rather than a copy per service.
        ViolationKind::Pattern(pattern) => format!(
            "Member must satisfy regular expression pattern: {}",
            ::service_common::validation::unanchored(pattern)
        ),
        ViolationKind::EnumValue(allowed) => {
            format!("Member must satisfy enum value set: [{}]", allowed.join(", "))
        }
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        _ => return None,
    };
    // An absent member has no submitted value at all, so it is reported as the null it
    // was: that is a fact of the request's shape, not a rendering of a value the caller
    // sent. Every other violation names the member only — the rejected value is the
    // caller's own content and AWS does not echo it back.
    let subject = match &violation.kind {
        ViolationKind::MissingRequiredMember => "Value null",
        _ => "Value",
    };
    Some(format!(
        "{subject} at '{}' failed to satisfy constraint: {requirement}",
        violation.path
    ))
}

/// One member of this service's input surface whose value is held to a `@length` floor
/// the MODEL does not declare.
///
/// `smithy_query query="shape" shape="ResourceArn,ApprovalToken"` reports a `@pattern`
/// and no `@length` for either, while CodePipeline states both a pattern clause and a
/// length clause for a malformed value. Each floor ([`crate::state::RESOURCE_ARN_MIN_LENGTH`],
/// [`crate::state::APPROVAL_TOKEN_MIN_LENGTH`]) is the shortest string its member's
/// pattern can match, so no value the pattern ADMITS falls below it: the floor only ever
/// adds a clause to a value the pattern already refused, which is why stating it here —
/// where the violations of one request are worded — is the whole rule rather than half of
/// it.
struct LengthFloor {
    /// The member name the value is bound to, at any depth in the input.
    member: &'static str,
    /// The inclusive minimum character count the service requires of it.
    min: i64,
}

/// Every member of this service's inputs whose length floor the model leaves unstated.
const LENGTH_FLOORS: &[LengthFloor] = &[
    LengthFloor {
        member: "resourceArn",
        min: crate::state::RESOURCE_ARN_MIN_LENGTH,
    },
    LengthFloor {
        member: "token",
        min: crate::state::APPROVAL_TOKEN_MIN_LENGTH,
    },
];

/// The undeclared length floor the member a violation path names is held to, if any.
///
/// The path is REQUEST-derived text, so it is only ever compared and split: a path this
/// table does not name yields `None`, which states exactly the model's own constraints.
fn length_floor(path: &str) -> Option<&'static LengthFloor> {
    LENGTH_FLOORS
        .iter()
        .find(|floor| path == floor.member || path.ends_with(&format!(".{}", floor.member)))
}

/// The violations one refusal STATES, from the ones the framework's walk collected.
///
/// Two rules, each applied over every member rather than to one operation's:
///
/// * a value below a length floor the model leaves unstated ([`LENGTH_FLOORS`]) is
///   reported for that too, after the clause for the constraint the model does declare,
///   so the clauses stay in the order the model states them in;
/// * a `@pattern` is not evaluated against an EMPTY value that already violated a
///   `@length` minimum the MODEL declares: the floor is the violation such a value has,
///   and the expression it would have had to match is not a second one. Keyed on the
///   model's own constraint, so a member whose floor only this file states keeps both
///   clauses — nothing says the service drops one there.
fn stated_violations(violations: &[Violation]) -> Vec<Violation> {
    let mut stated: Vec<Violation> = Vec::with_capacity(violations.len());
    let mut floored: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for violation in violations {
        stated.push(violation.clone());
        let Some(floor) = length_floor(&violation.path) else {
            continue;
        };
        let Some(value) = violation.value.as_deref() else {
            // An absent member has no value to measure: `Member must not be null` is
            // the whole of what the request got wrong there.
            continue;
        };
        // Stated once per member, after that member's first modelled clause: a member
        // with two modelled violations has one floor, not one per clause.
        let modelled = violations
            .iter()
            .filter(|other| other.path == violation.path)
            .any(|other| matches!(other.kind, ViolationKind::MinLength(_)));
        // The same count `@length` is checked with on a string: characters.
        if !modelled
            && (value.chars().count() as i64) < floor.min
            && floored.insert(violation.path.as_str())
        {
            stated.push(Violation {
                path: violation.path.clone(),
                kind: ViolationKind::MinLength(floor.min),
                value: violation.value.clone(),
                sensitive: violation.sensitive,
            });
        }
    }
    let floored_empty: std::collections::BTreeSet<&str> = violations
        .iter()
        .filter(|violation| {
            matches!(violation.kind, ViolationKind::MinLength(_))
                && violation.value.as_deref() == Some("")
        })
        .map(|violation| violation.path.as_str())
        .collect();
    stated
        .iter()
        .filter(|violation| {
            !matches!(violation.kind, ViolationKind::Pattern(_))
                || !floored_empty.contains(violation.path.as_str())
        })
        .cloned()
        .collect()
}

/// The `ValidationException` message for a refusal: the AWS envelope
/// (`N validation error(s) detected: `) over one clause per violation.
///
/// `None` declines the whole refusal — see [`constraint_clause`].
pub(crate) fn constraint_envelope(violations: &[Violation]) -> Option<String> {
    let clauses: Vec<String> = stated_violations(violations)
        .iter()
        .map(constraint_clause)
        .collect::<Option<Vec<_>>>()?;
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// The `com-amazonaws-codepipeline-codepipeline-20150709` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    /// The generated entry, routing through this service's AUDITED operations: every call
    /// it serves is a management event of this service, recorded in the account's event
    /// history where CloudTrail's `LookupEvents` reads it (see [`management_events`]).
    /// Identity, the protocols and every framework error renderer stay the generated
    /// entry's; the one further thing this service's own wrapper answers is which protocol
    /// serves a request addressed to the published trigger plane ([`request_protocol`]).
    entry: request_protocol::ServedWhateverProtocolItNames<
        AuditedDefinition<crate::ServiceDefinition<Eng>>,
    >,
    /// Serves the trigger plane a webhook publishes — the wire its `url` advertises. Held
    /// on the service because [`Service::override_operation`] hands the router a BORROW of
    /// one of its own operations, and it owns the shared engine so every answer is computed
    /// from the same store the control-plane handlers write.
    ///
    /// It is NOT behind the audit recorder: a third party POSTing a provider payload to a
    /// published URL makes no AWS API call, so there is no management event of this service
    /// to record for it.
    webhook_trigger: webhook_trigger::WebhookTriggerPlane<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        // Every modelled operation of this service, constructed from the same shared
        // engine and served behind the audit recorder, so no request reaches a handler by
        // a path that records nothing (see [`management_events::audited_table`]).
        let audited = management_events::audited_table(&engine);
        Self {
            webhook_trigger: webhook_trigger::WebhookTriggerPlane::new(Arc::clone(&engine)),
            // Outermost, so the audited table is what it routes from: the request addressed
            // to the published trigger plane names none of the two protocols this model
            // declares, and without this wrapper the framework's per-request selection
            // refuses it before `override_operation` is asked for it at all (see
            // [`request_protocol`]).
            entry: request_protocol::ServedWhateverProtocolItNames::new(AuditedDefinition::new(
                crate::ServiceDefinition::new(engine),
                audited,
            )),
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
        // And a request addressed to the TRIGGER plane a webhook publishes: the URL
        // `PutWebhook`/`ListWebhooks` hand a third party names no modelled operation and
        // carries no routing key at all, so without this the router answers `404
        // UnknownService` for a URL this service handed out. See
        // [`webhook_trigger`].
        webhook_trigger::claims_parts(parts)
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
        // A POST to a published webhook trigger URL is the one such request: the third
        // party's own call names no modelled operation, so it is named here instead and
        // served by the plane that holds this service's state. Every method addressed to
        // the plane reaches it, so it answers the method itself rather than falling through
        // to the router's answer for an unrouted request.
        if self.webhook_trigger.claims(req) {
            return Some(&self.webhook_trigger);
        }
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it.
        //
        // CodePipeline states a constraint violation WITHOUT the value that violated it
        // — the message names the member path and the constraint only — a `@pattern` as
        // the expression a value must satisfy rather than as the model's anchored form,
        // and a map entry's constraint against the map member. The code stays the
        // modelled `ValidationException` (HTTP 400), which is what the framework would
        // have answered: only the wording changes.
        //
        // An authentication refusal keeps the framework's wording: it is not an
        // input-constraint failure.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: VALIDATION_EXCEPTION.to_string(),
                message: constraint_envelope(violations)?,
            }),
            _ => None,
        }
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
