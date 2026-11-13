// SPDX-License-Identifier: CC0-1.0

//! A GraphQL API's DATA plane: what ANSWERS at the two endpoints `CreateGraphqlApi`
//! publishes in `GraphqlApi$uris`.
//!
//! Neither value is a label. "The URIs." (`GraphqlApi$uris`) are the addresses the API is
//! served at, and both are SOCKETS this emulator answers on:
//!
//! * [`query`] — the `GRAPHQL` endpoint, which serves `POST /graphql`: the web ACL in front
//!   of the API decides the request at that edge, the material it presents has to authorize
//!   a call on the API, and the document it carries is EXECUTED against the API's own
//!   resolvers ([`execution`]);
//! * [`realtime`] — the `REALTIME` endpoint, where a subscriber completes an RFC 6455
//!   opening handshake and then exchanges frames: `connection_init`/`connection_ack`, a
//!   `start` that begins a subscription on a field of the API's schema, the `stop` that ends
//!   one, and the `data` frames a subscription receives.
//!
//! Both are opened when the API is created ([`open_for_api`], called by `CreateGraphqlApi`)
//! and closed when it is deleted, and the `host:port` each landed on is what the API
//! publishes — because the whole purpose of either value is that a client dials it, and no
//! client on this machine resolves an AWS hostname (`crate::state::api_dns`).
//!
//! They cannot be served through the emulator's front door for the same reason an Event
//! API's endpoints cannot ([`super::event_api`] states it in full): the seams a service is
//! given take one buffered request and return one buffered response, so nothing there can
//! switch a connection to another protocol and carry frames over it, and no address of that
//! door is a per-resource endpoint. The core lends a service a socket for exactly this —
//! `engine.listeners().tcp_plaintext(handler)` — binds it on the operator-chosen interface,
//! assigns the port, caps the connections and severs them when the handle is dropped. The
//! socket is handed over PLAINTEXT and the first byte says what the client opened with
//! ([`super::event_api::negotiated`]), so one announced address serves both the
//! `https://`/`wss://` a client of AWS dials and the `http://`/`ws://` a client that does
//! not verify a self-signed certificate dials, and this service never touches a key or a
//! certificate.
//!
//! What a request on either OBSERVES about the API is this service's own records, read under
//! the one shared store lock: the API, its authentication configuration, its API keys, its
//! data sources, the resolvers attached to its fields and the schema those fields belong to.

use ::std::sync::Arc;

use crate::ops::types::AdditionalAuthenticationProvider;
use crate::state::ComAmazonawsAppsyncAwsdeepdishcontrolplaneserviceState as ServiceStore;

pub mod execution;
pub mod metrics;
pub mod query;
pub mod realtime;

/// Everything one of these sockets needs to answer from: the service's own store, the API
/// the socket belongs to (the socket itself says which, so no host has to name it), where
/// that API's metrics go, and the shared engine the web ACL in front of the API is decided
/// through.
#[derive(Clone)]
pub struct PlaneContext {
    /// The shared store, so every connection reads the records the control plane writes.
    pub state: ::engine::ServiceState<ServiceStore>,
    /// The `apiId` this socket serves.
    pub api_id: String,
    /// The publisher of this API's `AWS/AppSync` series.
    pub metrics: metrics::GraphqlApiMetrics,
    /// The shared engine, kept because a request on this plane has no operation context of
    /// its own and the WAFv2 evaluation in front of the API spans two stores — WAF's own
    /// records and the CloudWatch series it reports through ([`query`]).
    pub engine: Arc<::engine::InMemoryEngine>,
}

// By hand, because a store handle has no rendering of its own: what is diagnosable about a
// socket's context is which API it serves and where its metrics go.
impl ::std::fmt::Debug for PlaneContext {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("PlaneContext")
            .field("api_id", &self.api_id)
            .field("metrics", &self.metrics)
            .finish()
    }
}

