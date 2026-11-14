// SPDX-License-Identifier: CC0-1.0

//! The judgement every WAFv2 operation makes about the REQUEST ITSELF, before it
//! resolves anything the request names or touches the store: the `Scope` the request
//! names, against the endpoint it was signed for.
//!
//! Two readings, because a scope answers two different questions. A call that RESOLVES
//! something in a namespace is served where that namespace lives ([`validate_scope`]);
//! a call that MINTS a web ACL into one names only the namespace its new resource lands
//! in, and is served wherever it arrives ([`validate_minted_scope`]).
//!
//! The ONLY thing it reads out of the request is the SigV4 credential scope's
//! region, through the framework's own reader ([`request_region`]); it reads no
//! environment variable, no host and no file, and it compares strings — nothing here
//! indexes, slices or unwraps a value a caller controls, so no request can reach a
//! panicking construct through it.
//!
//! It lives here, beside the file that decides which requests this service claims
//! and how it words what the framework refuses, rather than in [`crate::state`],
//! which is the store: it neither reads nor writes it. Every handler calls it by
//! this path, so what a handler reaches through `crate::state` is state and nothing
//! else.

use crate::ops::types::WAFInvalidParameterException;
use crate::state::{invalid_scope_exception, is_known_scope, DEFAULT_REGION, SCOPE_CLOUDFRONT};

/// The AWS region the request was signed for, or [`DEFAULT_REGION`] when it
/// carries no SigV4 credential scope to read one from.
///
/// The credential scope is parsed by the framework's own reader, which answers
/// `None` for a request carrying no usable `Authorization` header, so this reads
/// nothing else out of the request and cannot fail on a malformed one.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ONE region endpoint that serves the CLOUDFRONT scope.
///
/// "To work with CloudFront, you must also specify the Region US East (N.
/// Virginia) as follows: CLI - Specify the Region when you use the CloudFront
/// scope: --scope=CLOUDFRONT --region=us-east-1. API and SDKs - For all calls, use
/// the Region endpoint us-east-1" (`Scope`'s own doc): the global namespace lives
/// in that one region, so no other endpoint can be asked about it.
pub const CLOUDFRONT_SCOPE_REGION: &str = "us-east-1";

/// `Ok(())` when a request signed for this endpoint may name `scope`, else the
/// refusal WAF answers.
///
/// Two conditions, one refusal: a value the `Scope` enum does not declare is no
/// scope at all, and the CLOUDFRONT scope is served ONLY at
/// [`CLOUDFRONT_SCOPE_REGION`] (see that constant), so a request signed for any
/// other region names a scope this endpoint cannot act in. Both are a bad value of
/// the `Scope` member, judged from the request alone — so every operation that
/// takes a `Scope` makes this judgement FIRST, before it resolves anything the
/// request names: a caller at the wrong endpoint is told its scope is wrong rather
/// than that its resource does not exist (in the global namespace the regional
/// endpoint cannot see, nothing does).
pub fn validate_scope(
    scope: &str,
    parts: &::http::request::Parts,
) -> Result<(), WAFInvalidParameterException> {
    // A value the enum does not declare never reaches a handler — the framework
    // refuses it at the request boundary, in the request validator's own envelope —
    // so this branch states that invariant for a handler reached any other way
    // rather than guarding a case a request can still bring in.
    if !is_known_scope(scope) {
        return Err(invalid_scope_exception(scope));
    }
    scope_served_here(scope, parts)
}

/// The ENDPOINT half of [`validate_scope`]: `Ok(())` unless `scope` names a
/// namespace the endpoint this request reached cannot act in.
///
/// Separated from the value half because the two are judged at different points of a
/// request's life. Whether a value IS a scope is a modelled constraint on the
/// `Scope` member, which the request boundary refuses before any handler and states
/// in the request validator's own envelope; whether the namespace it names is served
/// HERE is WAF's own precondition, which no model expresses and which WAF takes
/// before it reports any other member's constraint
/// ([`crate::service::scope_gate`]). A caller that sent a scope the enum does not
/// declare has a malformed request; one that sent a declared scope to the wrong
/// endpoint has a well-formed request this endpoint cannot serve.
pub fn scope_served_here(
    scope: &str,
    parts: &::http::request::Parts,
) -> Result<(), WAFInvalidParameterException> {
    if scope == SCOPE_CLOUDFRONT && request_region(parts) != CLOUDFRONT_SCOPE_REGION {
        return Err(invalid_scope_exception(scope));
    }
    Ok(())
}

