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

use ::service_common::execute_api_connections_endpoint::EndpointAuthority;

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
    cell.lock().install(judged);
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

/// [`judged`] against a store already taken: the whole reading, stated over records alone.
fn judged_in(store: &ServiceStore, host: Option<&str>, prefix: &str) -> EndpointAuthority {
    // The emulator's own rewriting of an api's endpoint carries the api id in the path ahead
    // of the stage, because a client on this machine resolves the api's `execute-api` host
    // nowhere; what is left after it is the stage path, exactly as on the documented host.
    let (addressed_api, stage_path) = match super::invoke::local_endpoint_path(prefix) {
        Some((api_id, rest)) => (Some(api_id), rest),
        None => (host.and_then(api_id_of_host), prefix),
    };
    let segments = path_segments(stage_path);
    let Some(stage_name) = segments.first() else {
        // The modelled path itself: the addressing names no stage, so there is nothing here
        // to judge.
        return EndpointAuthority::Unjudged;
    };

    // A custom domain name's BASE PATH is the other addressing that reaches a stage: the
    // mapping on that path names the api and the stage, and the host names the domain.
    if let Some(domain) = host
        .map(host_name)
        .and_then(|name| store.domain_names.get(name))
    {
        let base_path = segments.join("/");
        if let Some(mapping) = domain.api_mapping_for_key(&base_path, None) {
            return authority(serves_stage(store, &mapping.api_id, &mapping.stage));
        }
    }

    // Otherwise the FIRST segment is the stage name, of the api the endpoint NAMES.
    match addressed_api {
        // An addressing that names an api id answers for THAT api alone, whether or not this
        // emulator holds it: an endpoint of an api that does not exist serves no stage.
        Some(api_id) => authority(serves_stage(store, api_id, stage_name)),
        // An addressing that names NO api id: a client on this machine cannot resolve an
        // api's `execute-api` host, so it reaches the emulator's own address, which carries
        // none. The stage is then resolved across the apis this emulator serves.
        None => authority(
            store
                .apis
                .keys()
                .any(|api_id| serves_stage(store, api_id, stage_name)),
        ),
    }
}

/// Whether `api_id` holds a stage named `stage_name` with a deployment released to it — what
/// makes an `execute-api` endpoint serve that stage at all
/// ([`super::websocket::integration::deployed`] states the same rule for the WebSocket
/// plane's own handshake).
fn serves_stage(store: &ServiceStore, api_id: &str, stage_name: &str) -> bool {
    super::websocket::integration::deployed(store, api_id, stage_name).is_some()
}

/// The authority a resolution answers with.
fn authority(deployed: bool) -> EndpointAuthority {
    if deployed {
        EndpointAuthority::Deployed
    } else {
        EndpointAuthority::Undeployed
    }
}