/// Open the two sockets `api_id`'s endpoints are served on and remember them, answering the
/// `host:port` each landed on — the authorities that API's `GRAPHQL` and `REALTIME`
/// endpoints publish, in that order.
///
/// The core owns the sockets, the interface, the ports and the connection caps; this hands
/// each one its handler and stores the returned handles in the service's own state, which is
/// what keeps the ports open for as long as the API exists. How many such pairs an account
/// can hold open is bounded by the create: `CreateGraphqlApi` refuses a create past the
/// AppSync per-Region quota of GraphQL APIs, so the sockets cannot grow without limit. `Err`
/// when this service was mounted on an engine with no listener surface, or when the core
/// could not bind: the caller then answers that rather than publishing an address nothing
/// serves.
pub async fn open_for_api<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
    api_id: &str,
) -> Result<(String, String), String> {
    // The engine as the shared one, kept as a handle rather than borrowed: the sockets
    // outlive this call, and what answers on them reads the WAFv2 records through it.
    let shared: Arc<::engine::InMemoryEngine> =
        (Arc::clone(engine) as Arc<dyn ::std::any::Any + Send + Sync>)
            .downcast::<::engine::InMemoryEngine>()
            .map_err(|_| "this emulator's engine has no listener surface".to_string())?;
    let state = crate::state::service_state(&*shared)
        .ok_or_else(|| "this emulator's engine does not carry the appsync store".to_string())?;
    let listeners = shared.listeners();
    let context = PlaneContext {
        state: state.clone(),
        api_id: api_id.to_string(),
        metrics: metrics::GraphqlApiMetrics::new(
            super::event_api::metrics::cloudwatch_state(&*shared),
            api_id,
        ),
        engine: Arc::clone(&shared),
    };
    // The core's own TLS server, for a client that dialled `https://` or `wss://`. A core
    // that cannot prepare it leaves this `None`, and such a client is then dropped at the
    // first byte rather than answered in the clear.
    let upgrader = listeners.tls_upgrader().ok();
    let graphql_listener = listeners
        .tcp_plaintext(query::QuerySocket::new(context.clone(), upgrader.clone()))
        .await
        .map_err(|error| format!("the API's GraphQL endpoint could not be opened: {error}"))?;
    let realtime_listener = listeners
        .tcp_plaintext(realtime::RealtimeSocket::new(context, upgrader))
        .await
        .map_err(|error| format!("the API's real-time endpoint could not be opened: {error}"))?;
    let authorities = state.lock().graphql_data_plane.remember_listeners(
        api_id,
        Arc::new(graphql_listener),
        Arc::new(realtime_listener),
    );
    ::tracing::info!(
        %api_id,
        graphql = %authorities.0,
        realtime = %authorities.1,
        "opened a GraphQL API's data-plane sockets"
    );
    Ok(authorities)
}

/// Whether the material presented authorizes a real-time connection to `api_id`, by the
/// rule AppSync applies to a call on a GraphQL API:
///
/// * material has to be PRESENTED: AppSync admits no anonymous connection, so a handshake
///   carrying none at all is unauthorized whatever the API configured;
/// * the material's provider has to be one the API set up — its `authenticationType`
///   ("The authentication type", the one every call may use) or one of its
///   `additionalAuthenticationProviders` ("A list of additional authentication providers
///   for the GraphqlApi API"). So an API configured for `API_KEY` alone refuses a token,
///   and one configured without `API_KEY` refuses a key;
/// * an API KEY has to name a key of this API that has not expired. This is the one
///   provider this service can decide itself, because it is the service that MINTED the key
///   (`CreateApiKey`), and an expired key authorizes nothing;
/// * any other material is of a provider this service does not hold the secret of — a
///   signature, a Cognito or OIDC token, a Lambda authorizer's answer. This emulator
///   verifies request signatures at its front door and a service never checks one itself,
///   and this socket is not that door, so such material is taken as PRESENTED once its
///   provider is one the API configured.
///
/// The same reading the Event API's plane applies to its own `authProviders`
/// ([`super::event_api::authorized`]), over the members a `GraphqlApi` carries them in.
pub fn authorized(
    store: &ServiceStore,
    api_id: &str,
    auth: &super::event_api::PresentedAuth,
) -> bool {
    if !auth.is_present() {
        return false;
    }
    let Some(api) = store.graphql_apis.get(api_id) else {
        return false;
    };
    let configured = |auth_type: &str| {
        api.authentication_type == auth_type
            || api
                .additional_authentication_providers
                .iter()
                .any(|provider| provider_type(provider) == Some(auth_type))
    };
    if let Some(presented) = auth.api_key.as_deref() {
        return configured(API_KEY_AUTH_TYPE)
            && store
                .api_keys_of(api_id)
                .and_then(|keys| keys.get(presented))
                .is_some_and(|key| key.expires > crate::state::unix_seconds());
    }
    // Material of every provider but `API_KEY`: authorized where the API configured any of
    // those providers. A provider that NAMES no authentication type sets none up, so it
    // configures nothing — the same reading [`provider_type`] gives the key branch above.
    api.authentication_type != API_KEY_AUTH_TYPE
        || api
            .additional_authentication_providers
            .iter()
            .any(|provider| matches!(provider_type(provider), Some(named) if named != API_KEY_AUTH_TYPE))
}

/// The authentication type one additional provider sets up, or `None` for a provider that
/// names none.
fn provider_type(provider: &AdditionalAuthenticationProvider) -> Option<&str> {
    provider.authentication_type.as_deref()
}

/// The authorization TYPE an API key belongs to, as `AuthenticationType` spells it.
const API_KEY_AUTH_TYPE: &str = "API_KEY";