/// `Ok(())` when a request MINTING a web ACL may name `scope`, else the refusal WAF
/// answers.
///
/// One condition, not two. What a mint does NOT depend on is the endpoint it arrived
/// at — a `Scope` names the NAMESPACE the new web ACL lands in rather than an endpoint
/// the caller must reach, and WAF accepts a global-namespace web ACL created from a
/// regional endpoint (recorded behaviour, against the "API and SDKs - For all calls,
/// use the Region endpoint us-east-1" the member's own doc states for a client),
/// storing the global resource either way: `resource_arn` carries the region the global
/// namespace lives in whatever the create was signed for, so the web ACL a regional
/// endpoint mints for the CLOUDFRONT scope is the same global resource as one minted at
/// [`CLOUDFRONT_SCOPE_REGION`], under the same ARN form.
///
/// What remains is the VALUE: a value the `Scope` enum does not declare is no scope at
/// all, the same bad parameter [`validate_scope`] refuses it as. `Scope` is a modelled
/// enum, so the framework refuses such a value at the request boundary before any
/// handler runs — the branch below states that invariant rather than guarding a case a
/// request can still reach.
///
/// A call that RESOLVES something already in that namespace keeps the endpoint
/// condition ([`validate_scope`]): the global namespace is read where it lives.
pub fn validate_minted_scope(scope: &str) -> Result<(), WAFInvalidParameterException> {
    if !is_known_scope(scope) {
        return Err(invalid_scope_exception(scope));
    }
    Ok(())
}

/// What a request naming a scope is refused with, where the OPERATION judges the value
/// rather than the request boundary.
///
/// An operation whose input hands `Scope` over (`skip_validation`, see `src/ops/types/`)
/// sees every value a caller can send, and the two WAF answers them with are two
/// different refusals:
///
/// * [`Self::Parameter`] — WAF's own bad-parameter refusal, carrying `Field` SCOPE_VALUE,
///   the offending value in `Parameter` and the scope reason. This is what a scope WAF
///   READS as a scope but will not act on gets: the global namespace's own token
///   ([`crate::state::SCOPE_GLOBAL_NAMESPACE`]), and the CLOUDFRONT scope at an endpoint
///   that does not serve it.
/// * [`Self::Constraint`] — the request validator's value-set clause, for a token WAF's
///   scope vocabulary does not carry at all.
pub enum ScopeRefusal {
    /// WAF's own bad-parameter refusal for the scope.
    Parameter(WAFInvalidParameterException),
    /// The request validator's envelope, as [`crate::state::scope_value_set_envelope`]
    /// words it.
    Constraint(String),
}

impl ScopeRefusal {
    /// This refusal on the error channel of the operation that made the judgement:
    /// `modeled` is that operation's `WAFInvalidParameterException` variant, and the
    /// validator's envelope travels on the channel's own validation variant, which
    /// renders the same status and code the boundary would have.
    pub fn refuse<E>(
        self,
        modeled: impl FnOnce(WAFInvalidParameterException) -> E,
    ) -> crate::scaffolded::Scaffolded<E> {
        match self {
            Self::Parameter(refusal) => crate::scaffolded::Scaffolded::Modeled(modeled(refusal)),
            Self::Constraint(message) => crate::scaffolded::Scaffolded::validation(message),
        }
    }
}

/// [`validate_scope`] for an operation that judges the scope VALUE itself, so a value
/// outside the `Scope` enum reaches it: the same two conditions, plus the split between
/// the two refusals WAF answers an undeclared value with ([`ScopeRefusal`]).
pub fn judge_scope(scope: &str, parts: &::http::request::Parts) -> Result<(), ScopeRefusal> {
    if !crate::state::is_scope_token(scope) {
        return Err(ScopeRefusal::Constraint(
            crate::state::scope_value_set_envelope(scope),
        ));
    }
    validate_scope(scope, parts).map_err(ScopeRefusal::Parameter)
}
