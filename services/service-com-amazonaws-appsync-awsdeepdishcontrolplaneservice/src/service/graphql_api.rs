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
//! What a call presenting a TOKEN is decided by is the mode that issued it: a Cognito user
//! pool token is verified against the pool the API configured ([`user_pool`]), an OpenID
//! Connect token against the provider it configured ([`open_id_connect`]), and a Lambda
//! authorizer's input is handed to the function the API named ([`authorizer`]). Both endpoints
//! read that decision through ONE seam ([`token_decision`]), so a token admitted on a call is
//! admitted on a connection and refused alike.
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

use ::smithy_server_runtime::EmbeddedEmulator;

use crate::ops::types::AdditionalAuthenticationProvider;
use crate::state::ComAmazonawsAppsyncAwsdeepdishcontrolplaneserviceState as ServiceStore;

pub mod api_cache;
pub mod authorizer;
pub mod caller;
pub mod data_source_role;
pub mod dynamodb;
pub mod enhanced_metrics;
pub mod event_bridge;
pub mod execution;
pub mod field_auth;
pub mod field_logs;
pub mod gauges;
pub mod http_endpoint;
pub mod in_process;
pub mod introspection;
pub mod lambda;
pub mod mapping_template;
pub mod merged_api;
pub mod metrics;
pub mod open_id_connect;
pub mod query;
pub mod realtime;
pub mod relational_database;
pub mod role_session;
pub mod subscription_filters;
pub mod subscriptions;
pub mod user_pool;

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
    /// The Region the API was created in, which is where what it logs about a request is
    /// written ([`field_logs`]).
    pub region: String,
    /// The in-process transport into this same emulator — the handle the operation that
    /// created the API was given (`OperationContext::callback`) — kept because a request on
    /// this plane has no context of its own and what the API LOGS about one is written into
    /// this emulator's CloudWatch Logs ([`field_logs`]).
    pub callback: EmbeddedEmulator,
    /// WHO may drive this API's GraphQL endpoint from a page a BROWSER serves, as the operator
    /// asked for it ([`query::REMOTE_ORIGINS_ENV`]): resolved ONCE here, when the sockets are
    /// opened, so no request-path code reads the environment. A page served from this machine
    /// unless the operator named other sites, which is announced where they do.
    pub admitted_origins: Arc<query::AdmittedOrigins>,
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

