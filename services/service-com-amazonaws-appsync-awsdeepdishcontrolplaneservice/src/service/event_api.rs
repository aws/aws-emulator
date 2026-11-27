// SPDX-License-Identifier: CC0-1.0

//! The Event API's DATA plane: what ANSWERS at the two endpoints `CreateApi` publishes in
//! `Api$dns`.
//!
//! An Event API "provides real-time message publishing and message subscriptions over
//! WebSockets" (`Api`), and `Api$dns` "will include an HTTP and a real-time endpoint". Those
//! two values are not labels: a publisher POSTs an event document to the HTTP one and a
//! subscriber completes an RFC 6455 opening handshake against the real-time one and then
//! exchanges frames over it. So each is a SOCKET this emulator answers on:
//!
//! * [`publish`] — the HTTP endpoint, which serves `POST /event`: it resolves the channel's
//!   channel namespace, runs that namespace's `onPublish` handler over the events the
//!   request carried ([`handlers`]), delivers what the handler returned to every matching
//!   subscription, and answers which events it accepted and which the handler refused;
//! * [`realtime`] — the real-time endpoint, which serves the WebSocket at
//!   `/event/realtime`: `connection_init`/`connection_ack`, `subscribe`/`unsubscribe`, the
//!   `publish` a client makes over its own socket, and the `data` frames an event is
//!   delivered to a subscriber as.
//!
//! Both are opened when the API is created ([`open_for_api`], called by `CreateApi`) and
//! closed when it is deleted, and every answer either gives is computed from this service's
//! own records — the API, its authorization configuration, its API keys, its channel
//! namespaces and their handler code — read under the one shared store lock
//! ([`crate::state::event_data_plane`]).
//!
//! # Why the plane is a socket of its own, and what it is announced as
//!
//! An Event API cannot be served through the emulator's front door: that door serves ONE
//! buffered request per call, and the seams a service is given (`Service::can_handle`,
//! `Service::override_operation`, `Service::override_routing_error`) take an
//! already-buffered request and return a buffered response, so nothing there can switch a
//! connection to another protocol and carry frames over it. What a WebSocket endpoint needs
//! is a socket, which is what the core lends a service for a resource with a native
//! protocol: `engine.listeners().tcp_plaintext(handler)` binds it on the operator-chosen
//! interface (loopback by default), assigns the port, caps the connections and severs them
//! when the handle is dropped, and hands this module each accepted connection's byte
//! stream.
//!
//! Each socket is handed over PLAINTEXT and the first byte says what the client opened
//! with ([`negotiated`]): a TLS record is handed to the core's own TLS server
//! ([`::emulator_server::TlsUpgrader`]) and everything after it is read decrypted, while an
//! HTTP method is read as it stands. So ONE announced address serves both the `https://`
//! and `wss://` a client of AWS dials and the `http://`/`ws://` a client that does not
//! verify a self-signed certificate dials, and this service never touches a key or a
//! certificate.
//!
//! The addresses the two sockets land on are what the API publishes as its `dns` records
//! ([`crate::state::event_api_dns`]), because the whole purpose of those values is that a
//! client dials them and no client on this machine resolves an AWS hostname.

use ::std::sync::Arc;

use ::emulator_server::{ListenerConn, TlsUpgrader};

use crate::ops::types::{AuthMode, EventConfig};
use crate::state::ComAmazonawsAppsyncAwsdeepdishcontrolplaneserviceState as ServiceStore;

pub mod gauges;
pub mod handlers;
pub mod metrics;
pub mod publish;
pub mod realtime;

/// The largest message either endpoint reads from a client, in bytes, and so the largest
/// an event's own bytes can be.
///
/// It bounds BOTH endpoints: the publish route reads no more than this of a request body,
/// and the real-time socket reads no more than this of a message. That matters because a
/// published event is copied once per matching subscription, so this is the factor that —
/// with the per-connection frame depth, the connections per API and the subscriptions per
/// connection a plane holds (`crate::state::event_data_plane`) — bounds the BYTES one
/// publish can put in flight, not just their number.
pub const MAX_MESSAGE_BYTES: usize = 131_072;

/// Everything one of this plane's sockets needs to answer from: the service's own store,
/// the API the socket belongs to (the socket itself says which, so no host has to name it),
/// and where that API's metrics go.
#[derive(Clone)]
pub struct PlaneContext {
    /// The shared store, so every connection reads the records the control plane writes.
    pub state: ::engine::ServiceState<ServiceStore>,
    /// The `apiId` this socket serves.
    pub api_id: String,
    /// The publisher of this API's `AWS/AppSync` series.
    pub metrics: metrics::EventApiMetrics,
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
    /// The counts the two gauges read at this moment.
    pub fn connection_counts(&self) -> metrics::ConnectionCounts {
        let store = self.state.lock();
        metrics::ConnectionCounts {
            connections: store.event_data_plane.active_connections(&self.api_id),
            subscriptions: store.event_data_plane.active_subscriptions(&self.api_id),
        }
    }
}

