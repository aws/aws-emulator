// SPDX-License-Identifier: CC0-1.0

//! The WEBSOCKET data plane's TRANSPORT: the core-owned socket each WebSocket api is reached
//! at, and the `ConnectionId`s its handshakes mint.
//!
//! A WebSocket api is not reached by one request and one response: a client completes the
//! RFC 6455 opening handshake against the api's own endpoint and then exchanges FRAMES over
//! that socket until either side closes it. So the api needs a socket of its own, which the
//! core owns (`engine.listeners()`), and this module holds the handle that socket's lifetime
//! hangs off.
//!
//! Two invariants it keeps, so nothing else has to re-decide them:
//!
//! * ONE listener per api, opened when the api is created and dropped when the api is —
//!   dropping the [`ResourceListener`] closes the port and severs every connection on it,
//!   which is what makes a deleted api stop answering. The address it landed on is what the
//!   api's `ApiEndpoint` announces ([`announced_endpoint`]), so the value a caller hands a
//!   WebSocket client library is an address that client can reach.
//! * a `ConnectionId` is MINTED here, once per completed handshake, and is never reused.
//!   The connection it identifies is registered in the registry the `@connections`
//!   management API serves, which is where a connection EXISTS for every reader of one
//!   (`service_com_amazonaws_apigatewaymanagementapi_apigatewaymanagementapi::state`): one
//!   registry, so a connection this data plane carries and a connection that API reports are
//!   the same connection.

use std::collections::BTreeMap;
use std::sync::Arc;

use ::emulator_server::ResourceListener;

/// The transport half of this service's state: the listener each WebSocket api is reached
/// at, and the counter its connection ids are minted from.
///
/// A field of [`crate::state::ComAmazonawsApigatewayv2Apigatewayv2State`] rather than a
/// store of its own, so the records that say which apis exist and the sockets those apis are
/// reached at are read under ONE lock and cannot drift apart.
#[derive(Default)]
pub struct WebSocketTransport {
    /// The core-owned listener each WebSocket api is reached at, keyed by `ApiId`. Holding
    /// the handle is what keeps the port open; removing the entry closes it.
    listeners: BTreeMap<String, Arc<ResourceListener>>,
    /// Drives [`Self::mint_connection_id`]: how many connections this transport has
    /// accepted, which is what makes each minted id a new one.
    connection_seed: u64,
}

// By hand, because a `ResourceListener` is a live socket and carries no `Debug` of its own:
// what is diagnosable about the transport is which apis hold a socket and where each landed,
// not the handle's internals.
impl std::fmt::Debug for WebSocketTransport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let listeners: BTreeMap<&String, String> = self
            .listeners
            .iter()
            .map(|(api_id, listener)| (api_id, authority_of(listener)))
            .collect();
        formatter
            .debug_struct("WebSocketTransport")
            .field("listeners", &listeners)
            .field("connection_seed", &self.connection_seed)
            .finish()
    }
}

/// The `host:port` a core listener is addressed by, written as a URL parses it — the ONE
/// reading of a listener's address (`service_common::data_plane_addressing`).
pub fn authority_of(listener: &ResourceListener) -> String {
    ::service_common::data_plane_addressing::listener_authority(listener)
}

/// The `ApiEndpoint` a WebSocket api announces for the socket it is served on: the WebSocket
/// scheme its `ProtocolType` gives it ([`crate::state::execute_api_scheme`]) over the address
/// the core's listener landed on.
///
/// AWS announces the api's `execute-api` host there; this emulator announces the address the
/// api's socket is REACHED at, because the value's whole purpose is that a client dials it —
/// "the stage name is typically appended to this URI to form a complete path to a deployed
/// API stage" (`Api$ApiEndpoint`) — and no client on this machine resolves the AWS host.
pub fn announced_endpoint(authority: &str) -> String {
    format!(
        "{}://{authority}",
        crate::state::execute_api_scheme(crate::state::WEBSOCKET_PROTOCOL_TYPE)
    )
}

impl WebSocketTransport {
    /// Remember the listener `api_id`'s WebSocket data plane is served on, and answer the
    /// `host:port` it landed on — the authority that api's `ApiEndpoint` announces.
    ///
    /// Replaces any listener the api already held, so re-opening one closes the old port
    /// rather than leaving two sockets answering for one api.
    pub fn remember_listener(&mut self, api_id: &str, listener: Arc<ResourceListener>) -> String {
        let authority = authority_of(&listener);
        self.listeners.insert(api_id.to_string(), listener);
        authority
    }

    /// The `host:port` `api_id`'s WebSocket data plane is reached at, or `None` when this
    /// api holds no listener.
    pub fn authority(&self, api_id: &str) -> Option<String> {
        self.listeners.get(api_id).map(|l| authority_of(l))
    }