impl PlaneContext {
    /// The counts the two gauges this plane reports read at this moment: the real-time
    /// connections open on the API and the subscriptions they hold, as the store holds them.
    pub fn connection_counts(&self) -> metrics::ConnectionCounts {
        let store = self.state.lock();
        metrics::ConnectionCounts {
            connections: store.graphql_data_plane.active_connections(&self.api_id),
            subscriptions: store.graphql_data_plane.active_subscriptions(&self.api_id),
        }
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
    region: &str,
    callback: EmbeddedEmulator,
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
        region: region.to_string(),
        callback,
        admitted_origins: Arc::new(query::configured_admitted_origins()),
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
        .tcp_plaintext(realtime::RealtimeSocket::new(context.clone(), upgrader))
        .await
        .map_err(|error| format!("the API's real-time endpoint could not be opened: {error}"))?;
    let authorities = state.lock().graphql_data_plane.remember_listeners(
        api_id,
        context,
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
    let configured = |auth_type: &str| configures(api, auth_type);
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

/// What the TOKEN modes of an API decided about one value presented in a call's own
/// `authorization` field.
///
/// The modes that decide a call by a token each read the same field, so the decision is taken
/// once, HERE, and both of the API's endpoints read it through this one seam: a call on the
/// `GRAPHQL` endpoint ([`query`]) and a connection or a `start` on the `REALTIME` one
/// ([`realtime`]) cannot differ about what a token authorizes.
#[derive(Debug, Clone)]
pub enum TokenDecision {
    /// No token mode of this API decides the value presented: the API configured none, the
    /// call presented nothing in that field, or what it presented is another provider's
    /// material.
    NotApplicable,
    /// The value authorizes nobody, and the call reaches no field of the API.
    Refused(user_pool::Refusal),
    /// The call is served, under what the token says about the caller.
    Admitted {
        /// What a resolver reads as `$context.identity` for this caller.
        identity: ::serde_json::Value,
        /// The groups an admitted Cognito user pool token CLAIMS, which the group guards a
        /// schema writes are judged against ([`user_pool::refused_fields`]) — `None` for a
        /// mode whose tokens carry no such claim, which states no group requirement to judge.
        groups: Option<Vec<String>>,
    },
}

/// Decide `presented` — the value a call carries in its own `authorization` field — under the
/// token modes the API `api_id` is served for, at `now`, for a call that came from `source_ip`.
///
/// The modes are read in the order they claim material: the Cognito user pool mode first
/// ([`user_pool::decided`]), which defers a value it cannot place to whichever other token
/// mode the API set up, and the OpenID Connect mode for what is left
/// ([`open_id_connect::decided`]). A mode the API did not set up decides nothing, so an API
/// that set up neither leaves every value to the plane's own authorization rule
/// ([`authorized`]).
pub fn token_decision(
    context: &PlaneContext,
    presented: &str,
    source_ip: Option<String>,
    now: ::std::time::SystemTime,
) -> TokenDecision {
    match user_pool::decided(context, presented, source_ip, now) {
        user_pool::Decided::Refused(refused) => TokenDecision::Refused(refused),
        user_pool::Decided::Admitted(admitted) => TokenDecision::Admitted {
            identity: admitted.identity,
            groups: Some(admitted.groups),
        },
        user_pool::Decided::NotApplicable => {
            match open_id_connect::decided(context, presented, now) {
                open_id_connect::Decided::Refused(refused) => TokenDecision::Refused(refused),
                open_id_connect::Decided::Admitted(admitted) => TokenDecision::Admitted {
                    identity: admitted.identity,
                    groups: None,
                },
                open_id_connect::Decided::NotApplicable => TokenDecision::NotApplicable,
            }
        }
    }
}

/// The authorization modes that decide a call by the value its `authorization` field carries,
/// each named by the module that decides it.
///
/// What they share: one field of the request is their input, so a value one of them cannot
/// place is another's to judge ([`another_token_mode_configured`]), and each judges the CLIENT
/// a token names against an expression its own configuration states
/// ([`client_unmatched`]).
pub const TOKEN_MODES: [&str; 3] = [
    user_pool::USER_POOL_AUTH_TYPE,
    open_id_connect::OPEN_ID_CONNECT_AUTH_TYPE,
    authorizer::LAMBDA_AUTH_TYPE,
];

/// Whether the API `context` serves set up a token mode BESIDES `besides`.
///
/// What it decides: whether a value the mode `besides` cannot place is REFUSED by it or left to
/// another mode that reads the same field. An API that set up no other token mode has no second
/// reader for such a value, so it authorizes nobody there; an API that did set one up leaves
/// the judgement to it. The ONE reading of that question, so no two token modes can differ
/// about which of them a value is left to.
pub fn another_token_mode_configured(context: &PlaneContext, besides: &str) -> bool {
    let store = context.state.lock();
    store
        .graphql_apis
        .get(&context.api_id)
        .is_some_and(|api| {
            TOKEN_MODES
                .iter()
                .any(|mode| *mode != besides && configures(api, mode))
        })
}

/// Why the client `token` names does not satisfy the expression `configured`, which the API's
/// configuration states in `member`, or `None` when it does — including when the configuration
/// states none, where no filtering is applied.
///
/// The ONE reading of that check, which every token mode of this plane states one member for: a
/// Cognito user pool configuration's `appIdClientRegex` ("a regular expression for validating
/// the incoming Amazon Cognito user pool app client ID. If this value isn't set, no filtering is
/// applied") and an OpenID Connect configuration's `clientId` ("you can specify a regular
/// expression so that AppSync can validate against multiple client identifiers at a time").
///
/// The clients read are the shared ones: an identity token names its client in `aud`, an access
/// token in `client_id`, and a match under either satisfies the configuration
/// ([`::service_common::jwt::CompactJws::audiences`]). The expression comes from a stored
/// configuration rather than from this crate, so it is compiled under the explicit program
/// budget this crate compiles every caller-supplied pattern under — and one past that budget
/// REFUSES every token, because a filter this emulator could not evaluate has not admitted
/// anything: treating it as "no filtering" would be a success reported for a check nobody
/// performed. `configured_for` names, for this emulator's own log, the configuration the
/// expression came from.
pub fn client_unmatched(
    configured: Option<&str>,
    token: &::service_common::jwt::CompactJws,
    member: &str,
    configured_for: &str,
) -> Option<String> {
    let expression = configured
        .map(str::trim)
        .filter(|expression| !expression.is_empty())?;
    let compiled = ::regex::RegexBuilder::new(expression)
        .size_limit(mapping_template::utilities::MAX_PATTERN_PROGRAM_BYTES)
        .dfa_size_limit(mapping_template::utilities::MAX_PATTERN_DFA_BYTES)
        .build();
    let pattern = match compiled {
        Ok(pattern) => pattern,
        Err(_) => {
            ::tracing::warn!(
                %configured_for,
                %member,
                "this API's client expression is not a regular expression this emulator \
                 evaluates within its compiled-size budget, so no token satisfies it"
            );
            return Some(format!(
                "this API's {member} is not an expression this emulator evaluates, so the \
                 presented token's client could not be validated against it"
            ));
        }
    };
    if token
        .audiences()
        .iter()
        .any(|audience| pattern.is_match(audience))
    {
        return None;
    }
    Some(format!(
        "the presented token's client does not match this API's {member}"
    ))
}

/// Whether `api` SET UP the authorization type `auth_type`: either "The authentication
/// type" every call on the API may use, or one of its `additionalAuthenticationProviders`
/// ("a list of additional authentication providers for the GraphqlApi API").
///
/// The ONE reading of that question, so the material a call presents ([`authorized`]) and
/// the decision a mode takes about the call afterwards (`query`'s IAM refusal) cannot differ
/// about which modes the API configured.
pub fn configures(api: &crate::state::GraphqlApiRecord, auth_type: &str) -> bool {
    api.authentication_type == auth_type
        || api
            .additional_authentication_providers
            .iter()
            .any(|provider| provider_type(provider) == Some(auth_type))
}

/// The authentication type one additional provider sets up, or `None` for a provider that
/// names none.
fn provider_type(provider: &AdditionalAuthenticationProvider) -> Option<&str> {
    provider.authentication_type.as_deref()
}

/// The authorization TYPE an API key belongs to, as `AuthenticationType` spells it.
const API_KEY_AUTH_TYPE: &str = "API_KEY";
