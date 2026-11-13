// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-applicationsignals-applicationsignals` service: how it claims requests and how it words the
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
//! - `override_operation` — which operation serves a request. EDITED HERE: the key
//!   and the handler stay the generated table's, but an operation that binds a
//!   timestamp to the query string is served through [`QueryTimestampForms`],
//!   which reads such a value in every form the wire names an instant in — the
//!   epoch-seconds number the member documents among them.
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
//! # Timestamps in the query string
//!
//! Every time-ranged read of this service binds its `StartTime`/`EndTime` to the query
//! string (`@httpQuery`, `@required`) and documents the value's wire form on the member
//! itself: "When used in a raw HTTP Query API, it is formatted as epoch time in seconds.
//! For example, 1698778057" (`smithy_query(query="doc", shape="GetServiceInput",
//! member="StartTime")`). The model carries no `@timestampFormat`, so the protocol's
//! default for an `@httpQuery` timestamp — RFC-3339 `date-time` — is the only form the
//! framework's query-value deserializer reads, and a client sending the documented
//! epoch-seconds number is refused before the operation's handler runs.
//!
//! [`QueryTimestampForms`] closes that gap for every operation of this service that binds
//! a timestamp to the query string: a query value in any form the wire names an instant in
//! — the epoch-seconds number, and RFC 3339 with the numeric zone offset as well as with
//! `Z` — binds that instant, a value the framework's own reader already takes is left
//! exactly as the client sent it, and a value that names no instant is refused the way the
//! protocol refuses one. It is the SHARED wrapper in `service_common::query_timestamps` —
//! the gap is the generated schema's, not this service's, so the reading is written once
//! for every service whose model binds a timestamp to the query string.

use service_common::query_timestamps::QueryTimestampForms;
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service, Violation, ViolationKind,
};
// What the query-timestamp routing table below needs, on its own `use` so this set and
// the service's own above are edited on separate lines.
use smithy_server_runtime::OperationRoutingTable;
use std::sync::Arc;

/// The error code this service answers an input-constraint violation with: the modelled
/// `ValidationException` (`smithy_query(shape="ValidationException")`: `@error client`,
/// `@httpError 400`), which is also the code the framework would have chosen — restated
/// here because `override_refusal` replaces the code and the message together.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The routes this service's generated table cannot name an operation for when a `@required`
/// `@httpQuery` member is absent from the request, as `(method, path, operation)`.
///
/// The framework's REST router rejects a candidate route whose `@required` query bindings
/// are not on the request. That rule is what lets an S3-class model tell four operations
/// apart on one `PUT /{Bucket}/{Key+}` by the SHAPE of the request — but this service
/// overloads no route, so for it the rule only misfires: a call that omits one of these
/// members matches NO route, and the request is answered "unknown operation" (HTTP 501
/// `NotImplemented`) instead of the modelled HTTP 400 `ValidationException` that names the
/// absent member. An absent required input is a bad request, never an operation nobody
/// wrote.
///
/// Every entry below is a route with a STATIC path that declares at least one `@required`
/// `@httpQuery` member (`smithy_query(query="operation",operation="GetService,ListServices,
/// ListServiceOperations,ListServiceDependencies,ListServiceDependents,ListAuditFindings,
/// ListTagsForResource")`: `StartTime`/`EndTime` on the six time-ranged reads, `ResourceArn`
/// on `ListTagsForResource`), so this is exactly the set of routes the rule can misfire on,
/// and on each of them the method and path name the operation on their own.
const REQUIRED_QUERY_ROUTES: &[(&str, &str, &str)] = &[
    ("POST", "/service", "GetService"),
    ("GET", "/services", "ListServices"),
    ("POST", "/service-operations", "ListServiceOperations"),
    ("POST", "/service-dependencies", "ListServiceDependencies"),
    ("POST", "/service-dependents", "ListServiceDependents"),
    ("POST", "/auditFindings", "ListAuditFindings"),
    ("GET", "/tags", "ListTagsForResource"),
];

/// The path of a request URI: no scheme, no authority, no query string, and no trailing
/// slash, which is how [`REQUIRED_QUERY_ROUTES`] spells a route. The router's own matcher
/// ignores a trailing slash, so this one must too, or the two would disagree about which
/// requests the table already resolves.
fn request_path(uri: &str) -> &str {
    let after_authority = match uri.find("://") {
        None => uri,
        Some(scheme_end) => {
            let authority = &uri[scheme_end + 3..];
            match authority.find('/') {
                Some(path_start) => &authority[path_start..],
                None => "/",
            }
        }
    };
    let path = after_authority
        .split(['?', '#'])
        .next()
        .unwrap_or(after_authority);
    match path.trim_end_matches('/') {
        "" => "/",
        trimmed => trimmed,
    }
}