/// Open the two sockets `api_id`'s data plane is served on and remember them, answering the
/// `host:port` each landed on — the authorities that API's `dns` records publish (the HTTP
/// endpoint first, then the real-time one).
///
/// The core owns the sockets, the interface, the ports and the connection caps; this hands
/// each one its handler and stores the returned handles in the service's own state, which
/// is what keeps the ports open for as long as the API exists. `Err` when this service was
/// mounted on an engine with no listener surface, or when the core could not bind: the
/// caller then answers that rather than publishing an address nothing serves.
pub async fn open_for_api<Eng: Send + Sync + 'static>(
    engine: &Eng,
    api_id: &str,
) -> Result<(String, String), String> {
    let shared = (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .ok_or_else(|| "this emulator's engine has no listener surface".to_string())?;
    let state = crate::state::service_state(engine)
        .ok_or_else(|| "this emulator's engine does not carry the appsync store".to_string())?;
    let listeners = shared.listeners();
    let context = PlaneContext {
        state: state.clone(),
        api_id: api_id.to_string(),
        metrics: metrics::EventApiMetrics::new(metrics::cloudwatch_state(engine), api_id),
    };
    // The core's own TLS server, for a client that dialled `https://` or `wss://`. A core
    // that cannot prepare it leaves this `None`, and such a client is then dropped at the
    // first byte rather than answered in the clear.
    let upgrader = listeners.tls_upgrader().ok();
    let publish_listener = listeners
        .tcp_plaintext(publish::PublishSocket::new(
            context.clone(),
            upgrader.clone(),
        ))
        .await
        .map_err(|error| format!("the API's HTTP endpoint could not be opened: {error}"))?;
    let realtime_listener = listeners
        .tcp_plaintext(realtime::RealtimeSocket::new(context, upgrader))
        .await
        .map_err(|error| format!("the API's real-time endpoint could not be opened: {error}"))?;
    let authorities = state.lock().event_data_plane.remember_listeners(
        api_id,
        Arc::new(publish_listener),
        Arc::new(realtime_listener),
    );
    ::tracing::info!(
        %api_id,
        http = %authorities.0,
        realtime = %authorities.1,
        "opened an Event API's data-plane sockets"
    );
    Ok(authorities)
}

/// The connection with encryption negotiated — handed to the core's TLS server when the
/// client opened with a TLS record, and read as it stands otherwise — through the tree's
/// ONE reading of that negotiation (`service_common::in_band_tls`), so the sniff and the
/// replay of the first byte are not written again here.
pub async fn negotiated(
    conn: ListenerConn,
    upgrader: Option<TlsUpgrader>,
) -> Option<ListenerConn> {
    ::service_common::in_band_tls::negotiated(conn, upgrader).await
}

/// The authorization material a request to either endpoint presented.
///
/// AppSync authorizes a connection, a publish and a subscribe by the `authProviders` the
/// API configured (`EventConfig`), and the material travels in the request's own fields: an
/// `x-api-key` for the `API_KEY` provider, an `Authorization` field for the others. A
/// WebSocket client cannot set fields on an opening handshake, so it carries the same
/// object base64url-encoded in a subprotocol token instead
/// ([`Self::from_subprotocols`]).
#[derive(Debug, Default, Clone)]
pub struct PresentedAuth {
    /// The API key presented, when one was.
    pub api_key: Option<String>,
    /// The `Authorization` field presented, when one was.
    pub authorization: Option<String>,
}

/// The field an API key is presented in.
pub const API_KEY_FIELD: &str = "x-api-key";

/// The field every other authorization material is presented in.
const AUTHORIZATION_FIELD: &str = "authorization";

/// The subprotocol token the real-time endpoint carries its authorization object in:
/// `header-<base64url of the authorization object>`.
const AUTH_SUBPROTOCOL_PREFIX: &str = "header-";

impl PresentedAuth {
    /// The material a request's own fields carry.
    pub fn from_fields(headers: &::http::HeaderMap) -> Self {
        let field = |name: &str| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
                .filter(|value| !value.is_empty())
        };
        Self {
            api_key: field(API_KEY_FIELD),
            authorization: field(AUTHORIZATION_FIELD),
        }
    }

    /// The material an opening handshake's subprotocol list carries: the
    /// `header-<base64url>` token's decoded object, read case-insensitively by field name
    /// because that object is written with the field names a request would carry.
    ///
    /// Both base64url spellings are accepted — padded and unpadded — because the encoding
    /// is the client's choice and both decode to the same object.
    pub fn from_subprotocols(subprotocols: &str) -> Self {
        let encoded = subprotocols
            .split(',')
            .map(str::trim)
            .find_map(|token| token.strip_prefix(AUTH_SUBPROTOCOL_PREFIX));
        let Some(encoded) = encoded else {
            return Self::default();
        };
        Self::from_base64url_object(encoded)
    }

    /// The material a base64url-encoded authorization OBJECT carries — the form a real-time
    /// client that cannot set a request's fields encodes it in, whether it travels in a
    /// subprotocol token ([`Self::from_subprotocols`]) or, as a GraphQL API's real-time
    /// client writes it, in the handshake's `header` query parameter
    /// ([`crate::service::graphql_api::realtime`]).
    ///
    /// Both base64url spellings are accepted — padded and unpadded — because the encoding
    /// is the client's choice and both decode to the same object. A value that is not
    /// base64url, or does not decode to an object, carries no material at all.
    pub fn from_base64url_object(encoded: &str) -> Self {
        let Some(decoded) = decoded_base64url(encoded) else {
            return Self::default();
        };
        let Ok(object) = ::serde_json::from_slice::<::serde_json::Value>(&decoded) else {
            return Self::default();
        };
        Self::from_object(&object)
    }

    /// The material an authorization OBJECT carries — the object a handshake encodes, and
    /// the one a `subscribe` or `publish` message carries for itself.
    ///
    /// The field names are read case-insensitively, because the object is written with the
    /// field names a request would carry and a field name is case-insensitive.
    pub fn from_object(object: &::serde_json::Value) -> Self {
        let member = |name: &str| {
            object
                .as_object()?
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .and_then(|(_, value)| value.as_str())
                .map(str::to_string)
                .filter(|value| !value.is_empty())
        };
        Self {
            api_key: member(API_KEY_FIELD),
            authorization: member(AUTHORIZATION_FIELD),
        }
    }

    /// Whether any material was presented at all.
    pub fn is_present(&self) -> bool {
        self.api_key.is_some() || self.authorization.is_some()
    }
}

