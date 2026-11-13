// SPDX-License-Identifier: CC0-1.0

//! The REST endpoint a `ProductRestEndpointPage` documents, as the service that OWNS REST
//! APIs reports it.
//!
//! A REST API is not a resource of this service: an `IdentifierParts` names one by its
//! `RestApiId` and one of its stages, and both live in the API Gateway (v1) service, which
//! is where a client created them (`apigateway:CreateRestApi`, `apigateway:CreateStage`).
//! A page's content is GENERATED from that endpoint — "By default, this is the
//! documentation of your REST API from API Gateway"
//! (`DisplayContentOverrides$Body`) — so whether there IS such an endpoint to document is a
//! question only the owner of REST APIs can answer, and it is resolved there rather than
//! guessed here. A write naming one the account does not have is refused.
//!
//! The owner is reached through its own `state` accessor on the SHARED engine, the way
//! [`crate::service::authorization`] reads the IAM and STS stores, and under the same lock
//! order: the other service's lock is taken and released BEFORE this service's own store
//! lock, so every caller resolves first and reaches its own store after. The v1 service
//! reads nothing of this one's state.
//!
//! Nothing is cached: a write's identifier is resolved by that write, so a REST API created
//! after it does not retroactively change what an already-stored page documents, and a read
//! of the page asks nothing of anybody.

use super::*;

/// The sentence this service refuses a `RestEndpointIdentifier` that names no REST API, or
/// no stage of the API it names, with: the identifier identifies no endpoint to document.
///
/// Held here once, so every writer of a page refuses the same fact with the same words. It
/// has no variable parts — API Gateway names neither the API nor the stage in it.
pub const UNRESOLVED_REST_ENDPOINT_MESSAGE: &str =
    "Invalid restEndpointIdentifier stage or restApiId";

/// `Some(the refusal message)` when the REST endpoint `parts` identify cannot be resolved,
/// and `None` when it resolves.
///
/// A page's content is GENERATED from the endpoint it documents, so a write naming one that
/// does not exist describes no page at all: the write is refused `BadRequestException`
/// rather than storing a page that points at nothing.
///
/// A HOP THAT CANNOT BE MADE IS EVIDENCE OF NOTHING: an engine carrying no API Gateway (v1)
/// store cannot be asked which REST APIs the account has, so nothing is refused there. That
/// is the same reading [`super::ec2_subnets`] takes of silence from the owner of subnets.
///
/// Call it BEFORE this service's own store lock is taken — see this module's documentation.
pub fn unresolved_rest_endpoint<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    parts: &wire::IdentifierParts,
) -> Option<String> {
    let owner = ::service_com_amazonaws_apigateway_backplanecontrolservice::state::service_state(
        engine,
    )?;
    let resolved = {
        let rest_apis = owner.lock();
        rest_apis
            .rest_apis
            .get(&parts.rest_api_id)
            // A page documents ONE endpoint of ONE stage, and it is the stage that is
            // deployed and invocable, so an API the account has under a name it has no such
            // stage for identifies nothing either.
            .is_some_and(|rest_api| rest_api.stages.contains_key(&parts.stage))
    };
    (!resolved).then(|| UNRESOLVED_REST_ENDPOINT_MESSAGE.to_string())
}