/// Render one constraint violation the way this service states it: the member path and the
/// requirement it violated, and NOTHING of the value that was rejected.
///
/// `None` for a violation class this service has no wording for, which lets
/// [`constraint_envelope`] decline the whole refusal and keep the framework's wording
/// rather than invent a sentence ([`ViolationKind`] is `#[non_exhaustive]`).
fn constraint_clause(violation: &Violation) -> Option<String> {
    let requirement = match &violation.kind {
        // An absent member is stated as the null it was, by the one builder that words it.
        ViolationKind::MissingRequiredMember => {
            return Some(crate::state::absent_member_clause(&violation.path))
        }
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
        // model spells it with: the constraint stated is the expression a value must
        // satisfy, not the anchoring the model uses to make the match total.
        ViolationKind::Pattern(pattern) => format!(
            "Member must satisfy regular expression pattern: {}",
            crate::state::unanchored(pattern)
        ),
        ViolationKind::EnumValue(allowed) => {
            format!("Member must satisfy enum value set: [{}]", allowed.join(", "))
        }
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        _ => return None,
    };
    Some(crate::state::constraint_clause(&violation.path, &requirement))
}

/// The `ValidationException` message for a refusal: the AWS envelope
/// (`N validation error(s) detected: `) over one clause per violation, joined by `; `.
///
/// `None` declines the whole refusal — see [`constraint_clause`].
fn constraint_envelope(violations: &[Violation]) -> Option<String> {
    let clauses: Vec<String> = violations
        .iter()
        .map(constraint_clause)
        .collect::<Option<Vec<_>>>()?;
    if clauses.is_empty() {
        return None;
    }
    Some(crate::state::validation_envelope(&clauses))
}

/// The `com-amazonaws-applicationsignals-applicationsignals` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The operations that bind a timestamp to the query string, each the generated
    /// table's own operation wrapped in [`QueryTimestampForms`] and built from the SAME
    /// shared engine, so a wrapped operation reads and writes the one store. Keyed by the
    /// routing key the protocol resolves, which is the key the generated table uses.
    query_timestamp_forms: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`QueryTimestampForms`] and keyed by its operation shape name — the routing key
/// `ServerProtocol::routing_key` resolves and the generated table is keyed by.
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                stringify!($operation),
                ::std::boxed::Box::new(QueryTimestampForms::new(
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
        // Exactly the operations whose input binds a timestamp to the query string —
        // `StartTime` and `EndTime` on each (`smithy_query(query="operation", …)`:
        // `{"target": "Timestamp", "httpQuery": "StartTime", "required": true}`). Every
        // other operation of this service takes its timestamps in the body, where the
        // protocol already reads the epoch-seconds form, so wrapping them would change
        // nothing.
        let mut query_timestamp_forms = OperationRoutingTable::new();
        wrap_operations!(
            query_timestamp_forms,
            engine,
            get_service::GetService,
            list_audit_findings::ListAuditFindings,
            list_service_dependencies::ListServiceDependencies,
            list_service_dependents::ListServiceDependents,
            list_service_operations::ListServiceOperations,
            list_services::ListServices,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            query_timestamp_forms,
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
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        }
    }

    /// Serve the request through the operation the generated table's own protocol names,
    /// and serve a request whose method and path name one modelled route even when a
    /// `@required` `@httpQuery` member it declares is ABSENT.
    ///
    /// The first branch is NOT a re-route: the key is the one the generated table's own
    /// protocol resolves, and the operation it names is the one served — wrapped in
    /// `QueryTimestampForms` for the operations that bind a timestamp to the query string,
    /// so every form the wire names an instant in binds, the epoch-seconds form their
    /// members document among them. Every other key resolves to `None` there exactly as
    /// it does for an unedited service, leaving the generated table to serve it.
    ///
    /// The second branch is reached only when the protocol can name NO operation at all,
    /// which for this service happens when a required query member is missing (see
    /// [`REQUIRED_QUERY_ROUTES`]). The operation then runs the centralized walk, whose
    /// validation step refuses the absent member with the modelled HTTP 400
    /// `ValidationException` naming it — the answer the real service gives, and the one a
    /// 501 `NotImplemented` was standing in for. It is served through the same wrapper, so
    /// a request that carries one timestamp in the epoch-seconds form and omits the other
    /// is refused for the member it omitted rather than for the form it used.
    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        if let Ok(key) = self.entry.protocol().routing_key(req) {
            return self.query_timestamp_forms.get(key);
        }
        let path = request_path(req.uri());
        let operation = REQUIRED_QUERY_ROUTES
            .iter()
            .find(|(method, route, _)| *method == req.method() && *route == path)
            .map(|(_, _, operation)| *operation)?;
        self.query_timestamp_forms
            .get(operation)
            .or_else(|| self.entry.routing_table().get(operation))
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // Application Signals states a constraint violation WITHOUT the value that
        // violated it, and states a `@pattern` requirement as the expression the model
        // declares minus its anchors. The code stays the modelled `ValidationException`
        // (HTTP 400), which is what the framework would have answered — only the wording
        // changes.
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