/// `encoded` decoded, in either base64url spelling.
fn decoded_base64url(encoded: &str) -> Option<Vec<u8>> {
    use ::base64::Engine as _;
    ::base64::engine::general_purpose::URL_SAFE
        .decode(encoded)
        .or_else(|_| ::base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(encoded))
        .ok()
}

/// Which call of the data plane is being authorized, which is what decides WHICH of the
/// API's configured authorization modes apply to it.
///
/// `EventConfig` declares one list per call: "a list of valid authorization modes for the
/// Event API connections", "… for the Event API publishing" and "… for the Event API
/// subscriptions"; a channel namespace may override the publishing and the subscription
/// lists for its own channels ("This configuration overrides the default `Api`
/// authorization configuration", `ChannelNamespace$publishAuthModes` /
/// `$subscribeAuthModes`), which is why the two namespace-scoped calls carry that
/// override.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaneCall {
    /// Opening a real-time connection to the API.
    Connect,
    /// Publishing to a channel of one of its namespaces.
    Publish,
    /// Subscribing to a channel of one of its namespaces.
    Subscribe,
}

/// The authorization TYPE one piece of presented material belongs to: the `API_KEY`
/// provider for an API key, and one of the API's other configured providers for anything
/// else.
const API_KEY_AUTH_TYPE: &str = "API_KEY";

/// The sentence AppSync answers an unauthorized call with, and the error type it carries.
pub const UNAUTHORIZED_ERROR_TYPE: &str = "UnauthorizedException";
pub const UNAUTHORIZED_MESSAGE: &str = "You are not authorized to make this call.";

/// What either endpoint answers a request whose `Host` is not the address it serves, and
/// one that names no host at all.
pub const HOST_NOT_ALLOWED: &str = "This endpoint does not serve that host.";
pub const HOST_REQUIRED: &str = "This endpoint requires the host it is addressed by.";

