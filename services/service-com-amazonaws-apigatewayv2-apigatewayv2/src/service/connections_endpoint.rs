// SPDX-License-Identifier: CC0-1.0

//! Whether a `@connections` request's endpoint names a stage THIS service has DEPLOYED.
//!
//! The three `@connections` operations are ApiGatewayManagementApi's, and they are served on
//! the owning api's own invoke endpoint — "the stage name is typically appended to this URI
//! to form a complete path to a deployed API stage" (`Api$ApiEndpoint`) — so such a request
//! carries a stage of one of THIS service's apis in its path while naming no api or stage
//! member at all. The apis, the stages and the deployment released to each stage are this
//! service's records, and the management API cannot read them (this crate already depends on
//! that one for the connection registry), so this module is the reading it asks for instead:
//! it is installed into the shared cell
//! (`service_common::execute_api_connections_endpoint`) when this service is built and
//! called with the shared engine, so it resolves the live records rather than a copy of them.
//!
//! The rule it states is the one both of this service's own data planes follow
//! ([`super::invoke`], [`super::websocket`]): an `execute-api` endpoint serves a stage of an
//! api it holds whose deployment has been released, and an addressing that resolves to no
//! such stage is not the caller's to invoke — `403` `Forbidden`
//! ([`super::data_plane::FORBIDDEN_MESSAGE`]).

use std::any::Any;

use ::service_common::execute_api_connections_endpoint::{AddressedStage, EndpointAuthority};

use super::data_plane::{api_id_of_host, host_name, path_segments};
use crate::state::ComAmazonawsApigatewayv2Apigatewayv2State as ServiceStore;

/// Install this module's reading as the judge of a `@connections` endpoint addressing.
///
/// Called once per engine, when this service is built: the management API asks the cell on
/// every `@connections` request, and an emulator this service is not mounted in leaves the
/// cell empty, which that service reads as "nothing judges this addressing".
pub fn install<Eng: Send + Sync + 'static>(engine: &Eng) {
    let Some(cell) = crate::state::connections_endpoint_authority(engine) else {
        return;
    };
    let mut cell = cell.lock();
    cell.install(judged);
    // The same records answer WHICH api's stage an addressing resolved to, which is the
    // resource an `execute-api` permission over the `@connections` surface is written about.
    cell.install_addressed_stage(addressed);
}

/// What the endpoint `host` and the invoke-path `prefix` in front of the modelled
/// `/@connections/{ConnectionId}` binding resolve to.
///
/// Installed as a `service_common::execute_api_connections_endpoint::EndpointReading`, so the
/// engine arrives as [`Any`] and the store is taken here — no lock of the asking service is
/// held while this runs.
fn judged(engine: &dyn Any, host: Option<&str>, prefix: &str) -> EndpointAuthority {
    let Some(engine) = engine.downcast_ref::<::engine::InMemoryEngine>() else {
        return EndpointAuthority::Unjudged;
    };
    let Some(state) = crate::state::service_state(engine) else {
        return EndpointAuthority::Unjudged;
    };
    let store = state.lock();
    judged_in(&store, host, prefix)
}

/// WHICH api's stage the endpoint `host` and the invoke-path `prefix` resolve to, or `None`
/// for an addressing that resolves no deployed stage of this service.
///
/// Installed as a `service_common::execute_api_connections_endpoint::AddressedStageReading`
/// beside [`judged`], and resolved by the same reading ([`resolved_in`]) so the verdict and
/// the resource it is about cannot disagree.
fn addressed(engine: &dyn Any, host: Option<&str>, prefix: &str) -> Option<AddressedStage> {
    let engine = engine.downcast_ref::<::engine::InMemoryEngine>()?;
    let state = crate::state::service_state(engine)?;
    let store = state.lock();
    resolved_in(&store, host, prefix)
}

