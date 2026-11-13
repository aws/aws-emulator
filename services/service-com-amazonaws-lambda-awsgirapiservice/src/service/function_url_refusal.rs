// SPDX-License-Identifier: CC0-1.0

//! How a request to a FUNCTION URL that the emulator's authentication layer turned away is
//! worded.
//!
//! A function URL whose `AuthType` is `AWS_IAM` "restrict[s] access to authenticated users
//! only", so the mounting that serves it declares the SigV4 scheme and the emulator refuses
//! an unsigned request — or one whose signature does not verify — before the plane runs
//! ([`super::function_url`]). What a CLIENT of that endpoint is then told is the front
//! door's: the sentence AWS's Lambda front door answers a caller it does not admit with,
//! `403 {"Message":"Forbidden"}`, which is the same answer that endpoint gives a caller its
//! resource-based policy does not admit.
//!
//! The emulator's authentication layer words its refusal in the structural vocabulary of an
//! API error instead (an error code and a `message` member), because that is the right answer
//! for a request to this service's modelled API. A function URL is not that API: it is an
//! HTTP endpoint a stock client follows, and a client reading the API's refusal there reads a
//! body no function URL ever answers. So this module restates the one refusal, for the one
//! request space it applies to.
//!
//! WHICH requests those are is read the same way the plane claims them — the stored function
//! URL the request's address resolves to, out of the one shared store
//! ([`super::function_url`]) — so the endpoint that announced the address and the endpoint
//! that refuses the caller cannot disagree about which requests belong to it. Every other
//! refusal, of every other request, passes through exactly as the generated definition words
//! it: the identity, the protocol, the routing table and every other rendering are that
//! definition's and are restated nowhere here.
//!
//! One refusal that arrives here looking the same is NOT this one, and keeps the words it was
//! given: the emulator's refusal of a request whose browser-set headers say a browser sent it
//! from another site. That guard is a property of this emulator, which serves a developer's
//! machine, rather than of the service it emulates, so a caller it turned away is told so
//! rather than being given Lambda's own sentence. Which of the two a refusal is, is read with
//! the CORE's own classification of those headers ([`::emulator_server::classify_browser_request`]),
//! so the two cannot disagree.

use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::ProtocolError;
use smithy_server_runtime::{
    ErasedServiceDefinition, OperationRoutingTable, Refused, ServerHttpProtocol, ServiceIdentity,
    WireProtocol,
};

/// This service's generated definition with the function-URL refusal above stated in the
/// words that endpoint answers.
pub struct FunctionUrlRefusals<Eng: Send + Sync + 'static> {
    /// The model-derived half of this service, which every other question is answered from.
    inner: crate::ServiceDefinition<Eng>,
    /// The ONE shared engine, so the function URLs a request is read against are the ones
    /// the control plane persisted.
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> FunctionUrlRefusals<Eng> {
    /// Wrap the generated definition built on `engine`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            inner: crate::ServiceDefinition::new(Arc::clone(&engine)),
            engine,
        }
    }

    /// Whether `req` addresses a function URL this store HOLDS, by the same reading the
    /// plane claims one with.
    fn addresses_function_url(&self, req: &RtRequest) -> bool {
        super::function_url::target_of(
            &*self.engine,
            &crate::state::AddressedFunctionUrl::of_request(req),
        )
        .is_some()
    }
}

impl<Eng: Send + Sync + 'static> ErasedServiceDefinition for FunctionUrlRefusals<Eng> {
    fn identity(&self) -> &ServiceIdentity {
        self.inner.identity()
    }

    fn protocol(&self) -> &dyn ServerHttpProtocol {
        self.inner.protocol()
    }

    fn routing_table(&self) -> &OperationRoutingTable {
        self.inner.routing_table()
    }

    fn routing_error_response(&self, req: &RtRequest) -> RtResponse {
        self.inner.routing_error_response(req)
    }

    fn framework_error_response(&self, error: ProtocolError, req: &RtRequest) -> RtResponse {
        self.inner.framework_error_response(error, req)
    }

    fn refusal_response(
        &self,
        error: ProtocolError,
        chosen: Option<Refused>,
        req: &RtRequest,
    ) -> RtResponse {
        // ONE refusal is restated: the caller the authentication layer did not admit, to an
        // address that is a function URL of this store. Everything else — every refusal of
        // the modelled API, the emulator's own cross-site guard, and every refusal a handler
        // or this service worded itself — is the generated definition's.
        if matches!(error, ProtocolError::Unauthenticated { .. })
            && !sent_cross_site_by_a_browser(req)
            && self.addresses_function_url(req)
        {
            return super::function_url::forbidden();
        }
        self.inner.refusal_response(error, chosen, req)
    }

    fn wire_protocol(&self, req: &RtRequest) -> WireProtocol<'_> {
        self.inner.wire_protocol(req)
    }
}

/// Whether `req` is a request the emulator's own cross-site guard turns away: one whose
/// browser-set headers say a browser sent it from another site.
///
/// The CORE's own classification of those headers, under the operator default that serves
/// same-origin and non-browser callers only — so this reading and the guard in front of every
/// endpoint this emulator serves cannot disagree about which requests it refuses.
fn sent_cross_site_by_a_browser(req: &RtRequest) -> bool {
    let header = |name: &str| req.headers().get(name);
    matches!(
        ::emulator_server::classify_browser_request(
            header("origin"),
            header("sec-fetch-site"),
            &::emulator_server::RemoteOrigins::SameOriginOnly,
            false,
        ),
        ::emulator_server::BrowserVerdict::Forbid(_)
    )
}