    /// The api whose data plane is reached at `authority`, or `None` when that address is no
    /// api's socket.
    ///
    /// The reverse of [`Self::authority`], for a request that arrived at one of these
    /// addresses: the socket an api is served on IS that api's whole `execute-api` endpoint —
    /// its `ApiEndpoint` announces it and its `@connections` calls are made at it — so the
    /// address a request reached names the api it addressed, which no `execute-api` host and
    /// no path segment of a locally reached endpoint does.
    pub fn api_at(&self, authority: &str) -> Option<&str> {
        self.listeners
            .iter()
            .find(|(_api_id, listener)| authority_of(listener) == authority)
            .map(|(api_id, _listener)| api_id.as_str())
    }

    /// Close `api_id`'s socket — what an api being deleted does to its data plane. Dropping
    /// the handle severs every connection that was open on it, and the task serving each one
    /// then forgets it in the connection registry.
    pub fn forget_api(&mut self, api_id: &str) {
        self.listeners.remove(api_id);
    }

    /// Mint the next `ConnectionId`, in the shape API Gateway mints one: 16 characters of
    /// base64 with its single padding character, e.g. `L0SM9cOFyjUCIhA=`.
    ///
    /// Minted from this transport's own counter, so an id is never reused and nothing a
    /// client sends chooses it. The bytes are derived from the counter through the tree's
    /// one derived-id mixer, so consecutive ids do not spell consecutive numbers — a
    /// connection id is opaque to its holder.
    pub fn mint_connection_id(&mut self) -> String {
        use ::base64::Engine as _;
        self.connection_seed = self.connection_seed.wrapping_add(1);
        let seed = self.connection_seed.to_string();
        // Two mixings, because one carries 8 bytes and a 16-character base64 id is 11: the
        // second is domain-separated from the first by its extra part, so the halves of an
        // id are not the same 8 bytes twice.
        let mixed = format!(
            "{}{}",
            ::service_common::stable_hex_id(&[CONNECTION_ID_MIX_DOMAIN, &seed]),
            ::service_common::stable_hex_id(&[CONNECTION_ID_MIX_DOMAIN, &seed, CONNECTION_ID_MIX_DOMAIN]),
        );
        let bytes = ::hex::decode(mixed).unwrap_or_default();
        ::base64::engine::general_purpose::STANDARD
            .encode(&bytes[..CONNECTION_ID_BYTES.min(bytes.len())])
    }
}

/// How many bytes a minted `ConnectionId` carries before base64: the length the
/// `@connections` registry reads an id by
/// (`service_com_amazonaws_apigatewaymanagementapi_apigatewaymanagementapi::state::CONNECTION_ID_BYTES`,
/// 11 bytes), which is what encodes to the 16-character single-padded form API Gateway's own
/// ids have. Read from there so a minted id and an id those operations accept cannot drift
/// apart.
const CONNECTION_ID_BYTES: usize =
    ::service_com_amazonaws_apigatewaymanagementapi_apigatewaymanagementapi::state::CONNECTION_ID_BYTES;

/// What the id mixing is domain-separated by, so a connection id cannot coincide with
/// another id this service derives from the same counter.
const CONNECTION_ID_MIX_DOMAIN: &str = "apigatewayv2:websocket:connection";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minted_connection_id_has_the_shape_api_gateway_mints() {
        let mut transport = WebSocketTransport::default();
        let mut seen = Vec::new();
        // The rows span what the minting generalises over: the first id a transport mints
        // and a run of later ones, each of which has to keep the shape and be new.
        for case in 1..=8u64 {
            let minted = transport.mint_connection_id();
            assert_eq!(
                minted.chars().count(),
                16,
                "case {case}: expected a 16-character ConnectionId, got {minted:?}"
            );
            assert!(
                minted.ends_with('='),
                "case {case}: expected the single base64 padding character, got {minted:?}"
            );
            // And it is a value the `@connections` management API accepts as a connection
            // id at all: that service refuses a value of another form before it looks a
            // connection up, so an id minted here has to pass its reading.
            assert!(
                ::service_com_amazonaws_apigatewaymanagementapi_apigatewaymanagementapi::state::is_well_formed_connection_id(&minted),
                "case {case}: the management API must accept the minted id {minted:?}"
            );
            assert!(
                !seen.contains(&minted),
                "case {case}: expected a new ConnectionId, got the already-minted {minted:?}"
            );
            seen.push(minted);
        }
    }

    #[test]
    fn an_announced_endpoint_is_the_websocket_scheme_over_the_sockets_address() {
        // The rows span the addresses a core listener reports: an IPv4 literal, a bracketed
        // IPv6 one, and a name.
        let cases: &[(&str, &str)] = &[
            ("127.0.0.1:34567", "wss://127.0.0.1:34567"),
            ("[::1]:8443", "wss://[::1]:8443"),
            ("localhost:443", "wss://localhost:443"),
        ];
        for (authority, expected) in cases {
            let announced = announced_endpoint(authority);
            assert_eq!(
                announced, *expected,
                "case {authority:?}: expected {expected:?}, got {announced:?}"
            );
        }
    }
}