/// [`judged`] against a store already taken: the whole reading, stated over records alone.
///
/// The VERDICT is the resolution ([`resolved_in`]) read as a yes or no, so the addressing a
/// request is served on and the resource it is authorized against are one answer: an
/// addressing that names no stage at all is judged by nobody, one that resolves a deployed
/// stage is the caller's to invoke, and every other is not.
fn judged_in(store: &ServiceStore, host: Option<&str>, prefix: &str) -> EndpointAuthority {
    let addressed = addressing(store, host, prefix);
    if path_segments(addressed.1).is_empty() {
        // The modelled path itself: the addressing names no stage, so there is nothing here
        // to judge.
        return EndpointAuthority::Unjudged;
    }
    match resolved(store, host, addressed) {
        Some(_stage) => EndpointAuthority::Deployed,
        None => EndpointAuthority::Undeployed,
    }
}

/// [`addressed`] against a store already taken: WHICH api's stage the addressing resolves to.
fn resolved_in(store: &ServiceStore, host: Option<&str>, prefix: &str) -> Option<AddressedStage> {
    resolved(store, host, addressing(store, host, prefix))
}

/// The stage one already-read `addressing` resolves — the api it names and the stage path left
/// after it — so the verdict and the resource are one resolution read once.
fn resolved<'s>(
    store: &'s ServiceStore,
    host: Option<&str>,
    addressing: (Option<&'s str>, &'s str),
) -> Option<AddressedStage> {
    let (addressed_api, stage_path) = addressing;
    let segments = path_segments(stage_path);
    let stage_name = *segments.first()?;

    // A custom domain name's BASE PATH is the other addressing that reaches a stage: the
    // mapping on that path names the api and the stage, and the host names the domain.
    if let Some(domain) = host
        .map(host_name)
        .and_then(|name| store.domain_names.get(name))
    {
        let base_path = segments.join("/");
        if let Some(mapping) = domain.api_mapping_for_key(&base_path, None) {
            return served_stage(store, &mapping.api_id, &mapping.stage);
        }
    }

    // Otherwise the FIRST segment is the stage name, of the api the endpoint NAMES.
    match addressed_api {
        // An addressing that names an api id answers for THAT api alone, whether or not this
        // emulator holds it: an endpoint of an api that does not exist serves no stage.
        Some(api_id) => served_stage(store, api_id, stage_name),
        // An addressing that names NO api id: a client on this machine cannot resolve an
        // api's `execute-api` host, so it reaches the emulator's own address, which carries
        // none. The stage is then resolved across the apis this emulator serves.
        None => store
            .apis
            .keys()
            .find_map(|api_id| served_stage(store, api_id, stage_name)),
    }
}

/// The api the addressing NAMES, and the stage path left after it.
///
/// The emulator's own rewriting of an api's endpoint carries the api id in the path ahead of
/// the stage, because a client on this machine resolves the api's `execute-api` host nowhere;
/// what is left after it is the stage path, exactly as on the documented host.
///
/// A WEBSOCKET api is reached at a socket of its own, and the address that socket landed on is
/// the endpoint its `ApiEndpoint` announces and its `@connections` calls are made at
/// ([`crate::state::websocket_transport::WebSocketTransport::api_at`]) — so a request that
/// arrived there addresses THAT api, whatever other api of this emulator holds a stage of the
/// same name.
fn addressing<'a>(
    store: &'a ServiceStore,
    host: Option<&'a str>,
    prefix: &'a str,
) -> (Option<&'a str>, &'a str) {
    if let Some((api_id, rest)) = super::invoke::local_endpoint_path(prefix) {
        return (Some(api_id), rest);
    }
    if let Some(api_id) = host.and_then(|host| store.websocket_transport.api_at(host)) {
        return (Some(api_id), prefix);
    }
    (host.and_then(api_id_of_host), prefix)
}

/// `api_id`'s stage `stage_name` as the resource an addressing resolved, when that api holds
/// such a stage with a deployment released to it — what makes an `execute-api` endpoint serve
/// that stage at all ([`super::websocket::integration::deployed`] states the same rule for
/// the WebSocket plane's own handshake).
fn served_stage(
    store: &ServiceStore,
    api_id: &str,
    stage_name: &str,
) -> Option<AddressedStage> {
    let (api, _stage, _deployed) =
        super::websocket::integration::deployed(store, api_id, stage_name)?;
    Some(AddressedStage {
        api_id: api_id.to_string(),
        region: api.region.clone(),
        stage_name: stage_name.to_string(),
    })
}