/// Whether a request's `Host` names the interface this plane's sockets were bound on, by
/// the rule a core HTTP listener applies: a loopback `Host` on the default loopback bind —
/// which is what guards a BROWSER against DNS rebinding, since a rebound name pointing at
/// 127.0.0.1 would otherwise be same-origin to it — the operator's own address when they
/// bound a routable one, and any `Host` on a wildcard bind, where the operator exposed the
/// port deliberately and no single address names it.
///
/// Both endpoints apply it: the core runs this guard for its own `HttpHandler` listeners
/// only, so a plane served on a `tcp_plaintext` socket owns it (`vendor/server`'s
/// `serve_http` against `serve_native`), and an RFC 6455 handshake needs it as much as a
/// publish does — more, since a page can make one with no CORS preflight to stop it.
pub fn host_allowed(host: &str) -> bool {
    let host = ::service_common::data_plane_addressing::host_only(host);
    if ::emulator_server::is_loopback_host(host) {
        return true;
    }
    let Ok(bound) = ::emulator_server::bind_address_from_env() else {
        return false;
    };
    bound.is_unspecified()
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<::std::net::IpAddr>()
            .is_ok_and(|named| named == bound)
}

/// Why a request a BROWSER made from ANOTHER SITE is refused, or `None` for every other
/// client.
///
/// An `Origin` naming an address of THIS MACHINE ([`served_from_this_machine`]) names no
/// other site: these sockets are bound on this machine, so that origin is the one the
/// endpoint itself is addressed by, and both the client that states it about the address it
/// dialled and the page served from this machine are served — which is what an AppSync API's
/// data plane exists to be called from.
///
/// Every other origin is read by the CORE'S OWN reading of the browser-set fields
/// (`emulator_server::classify_browser_request`, which the core applies to every request on
/// one of its HTTP listeners) rather than by a second rule written here: a request carrying
/// no `Origin` and no `Sec-Fetch-Site` is not a browser's and is served, a same-origin one
/// is served, and a cross-site one is refused. The policy is the default one — no origin of
/// another site reaches a data plane — because nothing here is an operator knob.
pub fn cross_site_browser(origin: Option<&str>, sec_fetch_site: Option<&str>) -> Option<String> {
    if origin.is_some_and(served_from_this_machine) {
        return None;
    }
    match ::emulator_server::classify_browser_request(
        origin,
        sec_fetch_site,
        &::emulator_server::RemoteOrigins::SameOriginOnly,
        false,
    ) {
        ::emulator_server::BrowserVerdict::Allow => None,
        ::emulator_server::BrowserVerdict::Forbid(reason) => Some(reason.to_string()),
    }
}

/// Whether `origin` names an address of THIS MACHINE: an origin whose host is a loopback
/// one, by the core's own reading of that question (`emulator_server::is_loopback_host`,
/// which is also what the `Host` allowlist is decided by ([`host_allowed`])).
///
/// It is the origin an endpoint of this plane is ADDRESSED by — every such socket is bound
/// on this machine — so it is what a client states about the very address it dialled, and
/// what a browser sets for a page served from this machine: the development server on a
/// loopback port, the console opened against it. An origin of any OTHER site is not, and is
/// refused before anything of the API is reached ([`cross_site_browser`]).
pub fn served_from_this_machine(origin: &str) -> bool {
    let authority = origin
        .split_once("://")
        .map_or(origin, |(_scheme, authority)| authority);
    ::emulator_server::is_loopback_host(::service_common::data_plane_addressing::host_only(
        authority,
    ))
}

/// Whether the material presented authorizes `call` on `api_id`, by the ONE rule this plane
/// applies to a connection, a publish and a subscribe alike:
///
/// * material has to be PRESENTED: AppSync admits no anonymous connection or publish, so a
///   request carrying none at all is unauthorized whatever the API configured;
/// * the material's provider has to be one the API set up FOR THIS CALL — the authorization
///   modes `EventConfig` declares for connecting, publishing or subscribing, with the
///   channel namespace's own override where it carries one ([`authorized_modes`]). So an
///   API configured for `API_KEY` alone refuses a token, and one configured without
///   `API_KEY` refuses a key;
/// * an API KEY has to name a key of this API that has not expired. This is the one
///   provider this service can decide itself, because it is the service that MINTED the key
///   (`CreateApiKey`), and an expired key authorizes nothing — "API keys are deleted
///   automatically 60 days after they expire";
/// * any other material is the material of a provider this service does not hold the secret
///   of — a signature, a Cognito or OIDC token, a Lambda authorizer's answer. This emulator
///   verifies request signatures at its front door and a service never checks one itself,
///   and these sockets are not that door, so such material is taken as PRESENTED once its
///   provider is one the API configured for the call.
///
/// `false` is what every endpoint of this plane answers its own unauthorized refusal for.
pub fn authorized(
    store: &ServiceStore,
    api_id: &str,
    call: PlaneCall,
    namespace: Option<&str>,
    auth: &PresentedAuth,
) -> bool {
    if !auth.is_present() {
        return false;
    }
    let Some(api) = store.event_apis.get(api_id) else {
        return false;
    };
    let namespace_record = namespace.and_then(|name| api.channel_namespaces.get(name));
    let modes = authorized_modes(
        &api.event_config,
        call,
        namespace_record.map(|record| match call {
            PlaneCall::Publish => record.publish_auth_modes.as_slice(),
            _ => record.subscribe_auth_modes.as_slice(),
        }),
    );
    let configured = |auth_type: &str| modes.iter().any(|mode| mode.auth_type == auth_type);
    if let Some(presented) = auth.api_key.as_deref() {
        return configured(API_KEY_AUTH_TYPE)
            && store
                .api_keys_of(api_id)
                .and_then(|keys| keys.get(presented))
                .is_some_and(|key| key.expires > crate::state::unix_seconds());
    }
    // Material of every provider but `API_KEY`: authorized where the API configured any of
    // those providers for this call.
    modes.iter().any(|mode| mode.auth_type != API_KEY_AUTH_TYPE)
}

