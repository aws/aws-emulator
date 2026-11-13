// SPDX-License-Identifier: CC0-1.0

//! WHERE this service's own routes are served: the endpoint a control call arrives on,
//! and not an address this service announced for a DATA plane.
//!
//! `GetDataEndpoint` exists because a stream's media APIs are not served at the
//! control-plane endpoint — "Gets an endpoint for a specified stream for either reading or
//! writing" — and the separation holds in both directions: the address announced for a
//! plane serves that plane's media routes, and the control operations are served at the
//! endpoint the discovery call itself was made to. Kinesis Video Media states the same
//! rule from the data side ("Caller used wrong endpoint to write data to a stream"), and a
//! public data host carries no control route at all.
//!
//! So a control-plane call that reached an address this store announced for a plane
//! ([`super::super::state::published_plane`]) reached an endpoint that binds no route for
//! its path at all, and is answered as such: the status this family gives a path it routes
//! nothing for, with NO body, and the refusal's class in the `x-amzn-errortype` header
//! ([`super::unknown_operation::UNKNOWN_OPERATION_CODE`]) — no operation run, no member
//! reported, no message member of any kind, and nothing in the store touched. The class is
//! stated where a restJson1 client reads a failure's code because this is the refusal a
//! client re-discovers its endpoint off: it is the one a caller reaches by addressing the
//! wrong plane of an endpoint set it was HANDED. Every other address — the endpoint the
//! client is already talking to, this service's published regional host — keeps serving the
//! whole control surface.

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use ::smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

/// Whether this request reached an address this store announced for a data plane.
///
/// Only the addresses this emulator PUBLISHED are read: it knows the plane of those and of
/// no others, so a request that reached a host naming a service — a caller resolving AWS's
/// own hostnames — is served the control surface as before.
pub fn addressed_a_data_plane<Eng: Send + Sync + 'static>(engine: &Eng, req: &RtRequest) -> bool {
    let host = req.headers().get(::http::header::HOST.as_str());
    let Some(address) =
        ::service_common::data_plane_addressing::reached_emulator_endpoint(req.uri(), host)
    else {
        return false;
    };
    crate::state::published_plane(engine, &address).is_some()
}

/// Answers a control-plane call that reached a data plane's address: the refusal an
/// endpoint gives a path it routes nothing for.
///
/// Served as an operation rather than decided in `can_handle`, because the answer is this
/// endpoint's own and not the router's "no service claimed it": the request did reach this
/// emulator, at an address that serves another plane's routes.
#[derive(Debug, Clone, Copy)]
pub struct UnroutedOnDataPlaneAddress;

impl ErasedOperation for UnroutedOnDataPlaneAddress {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        _req: &'r RtRequest,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move { unrouted_address_response() })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // The MODELLED operation the routing key names decides which schemes sign a
        // request, and the router resolves that from the routing table rather than from
        // the operation this service handed back, so nothing here weakens them.
        None
    }
}

/// The front door's own answer to a request for a control route that reached an address
/// this service announced for a data plane: the 404 an endpoint gives a path it routes
/// NOTHING for, identified by the refusal's own class and carrying no body at all.
///
/// A data endpoint hosts that plane's media APIs only, so a control route asked for there
/// is a path that endpoint binds no route for — the status an endpoint answers such a path
/// with ([`super::unknown_operation::UNROUTED_PATH_STATUS`]), and NO body: no operation was
/// identified, so there is no member, value or sentence to state, and no message member of
/// any kind is reported.
///
/// What it does state is its CLASS, in the `x-amzn-errortype` header
/// ([`super::unknown_operation::UNKNOWN_OPERATION_CODE`]) — the one field a restJson1 client
/// resolves a failure's code from, and the only thing in this answer. This is the refusal a
/// client that followed `GetDataEndpoint` and then addressed the wrong plane of the endpoint
/// set it was handed meets, and the code is what it branches on to discover its endpoint
/// again, so an answer whose class an SDK cannot read leaves that loop unable to fire.
fn unrouted_address_response() -> RtResponse {
    let mut response = RtResponse::new(
        ::aws_smithy_runtime_api::http::StatusCode::try_from(
            super::unknown_operation::UNROUTED_PATH_STATUS,
        )
        .expect("the status is a constant of the front door's own module"),
        ::aws_smithy_types::body::SdkBody::empty(),
    );
    response.headers_mut().insert(
        "x-amzn-errortype",
        super::unknown_operation::UNKNOWN_OPERATION_CODE,
    );
    response
}
