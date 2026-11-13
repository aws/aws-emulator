// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-ecrpublic-spencerfrontendservice` service: how it claims requests and how it words the
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

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The `com-amazonaws-ecrpublic-spencerfrontendservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The shared engine, kept so the non-modelled asset route below reaches this
    /// service's own store ([`crate::state::service_state`]) — that route is answered by
    /// this service rather than by an operation, so it has no `OperationContext` to
    /// reach state through.
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            engine,
        }
    }
}

/// The clause ECR Public states for one refused input member: the member at fault and
/// the constraint it failed, and NEITHER the rejected value nor a count of its bytes.
///
/// ECR words a parameter it will not accept as `Invalid parameter at '<member path>'
/// failed to satisfy constraint: '<requirement>'`, and for a `@pattern` failure the
/// requirement names the regular expression the member declares. Both halves are built
/// from the request and the model — the member's dotted path, and the constraint the
/// validating walk found the value outside of — so they are reproduced from the violation
/// here rather than written out per member or per operation, with one substitution each
/// side: the service states some members under the path IT publishes rather than the wire
/// member name ([`published_path`]), and states the expression IT publishes for a
/// constrained string rather than the model's `@pattern` source ([`published_expression`]).
/// The rejected value is left out because the service does not state it: a `policyText` is
/// modelled up to 10 KiB and an `imageManifest` up to 4 MiB, so echoing one back makes an
/// error body as large as the request that caused it.
///
/// A violation kind this list does not know ([`ViolationKind`] is `#[non_exhaustive]`)
/// keeps the framework's own clause, which states the same two facts in its own words.
/// Same rule, same wording, and the same reasoning as the sibling ECR service
/// (`services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/src/service.rs`);
/// neither service's private helper is reachable from the other.
fn clause(violation: &Violation) -> String {
    // The members whose length rule the service states bare — see [`published_path`].
    let length_rule = |bound: &str, limit: &dyn std::fmt::Display| {
        if BARE_RULE_PATHS.contains(&violation.path.as_str()) {
            format!("Must have length {bound} than or equal to {limit}")
        } else {
            format!("Member must have length {bound} than or equal to {limit}")
        }
    };
    let requirement = match &violation.kind {
        ViolationKind::MissingRequiredMember => "Member must not be null".to_string(),
        ViolationKind::MinLength(min) => length_rule("greater", min),
        ViolationKind::MaxLength(max) => length_rule("less", max),
        ViolationKind::MinValue(min) => {
            format!("Member must have value greater than or equal to {min}")
        }
        ViolationKind::MaxValue(max) => {
            format!("Member must have value less than or equal to {max}")
        }
        // The one requirement NOT worded "Member must …": ECR states a pattern failure by
        // naming the expression a value has to satisfy, which is the form the recorded
        // refusal carries. Deliberate — do not "align" it with the arms around it.
        ViolationKind::Pattern(pattern) => {
            format!(
                "must satisfy regular expression '{}'",
                published_expression(pattern)
            )
        }
        ViolationKind::EnumValue(allowed) => {
            format!("Member must satisfy enum value set: [{}]", allowed.join(", "))
        }
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        _ => return violation.message(),
    };
    format!(
        "Invalid parameter at '{}' failed to satisfy constraint: '{requirement}'",
        published_path(&violation.path)
    )
}

/// The member paths ECR Public publishes under a different spelling from the wire member
/// name, and the spelling it publishes.
///
/// A refusal names the member the way the SERVICE documents that parameter, which for most
/// members is the wire name the request carried; the repository policy document is the one
/// member of this API whose published path is capitalised. Keyed on the member path, so the
/// substitution holds for every operation that takes that member and for every value of it.
const PUBLISHED_PATHS: &[(&str, &str)] = &[("policyText", "PolicyText")];

/// The collections whose refused ENTRY the service names after the parameter it validates
/// rather than after the position the entry arrived at.
///
/// A repository name is validated as a repository name wherever it is carried: a reader
/// that takes a list of them (`DescribeRepositories$repositoryNames`) states a name outside
/// the naming grammar the same way the writer that takes one does, so the entry's clause
/// names `repositoryName` and not `repositoryNames.<n>.member`. Keyed on the collection
/// member, so every entry of it and every constraint on it state the one published path.
const PUBLISHED_ELEMENT_PATHS: &[(&str, &str)] = &[("repositoryNames", "repositoryName")];

