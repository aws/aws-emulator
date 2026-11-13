// SPDX-License-Identifier: CC0-1.0

//! WHERE this service's modelled route is served: the address the control plane announces
//! for this media endpoint, and nowhere else on this emulator.
//!
//! `GetDataEndpoint` exists because a stream's media APIs are not served at the
//! control-plane endpoint — "Gets an endpoint for a specified stream for either reading or
//! writing … You must first call the `GetDataEndpoint` API to get an endpoint. Then send
//! the `GetMedia` requests to this endpoint using the `--endpoint-url` parameter" — and a
//! media API is bound on the endpoint whose model declares it and on no other: this
//! service's own `InvalidEndpointException` states the rule from the data side ("Caller
//! used wrong endpoint to write data to a stream. On receiving such an exception, the user
//! must call `GetDataEndpoint` with `AccessMode` set to \"READ\" and use the endpoint
//! Kinesis Video returns in the next `GetMedia` call").
//!
//! This service IS the Kinesis Video Media endpoint: its model binds `POST /getMedia` and
//! it serves the recorded `/putMedia` ingest beside it, so every route it binds is that
//! endpoint's — the address `GetDataEndpoint` hands back for `GET_MEDIA` and `PUT_MEDIA`.
//! A reader that skipped discovery and sent the call to the control-plane front door, or
//! that cached the archived-media endpoint's address, reached an endpoint that binds no
//! operation for `/getMedia`, and is answered as the unroutable request it is — this family's front-door envelope
//! ([`::service_com_amazonaws_kinesisvideo_kinesisvideo_20170930::service::unknown_operation`])
//! — with no stream resolved and nothing read.
//!
//! The sibling [`super::put_media`] scopes the ingest route the same way, against the same
//! reading of which plane an address belongs to, so the two routes of this one endpoint
//! cannot drift on where they are served.

use ::aws_smithy_runtime_api::http::Request as RtRequest;
use ::service_com_amazonaws_kinesisvideo_kinesisvideo_20170930::state as control_plane;

/// Whether a request addresses a route this service's MODEL binds.
///
/// Read off the generated, model-derived segment list rather than a literal, so a route
/// this service's model gains is scoped with the rest of them. The route key is checked by
/// the caller, which owns that half of the rule for every one of this service's routes.
pub fn claims_request(req: &RtRequest) -> bool {
    ::service_common::nth_path_segment(::service_common::request_path(req.uri()), 0)
        .is_some_and(|segment| crate::ROUTE_PATH_SEGMENTS.contains(&segment))
}

/// Whether the ADDRESS a request arrived on is one this service's read routes are served
/// at: the address announced for this media endpoint.
///
/// Which plane an address IS comes from the control plane that announced it
/// ([`control_plane::published_plane`]): this emulator knows the plane of the addresses it
/// published and of no others, so a request that reached a host NAMING a service — a
/// caller resolving AWS's own hostnames rather than an address this emulator handed out —
/// is left to the route's own claim.
pub fn read_plane_served_at<Eng: Send + Sync + 'static>(engine: &Eng, req: &RtRequest) -> bool {
    let host = req.headers().get(::http::header::HOST.as_str());
    let Some(address) =
        ::service_common::data_plane_addressing::reached_emulator_endpoint(req.uri(), host)
    else {
        return true;
    };
    control_plane::published_plane(engine, &address) == Some(control_plane::DataPlane::Media)
}
