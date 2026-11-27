// SPDX-License-Identifier: CC0-1.0

//! A GraphQL request that reaches a `GraphqlApi`'s `GRAPHQL` endpoint from INSIDE this
//! emulator.
//!
//! Both of an API's endpoints are SOCKETS the core lends this service ([`super`] states
//! why), and the `GRAPHQL` one is the address a CLIENT posts a document to. Another service
//! of this same emulator that has to reach it — EventBridge delivering a matched event to
//! an AppSync target, which is the `appsync:GraphQL` call that target's ARN names — is not
//! such a client: it holds no connection, dials nothing and has no address to dial, because
//! the endpoint is not a route of the emulator's front door.
//!
//! So the request is HANDED to the very code that socket serves
//! ([`query::served_buffered_request`]), from the context that API is served from
//! (`crate::state::GraphqlDataPlane::plane`): the web ACL in front of the API decides it,
//! the material it presents has to authorize a call on the API, each field it selects is
//! judged for `appsync:GraphQL`, the document is executed against the API's own resolvers
//! and the API logs what it served — all exactly as they are for a request that arrived on
//! the socket, under the same PANIC CONTAINMENT, so a panic under it is this endpoint's own
//! `500` rather than something that unwinds into the caller's own work. Nothing is answered
//! here and nothing of the API is read here: this module only resolves WHICH API the request
//! is for and hands it over, so a caller inside this emulator can observe no answer a client
//! on the socket could not.
//!
//! Two readings belong with it, because both are about an API this service PUBLISHED an
//! address for: which API an endpoint address names ([`api_at_graphql_endpoint`]), and which
//! FIELDS of it one operation addresses ([`addressed_field_resources`]) — the resources an
//! IAM policy grants `appsync:GraphQL` on, which a caller's own authorization decision about
//! the call is taken over.

use ::bytes::Bytes;
use ::http_body_util::BodyExt as _;
use ::serde_json::{Map as JsonMap, Value as Json};

use crate::state::ComAmazonawsAppsyncAwsdeepdishcontrolplaneserviceState as ServiceStore;

use super::execution::{self, GraphqlRequest};
use super::{caller, query};

/// The API whose `GRAPHQL` endpoint publishes `label`, or `None` when `label` names no one
/// API of this account.
///
/// `label` is the part of an AppSync GraphQL endpoint's ARN that names the API — the label
/// of the host that endpoint is published at. AppSync publishes an API's endpoint at
/// `<apiId>.appsync-api.<region>.<zone>`, where the label IS the API's id, and this emulator
/// publishes the same name in a zone of its own (`crate::state::api_endpoints`) over the port
/// of the socket that serves it, because no client on this machine resolves an AWS hostname —
/// so a caller that read the label off the published address carries a part of THAT address
/// instead. Both are the same question, "which API is served there", so every form of the
/// answer is accepted: the API's own id, the whole published authority, its host, and the
/// first label of that host.
///
/// A label that names SEVERAL of this account's APIs names none of them in particular, so it
/// resolves to nothing rather than to a guess, and a caller reports the target as one it
/// could not address.
#[must_use]
pub fn api_at_graphql_endpoint(
    state: &::engine::ServiceState<ServiceStore>,
    label: &str,
) -> Option<String> {
    if label.is_empty() {
        return None;
    }
    let store = state.lock();
    if store.graphql_apis.contains_key(label) {
        return Some(label.to_string());
    }
    let mut matched: Vec<String> = Vec::new();
    for (api_id, api) in store.graphql_apis.iter() {
        let Some(endpoint) = api.endpoints.get(crate::state::GRAPHQL_ENDPOINT) else {
            continue;
        };
        let host = endpoint.host.as_str();
        let first_label = host.split('.').next().unwrap_or(host);
        if endpoint.authority() == label || host == label || first_label == label {
            matched.push(api_id.clone());
        }
    }
    match matched.as_slice() {
        [api_id] => Some(api_id.clone()),
        _ => None,
    }
}

/// The resources an IAM policy grants `appsync:GraphQL` on for the operation `query` executed
/// on `api_id`: for EVERY root field that document selects, the API's own ARN extended with
/// the type and the field ([`caller::field_resource`]), which is how one field of one API is
/// addressed.
///
/// Every one of them, because a document may select several root fields and "you can use IAM
/// policies to restrict access to … individual fields": a call reaches each field it selects,
/// so each is a resource the action is performed on, and a caller judging the call has to be
/// permitted all of them. The document is read exactly as an executed request's is
/// ([`execution::selected_root_fields`]), so the fields a policy is judged against are the
/// ones the execution would run. Empty for a document that selects no field of the API at all
/// — one that does not parse, or that names no operation of it — and for an API this account
/// does not hold: there is no field for a statement to be about then.
#[must_use]
pub fn addressed_field_resources(
    state: &::engine::ServiceState<ServiceStore>,
    api_id: &str,
    query: &str,
    variables: &JsonMap<String, Json>,
) -> Vec<String> {
    let Some((plane, api_arn)) = ({
        // One read for both, so the context and the ARN are of the same API as the store
        // holds it at one instant; the guard is released before the document is read.
        let store = state.lock();
        store
            .graphql_data_plane
            .plane(api_id)
            .zip(store.graphql_apis.get(api_id).map(|api| api.arn.clone()))
    }) else {
        return Vec::new();
    };
    let request = GraphqlRequest {
        query: query.to_string(),
        variables: variables.clone(),
        ..GraphqlRequest::default()
    };
    execution::selected_root_fields(&plane, &request)
        .into_iter()
        .map(|(type_name, field_name)| caller::field_resource(&api_arn, &type_name, &field_name))
        .collect()
}

/// Serve `request` as a request to `api_id`'s `GRAPHQL` endpoint, and answer what that
/// endpoint answered — the status and the body, buffered.
///
/// `None` for an API this emulator serves no endpoint of: one this account never created,
/// and one whose delete took its records and its sockets with it. That is the ABSENCE of
/// the API rather than an answer from it, so the caller reports it as such rather than
/// reading a refusal into it.
pub async fn served(
    state: &::engine::ServiceState<ServiceStore>,
    api_id: &str,
    request: ::http::Request<Vec<u8>>,
) -> Option<::http::Response<Bytes>> {
    // The context is cloned out under the store's own lock, which is then released: the
    // answer below takes that lock itself, as every request on the socket does.
    let plane = state.lock().graphql_data_plane.plane(api_id)?;
    let (parts, body) = request.into_parts();
    let (parts, body) = query::served_buffered_request(&plane, &parts, &body)
        .await
        .into_parts();
    // The answer's body is already in memory (`Full<Bytes>`, whose collection cannot
    // fail); this only takes it out of the body type the endpoint writes.
    let collected = body
        .collect()
        .await
        .map_or_else(|_| Bytes::new(), |collected| collected.to_bytes());
    Some(::http::Response::from_parts(parts, collected))
}
