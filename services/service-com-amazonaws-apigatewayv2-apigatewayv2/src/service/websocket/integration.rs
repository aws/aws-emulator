// SPDX-License-Identifier: CC0-1.0

//! What a WEBSOCKET route's INTEGRATION answers, with the backend it fronts actually
//! performed.
//!
//! [`super::dispatch`] decides WHICH route serves a call and what an integration that
//! reaches no backend (`MOCK`) replies. This module is the other half: the call of the
//! backend itself, for the integration type a WebSocket route fronts a function with.
//!
//! * `AWS_PROXY` naming no `IntegrationSubtype` is the LAMBDA PROXY integration — "for
//!   integrating the route or method request with the Lambda function-invoking action with
//!   the client request passed through as-is" (`Integration$IntegrationType`). The call is
//!   passed through as the event a WebSocket api states ([`super::event`]), the function is
//!   RUN through Lambda's own modelled `Invoke` over the in-process transport the api's
//!   socket was opened with, and its answer is read back as the structured response
//!   ([`crate::service::invoke::proxy_response::websocket_reply`]) — so the status that
//!   decides the call and the payload a two-way route sends back are the function's own.
//! * `MOCK` is answered from the integration's own records ([`super::dispatch::mock_reply`]).
//! * Every other type — `AWS` and `HTTP` with their templates, `HTTP_PROXY` with an upstream
//!   URI this emulator opens no connection to, and a first-class service action — reaches a
//!   backend this data plane does not call over a socket, so it is reported as
//!   unperformable: the gateway's own `Internal server error` rather than an answer no
//!   backend gave.
//!
//! Every answer is the backend's: a function that raised, one that could not be invoked, one
//! that overran the integration's own `TimeoutInMillis` and one whose answer the structured
//! response cannot be read as are all "the integration produced no reply", with the reason
//! logged and never told to the client.

use std::time::Duration;

use ::smithy_server_runtime::EmbeddedEmulator;

use super::dispatch::{self, TemplateCall, Unserved};
use super::event::{self, Call, Connection};
use crate::service::invoke::{lambda, proxy_response};
use crate::state::{Api, ComAmazonawsApigatewayv2Apigatewayv2State as ServiceStore};
use crate::state::{DeployedConfiguration, Integration, Route, Stage};

/// What a route's integration ANSWERED for one call of a connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answered {
    /// The status the call produced — the `statusCode` a function answered, or the one a
    /// `MOCK` integration's rendered request template named — or `None` when the call
    /// produced none at all.
    pub status_code: Option<String>,
    /// The payload a TWO-WAY route sends back to the client, when the integration produced
    /// one.
    pub payload: Option<Vec<u8>>,
}

/// One call's route and integration, read out of the store under ONE lock and OWNED from
/// then on: performing the integration is awaited, and no state guard may be held across an
/// await — one connection's backend must not stall another connection's frame.
#[derive(Debug, Clone)]
pub struct Selected {
    /// The `RouteKey` of the route that serves the call.
    pub route_key: String,
    /// Whether that route declares a `RouteResponse`, which is what makes it two-way.
    pub two_way: bool,
    /// The integration the route targets, or `None` for a route that targets none — which
    /// has nothing to call at all.
    pub integration: Option<Integration>,
}

impl Selected {
    /// The route and its integration, as the deployment the stage released froze them.
    fn of(deployed: &DeployedConfiguration, route: &Route) -> Self {
        Self {
            route_key: route.route_key.clone(),
            two_way: dispatch::is_two_way(route),
            integration: dispatch::targeted_integration(deployed, route).cloned(),
        }
    }
}

/// The api and the configuration the stage's deployment FROZE — what that stage serves.
///
/// A stage the api does not hold, and one with no deployment released to it, resolve to
/// nothing: such an endpoint serves no call at all.
pub fn deployed<'s>(
    store: &'s ServiceStore,
    api_id: &str,
    stage_name: &str,
) -> Option<(&'s Api, &'s Stage, &'s DeployedConfiguration)> {
    let api = store.apis.get(api_id)?;
    let stage = api.stages.get(stage_name)?;
    let deployment = api.deployments.get(stage.deployment_id.as_deref()?)?;
    Some((api, stage, &deployment.configuration))
}

/// The route of this deployment whose `RouteKey` is `route_key`, selected for performing, or
/// `None` when the deployment holds none.
pub fn select_route_key(deployed: &DeployedConfiguration, route_key: &str) -> Option<Selected> {
    dispatch::deployed_route(deployed, route_key).map(|route| Selected::of(deployed, route))
}

/// The route that serves the message `body`, selected for performing, or `None` when the api
/// routes it nowhere ([`super::dispatch::message_route`]).
pub fn select_message(
    api: &Api,
    deployed: &DeployedConfiguration,
    body: &str,
) -> Option<Selected> {
    dispatch::message_route(api, deployed, body).map(|route| Selected::of(deployed, route))
}