/// The step a list element contributes to a violation's dotted path, as the framework
/// spells it (`aws_smithy_schema_server::constraints::path`): the 1-based position, then
/// the collection step.
const COLLECTION_STEP: &str = "member";

/// The member paths whose length rule ECR Public states without the leading `Member`.
///
/// The same member as [`PUBLISHED_PATHS`]: the service states the policy document's size
/// rule as `Must have length …`, where it states every other member's as
/// `Member must have length …`.
const BARE_RULE_PATHS: &[&str] = &["policyText"];

/// `path` under the spelling the service publishes for it ([`PUBLISHED_PATHS`]), after an
/// entry of a collection the service states by its parameter name is resolved to that name
/// ([`PUBLISHED_ELEMENT_PATHS`]).
fn published_path(path: &str) -> &str {
    let path = element_parameter(path).unwrap_or(path);
    PUBLISHED_PATHS
        .iter()
        .find(|(member, _)| *member == path)
        .map_or(path, |(_, published)| *published)
}

/// The parameter name an entry of `path` is stated under, when `path` addresses an entry of
/// a collection in [`PUBLISHED_ELEMENT_PATHS`].
fn element_parameter(path: &str) -> Option<&'static str> {
    let (collection, element) = path.split_once('.')?;
    let (position, step) = element.split_once('.')?;
    if step != COLLECTION_STEP || !position.chars().all(|digit| digit.is_ascii_digit()) {
        return None;
    }
    PUBLISHED_ELEMENT_PATHS
        .iter()
        .find(|(member, _)| *member == collection)
        .map(|(_, published)| *published)
}

/// The expressions ECR Public publishes for its constrained strings, keyed by the model's
/// own `@pattern` source for that string.
///
/// A pattern refusal states the rule a value has to satisfy, and the service states it in
/// the form IT publishes: the registry's own expression over the separators a repository
/// name's segments may carry, which is not the model's non-capturing-group `@pattern`
/// source (`smithy_query(query="shape", shape="RepositoryName")`). Only the MESSAGE is
/// stated from this table — what a value is admitted by stays the model's pattern, which is
/// what the validating walk applies. Keyed on the constraint rather than on a member, so
/// every member targeting that string — a repository name on any operation, and the entries
/// of a repository-name list — states the one published rule.
const PUBLISHED_EXPRESSIONS: &[(&str, &str)] = &[(
    "^(?:[a-z0-9]+(?:[._-][a-z0-9]+)*/)*[a-z0-9]+(?:[._-][a-z0-9]+)*$",
    r"[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*(/[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*)*",
)];

/// The expression a `@pattern` failure states: the one the service publishes for that
/// constraint when it publishes one, else the model's own expression without the anchors
/// (see [`unanchored`]).
fn published_expression(pattern: &str) -> &str {
    PUBLISHED_EXPRESSIONS
        .iter()
        .find(|(source, _)| *source == pattern)
        .map_or_else(|| unanchored(pattern), |(_, published)| *published)
}