/// The authorization modes that apply to `call`: the namespace's own list when it carries
/// one, and the API's default for that call otherwise.
///
/// An EMPTY override is no override — "This configuration overrides the default `Api`
/// authorization configuration" is about a namespace that states modes, and a namespace
/// that states none leaves the API's defaults in place, which is also how the create
/// projects it (`crate::state::ChannelNamespaceRecord::to_channel_namespace` reports no
/// override for an empty list).
fn authorized_modes<'a>(
    event_config: &'a EventConfig,
    call: PlaneCall,
    namespace_override: Option<&'a [AuthMode]>,
) -> &'a [AuthMode] {
    if let Some(modes) = namespace_override.filter(|modes| !modes.is_empty()) {
        return modes;
    }
    match call {
        PlaneCall::Connect => &event_config.connection_auth_modes,
        PlaneCall::Publish => &event_config.default_publish_auth_modes,
        PlaneCall::Subscribe => &event_config.default_subscribe_auth_modes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::base64::Engine as _;

    /// The authorization object a real-time client encodes, as that client encodes it.
    fn subprotocols(object: &::serde_json::Value, padded: bool) -> String {
        let json = object.to_string();
        let encoded = if padded {
            ::base64::engine::general_purpose::URL_SAFE.encode(json)
        } else {
            ::base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)
        };
        format!("{}, {AUTH_SUBPROTOCOL_PREFIX}{encoded}", realtime::SUBPROTOCOL)
    }

    #[test]
    fn a_handshakes_subprotocols_carry_the_material_its_object_holds() {
        // The rows span what the reading generalises over: an API key, an `Authorization`
        // field, the field names spelled in another case, both spellings of base64url, a
        // token that is not base64 at all, and a handshake offering no auth token.
        let cases: &[(&str, String, Option<&str>, Option<&str>)] = &[
            (
                "an api key, unpadded",
                subprotocols(
                    &::serde_json::json!({"host": "h", "x-api-key": "da2-key"}),
                    false,
                ),
                Some("da2-key"),
                None,
            ),
            (
                "an api key, padded",
                subprotocols(
                    &::serde_json::json!({"host": "h", "x-api-key": "da2-key"}),
                    true,
                ),
                Some("da2-key"),
                None,
            ),
            (
                "the field names in another case",
                subprotocols(&::serde_json::json!({"X-Api-Key": "da2-key"}), false),
                Some("da2-key"),
                None,
            ),
            (
                "an Authorization field",
                subprotocols(&::serde_json::json!({"Authorization": "AWS4-HMAC-SHA256 x"}), false),
                None,
                Some("AWS4-HMAC-SHA256 x"),
            ),
            (
                "a token that is not base64",
                format!("{}, header-not!base64", realtime::SUBPROTOCOL),
                None,
                None,
            ),
            (
                "no auth token at all",
                realtime::SUBPROTOCOL.to_string(),
                None,
                None,
            ),
        ];
        for (case, offered, api_key, authorization) in cases {
            let read = PresentedAuth::from_subprotocols(offered);
            assert_eq!(
                read.api_key.as_deref(),
                *api_key,
                "case {case:?}: expected the api key {api_key:?}, got {read:?}"
            );
            assert_eq!(
                read.authorization.as_deref(),
                *authorization,
                "case {case:?}: expected the authorization {authorization:?}, got {read:?}"
            );
        }
    }
}