/// Perform `selected`'s integration for one `call` of `connection`, and answer what it
/// replied.
///
/// `request_id` is the id of THIS call, minted by the socket that carries it: the event the
/// backend is handed reports it, and so does the stage's access-log record of the same call
/// ([`super::access_logs`]), which is how a client joins the two.
pub async fn perform(
    callback: &EmbeddedEmulator,
    connection: &Connection,
    selected: &Selected,
    call: &Call,
    request_id: &str,
) -> Result<Answered, Unserved> {
    let integration = selected
        .integration
        .as_ref()
        .ok_or(Unserved::NoIntegration)?;
    if integration.integration_type == dispatch::MOCK_INTEGRATION_TYPE {
        // The template is rendered against the CALL: the `$context` the gateway reports
        // about it and the serving stage's own variables, whichever of the three calls it
        // is, and the frame's payload for the one call that carries a frame.
        let context =
            event::template_context_of(connection, &selected.route_key, call, request_id);
        let frame = match call {
            Call::Message(body) => Some(TemplateCall::new(
                &context,
                &connection.stage_variables,
                Some(body),
            )),
            // A connection request a `MOCK` integration fronts has no decision to be
            // admitted by at all ([`dispatch::no_connection_decision`]).
            Call::Connect => return Err(dispatch::no_connection_decision()),
            // A close carries no frame of its own, so a template rendered for it reads the
            // call's context and nothing of a payload.
            Call::Disconnect { .. } => {
                Some(TemplateCall::new(&context, &connection.stage_variables, None))
            }
        };
        let reply = dispatch::mock_reply(integration, frame)?;
        return Ok(Answered {
            status_code: reply.status_code,
            payload: reply.payload.map(String::into_bytes),
        });
    }
    if !crate::state::is_lambda_proxy_integration(
        &integration.integration_type,
        integration.integration_subtype.as_deref(),
    ) {
        return Err(Unserved::Unperformable(format!(
            "an integration of type {}{} fronts a backend this data plane does not call for \
             a WebSocket route",
            integration.integration_type,
            integration
                .integration_subtype
                .as_deref()
                .map(|subtype| format!(" with the subtype {subtype}"))
                .unwrap_or_default()
        )));
    }
    lambda_proxy(callback, connection, selected, integration, call, request_id).await
}

/// Run the function this route's Lambda proxy integration names, with the event this call
/// is, and read its answer back.
async fn lambda_proxy(
    callback: &EmbeddedEmulator,
    connection: &Connection,
    selected: &Selected,
    integration: &Integration,
    call: &Call,
    request_id: &str,
) -> Result<Answered, Unserved> {
    // The stage's own variables are interpolated into the function the integration names
    // before it is addressed: "you can use API Gateway stage variables … as part of an HTTP
    // integration URL" and as part of the function a Lambda integration names, which is what
    // lets one api reach a different backend per stage. A reference the stage carries no
    // variable for names no function at all.
    let uri = integration
        .integration_uri
        .as_deref()
        .map(str::trim)
        .filter(|uri| !uri.is_empty())
        .and_then(|uri| {
            crate::service::data_plane::resolve_stage_variables(uri, &connection.stage_variables)
        })
        .ok_or_else(|| {
            Unserved::Unperformable(
                "this route's AWS_PROXY integration names no function this stage can reach"
                    .to_string(),
            )
        })?;
    let target = lambda::function_target(&uri).ok_or_else(|| {
        Unserved::Unperformable(
            "this route's AWS_PROXY integration names no Lambda function to invoke".to_string(),
        )
    })?;
    // The integration's own `RequestParameters` are applied BEFORE the event is stated: they
    // are "request parameters that are passed from the method request to the backend"
    // (`Integration$RequestParameters`), so what the function is told is the call as that
    // mapping leaves it ([`super::parameters`]).
    let mapped = super::parameters::mapped(connection, &integration.request_parameters);
    let event = event::event_of(&mapped, &selected.route_key, call, request_id);
    let payload = ::serde_json::to_vec(&event).map_err(|error| {
        Unserved::Unperformable(format!("the event for this call could not be stated: {error}"))
    })?;
    let ran = lambda::invoke_function(callback, &target, payload, deadline(integration))
        .await
        .map_err(|unreached| Unserved::Unperformable(unreached.reason()))?;
    if ran.failed() {
        // "An unhandled error is still HTTP 200" to Lambda's own caller, so the signal is
        // what tells a run that ended in an error from one that answered: the payload is
        // Lambda's error document, not a reply to read.
        return Err(Unserved::Unperformable(format!(
            "this route's function ended in an unhandled error ({}), so the integration \
             produced no reply",
            ran.function_error.as_deref().unwrap_or_default()
        )));
    }
    let (status, body) = proxy_response::websocket_reply(&ran.payload)
        .map_err(|malformed| Unserved::Unperformable(malformed.reason))?;
    Ok(Answered {
        status_code: Some(status.to_string()),
        payload: (!body.is_empty()).then_some(body),
    })
}

/// The deadline the integration gives its backend: its own `TimeoutInMillis`, "a custom
/// timeout between 50 and 29,000 milliseconds for WebSocket APIs"
/// (`Integration$TimeoutInMillis`), which the create and update of an integration bound to
/// that window, so the stored value is always a duration.
fn deadline(integration: &Integration) -> Duration {
    Duration::from_millis(u64::try_from(integration.timeout_in_millis.max(0)).unwrap_or_default())
}