/// A `@pattern` expression without the anchors a model may spell it with: AWS states the
/// expression a value has to satisfy, not the anchoring the model uses to make the match
/// total. A `$` escaped by a backslash is a literal dollar sign inside the expression,
/// not the end anchor, so it stays. (Same rule, and the same reasoning, as the sibling ECR
/// service's own `unanchored`; neither service's private helper is reachable from the
/// other.)
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
            None => {
                // The repository logo a catalog write published
                // (`RepositoryCatalogData$logoUrl`) is fetched by whatever browser,
                // gallery preview or publish pipeline holds that URL: anonymously, with
                // no credential scope and no `X-Amz-Target` to name this service by —
                // only the path this service itself minted, which carries the registry
                // the asset is read in. A GET (or a HEAD, which asks what that asset is)
                // of exactly that path is therefore claimed here, so the logo is
                // reachable at the URL this service reported; every other path still
                // needs the routing key above.
                (parts.method == http::Method::GET || parts.method == http::Method::HEAD)
                    && crate::state::logo_asset_route(&parts.uri.to_string()).is_some()
            }
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        let _ = req;
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // ECR Public states an input it will not accept as `InvalidParameterException`
        // ("The specified parameter is invalid. Review the available parameters for the
        // API request.") — the one client-input error every operation in this model
        // declares; the model carries no `ValidationException` at all. So a request the
        // framework refuses before a handler runs (an absent `repositoryName`, a name
        // outside 2..205 or `^(?:[a-z0-9]+(?:[._-][a-z0-9]+)*/)*[a-z0-9]+(?:[._-][a-z0-9]+)*$`,
        // a `catalogData.description` over 1024, a `policyText` over 10240, a
        // `maxResults` outside 1..1000, an `imageIds` list longer than 100, a
        // `displayName` longer than 100) is reworded to that code, and its message to the
        // `Invalid parameter at '<member>' failed to satisfy constraint: '<rule>'`
        // template ECR Public fills from the request (see [`clause`]) — NOT the
        // framework's `N validation error(s) detected: Value '<the value>' …` envelope,
        // which states a count ECR Public does not state and quotes the rejected value
        // back. The status stays with the protocol: awsJson1_1 renders a refusal as HTTP
        // 400, which is `InvalidParameterException`'s modelled status.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: "InvalidParameterException".to_string(),
                // Every member at fault, so a caller fixes the request from the message
                // alone; one refused member is the single-clause case of the same rule.
                // A refusal that named no violation has no member to name, so it answers
                // the modelled code with the error shape's own documented sentence.
                message: if violations.is_empty() {
                    "The specified parameter is invalid. Review the available parameters \
                     for the API request."
                        .to_string()
                } else {
                    violations
                        .iter()
                        .map(clause)
                        .collect::<Vec<String>>()
                        .join("; ")
                },
            }),
            // Authentication is not this service's concern; the framework's 403 wording
            // is already what a caller needs.
            _ => None,
        }
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. This endpoint serves ONE route the Smithy model does
        // not describe: the repository logo asset a catalog write published, which
        // `RepositoryCatalogData$logoUrl` locates for an anonymous client to fetch
        // (`logoImageBlob` is input-only, so fetching that URL is the only way the
        // uploaded bytes are observable). AWS serves it unauthenticated from the gallery's
        // own host, which is what this seam gives: the router consults it before it
        // authenticates anything, so an anonymous GET reaches the asset the same way it
        // does on AWS.
        //
        // Every other unrouted request keeps the protocol-correct default for this
        // service's envelope (awsJson -> 404) — including a path of this shape that names
        // no stored asset, so nothing is answered for a logo this registry does not hold.
        //
        // `HEAD` is answered exactly as `GET` is, with no body: a client that asks only
        // what the asset IS — its media type and its size — is asking about the same
        // asset, and an HTTP asset host answers both methods.
        let body_wanted = match req.method() {
            "GET" => true,
            "HEAD" => false,
            _ => return None,
        };
        let (registry_id, repository_name, asset_id) =
            crate::state::logo_asset_route(req.uri())?;
        let state = crate::state::service_state(&*self.engine)?;
        // Rendering the STORED bytes is a pure read: the asset is resolved in the registry
        // the URL this service minted names, against the repository that published it, and
        // nothing about the registry changes.
        let logo = {
            let store = state.lock();
            store
                .logo_asset(registry_id, repository_name, asset_id)
                .cloned()
        };
        // An asset-hosting answer, not a modelled API one: the bytes when this registry
        // holds them, and 404 when it does not — a logo it never stored, one a later
        // catalog write replaced, or a repository that does not exist. Nothing is
        // invented for an asset that is not there.
        let status = if logo.is_some() { 200u16 } else { 404 };
        let body = match (&logo, body_wanted) {
            (Some(logo), true) => logo.bytes.clone(),
            _ => Vec::new(),
        };
        let mut response = aws_smithy_runtime_api::http::Response::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(status).ok()?,
            ::aws_smithy_types::body::SdkBody::from(body),
        );
        let headers = response.headers_mut();
        if let Some(logo) = logo.as_ref() {
            // What the asset IS: the media type its own bytes announce, and the size of
            // the asset itself — which a `HEAD` reports with no body, as HTTP defines it.
            headers.insert("content-type", logo.media_type());
            headers.insert("content-length", logo.bytes.len().to_string());
            // The bytes are a caller's upload served back from this emulator's own
            // origin, so the type this response announces is the only type a browser may
            // treat them as: nothing is re-sniffed into a type the service did not state.
            headers.insert("x-content-type-options", "nosniff");
        }
        Some(response)
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
