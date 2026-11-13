// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi service:
//! the WebSocket **connection registry** every operation of this service reads.
//!
//! # The registry is the whole service
//!
//! `ApiGatewayManagementApi` models exactly three operations — `PostToConnection`,
//! `GetConnection` and `DeleteConnection` — and all three address one WebSocket
//! connection by its id. Two refusals come out of this state, split along the SHAPE of
//! the id the request sent:
//!
//! * a `ConnectionId` that is not of the shape API Gateway mints
//!   ([`is_well_formed_connection_id`]) identifies no connection anywhere, so it is
//!   refused as an invalid `ConnectionId` (HTTP 400 `BadRequestException`,
//!   [`invalid_connection_id`]) from the value alone — before the registry is consulted
//!   at all;
//! * a WELL-FORMED id the
//!   [`connections`](ComAmazonawsApigatewaymanagementapiApigatewaymanagementapiState::connections)
//!   registry does not hold for the endpoint the request was addressed at is
//!   `GoneException` ("The connection with the provided id no longer exists"), **so a
//!   missing `connections` entry for a well-formed id is the single source of truth for
//!   that error** — whether the id was never minted, or minted and since closed.
//!
//! # One endpoint's connections
//!
//! A connection belongs to the api whose data-plane ENDPOINT minted it, and AWS publishes
//! these three operations on that same endpoint
//! (`https://{api-id}.execute-api.{region}.amazonaws.com/{stage}/@connections/{id}`), so a
//! request reaching one api's endpoint addresses only the connections established there:
//! another api's live connection id is `GoneException` on it. [`Connection::endpoint`]
//! records where a connection was established and
//! [`endpoints`](ComAmazonawsApigatewaymanagementapiApigatewaymanagementapiState::endpoints)
//! which addresses are such endpoints; a request that arrived at an address that is no
//! api's endpoint addresses the registry as a whole.
//!
//! # The STAGE an endpoint serves
//!
//! That same endpoint carries the api's STAGE in its path, and a stage no api has deployed
//! serves nothing at all: such a request is refused `403 ForbiddenException` before any of
//! the above is read. Which stages are deployed is API Gateway V2's record, so the reading
//! is asked for through [`connections_endpoint_authority`] and
//! [`crate::service`] performs it before it dispatches a request.
//!
//! # The registration seam, and the rule it follows
//!
//! None of the three modeled operations CREATES a connection: in real AWS a
//! connection comes into being when a client completes a WebSocket handshake
//! against the API's own invoke endpoint — a data-plane event this management API
//! never serves. This emulator therefore registers a connection through exactly one
//! seam, [`register_connection`], which an emulator-internal caller (a test, or a
//! future WebSocket data plane for the API that owns the connection) calls on the
//! shared engine.
//!
//! The rule, stated once so no handler has to re-decide it:
//!
//! * A connection exists **only** after [`register_connection`] registered it, and
//!   stops existing the moment `DeleteConnection` removes it. There is deliberately
//!   NO auto-registration on first reference: auto-registration would make
//!   `GetConnection` answer 200 for an id that never existed, which is the one
//!   answer the real service never gives. A well-formed id no connection owns is
//!   `GoneException`, which is the answer the real service does give.
//! * `ConnectedAt` is fixed when the connection is registered and never moves
//!   again — re-registering an id that is already present changes nothing.
//! * `LastActiveAt` moves only when data actually flows to the connection, i.e. on a
//!   successful `PostToConnection`. `GetConnection` reports the registry and must
//!   not alter what a later caller sees, so it never bumps it.

use std::collections::{BTreeSet, HashMap};
use std::time::SystemTime;

use ::base64::Engine as _;
use ::tokio::sync::mpsc::{error::TrySendError, Sender};

pub use crate::ops::types::Identity;

/// How many bytes a `ConnectionId` carries before base64: 11, which is what encodes to the
/// 16-character single-padded token API Gateway's `@connections` endpoints mint and address
/// a connection by.
///
/// The model declares no syntax for the member — `ConnectionId` targets `__string` with
/// `@httpLabel` and `@required`, no `@pattern` and no `@length`
/// (`smithy_query(query="shape", shape="GetConnectionRequest")`) — so the shape comes from
/// what the service MINTS, and it lives here beside the registry so the ids this emulator
/// mints (`service_com_amazonaws_apigatewayv2_apigatewayv2::state::websocket_transport`)
/// and the ids these operations accept cannot drift apart.
pub const CONNECTION_ID_BYTES: usize = 11;

/// Whether `connection_id` is a connection identifier of the shape API Gateway mints:
/// standard base64, padded, decoding to exactly [`CONNECTION_ID_BYTES`] bytes.
///
/// This is the whole of what the three operations read off the VALUE: a token of that shape
/// may name a connection (and is `GoneException` when it names none), while anything else —
/// a single character, text that is not base64, an unpadded token, a 200-character one —
/// could never have been minted, so it is refused as an invalid `ConnectionId` before the
/// registry is consulted.
///
/// Decoded by the maintained base64 implementation rather than by a hand-written alphabet
/// or length rule, so padding and the alphabet are that crate's to decide.
pub fn is_well_formed_connection_id(connection_id: &str) -> bool {
    ::base64::engine::general_purpose::STANDARD
        .decode(connection_id)
        .is_ok_and(|bytes| bytes.len() == CONNECTION_ID_BYTES)
}

/// The ENDPOINT a request was addressed at: the authority its `Host` field names, or the one
/// an absolute-form request target carries.
///
/// What it is FOR: which api's connections a `@connections` request addresses is decided by
/// the endpoint it reached (see this module's docs), and the authority is what the request
/// carries that address in.
pub fn addressed_at(parts: &::http::request::Parts) -> Option<String> {
    parts
        .headers
        .get(::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .or_else(|| parts.uri.authority().map(::http::uri::Authority::as_str))
        .map(|host| host.trim().to_string())
        .filter(|host| !host.is_empty())
}

/// How many pushed payloads one LIVE connection's channel holds before a delivery to it is
/// refused.
///
/// An EMULATOR resource bound, not an AWS limit: a payload queued for a connection is bytes
/// the emulator holds on behalf of a client that is not reading them, so the depth is capped
/// and a delivery past it is refused rather than growing the process. Deep enough that a
/// caller sending a handful of payloads in a row never sees a refusal, and shallow enough
/// that the bytes one stalled connection can pin stay small (its product with the modelled
/// per-payload maximum, [`crate::ops::post_to_connection::MAX_DATA_BYTES`]).
pub const CONNECTION_PUSH_DEPTH: usize = 64;

/// One registered WebSocket connection: everything the three modeled operations
/// read or write about it.
///
/// `identity` reuses the modeled `Identity` shape (`SourceIp` / `UserAgent`) rather
/// than a second copy of those two members, so `GetConnection` returns the stored
/// record instead of rebuilding it.
#[derive(Debug, Clone)]
pub struct Connection {
    /// The `ConnectionId` (`@httpLabel`) value this record is keyed by.
    pub connection_id: String,
    /// When [`register_connection`] registered this connection. Serialized as
    /// `ConnectedAt`; never updated afterwards.
    pub connected_at: SystemTime,
    /// When data last flowed to this connection (a successful `PostToConnection`),
    /// or `connected_at` while none has. Serialized as `LastActiveAt`.
    pub last_active_at: SystemTime,
    /// The caller identity recorded for the connection at registration.
    pub identity: Identity,
    /// The most recently delivered `Data` payloads, in send order — a BOUNDED
    /// observation window, not a transcript.
    ///
    /// No modelled operation reads a connection's payloads, so this exists only for an
    /// emulator-internal caller ([`connection`]) to see what was delivered. Keeping
    /// every payload would let a caller grow shared engine state without limit (one
    /// `PostToConnection` may carry up to the modelled 131072 bytes, and the only
    /// release would be a `DeleteConnection` the same caller chooses to send), so
    /// [`Connection::push_delivered`] evicts the oldest entries until BOTH
    /// [`MAX_RETAINED_MESSAGES`] and [`MAX_RETAINED_BYTES`] hold. That caps the
    /// product of payload size and message count, which is what makes the store
    /// bounded.
    pub messages: Vec<Vec<u8>>,
    /// How many payloads `PostToConnection` has delivered to this connection in total,
    /// including ones [`Connection::messages`] has since evicted. A counter, so it
    /// stays exact without retaining bytes.
    pub delivered_count: u64,
    /// Total bytes delivered to this connection, including evicted payloads. Saturating,
    /// for the same reason.
    pub delivered_bytes: u64,
    /// The data-plane ENDPOINT this connection was established at — the `host:port` the
    /// api's own socket landed on, which is the address that api's `ApiEndpoint` announces
    /// and the one its `@connections` calls are made at.
    ///
    /// A request that reached THAT endpoint addresses this connection; one that reached
    /// another api's endpoint does not (see this module's docs). `None` for a connection
    /// registered at no endpoint, which is the emulator-internal registration seam
    /// ([`register_connection`]).
    pub endpoint: Option<String>,
    /// The LIVE socket this connection is carried on, when it is carried on one: the
    /// channel the api's own WebSocket data plane serves the connection over
    /// ([`open_connection`]). `None` for a connection registered without one, which is the
    /// emulator-internal registration seam ([`register_connection`]).
    ///
    /// What makes a `PostToConnection` a DELIVERY rather than a record: a payload handed to
    /// this channel is written to the client's socket by the task serving it. Holding the
    /// only sender here is also what closes that socket when the connection is deleted —
    /// dropping the record drops the sender, and the serving task sees its channel end.
    pub live: Option<Sender<Vec<u8>>>,
}

/// Why a payload could not be handed to a connection's live socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryRefused {
    /// The connection's channel is full: the client has stopped reading and
    /// [`CONNECTION_PUSH_DEPTH`] payloads are already waiting.
    Backlogged,
    /// The task serving the connection is gone, so the connection no longer exists.
    Closed,
}

/// How many delivered payloads one connection retains for inspection.
///
/// An EMULATOR retention bound, not an AWS limit: AWS retains nothing a caller can read
/// back, so there is no documented duration or depth to honour here. Chosen large enough
/// that a caller can see the recent traffic it sent and small enough that the retained
/// bytes per connection are bounded by `MAX_RETAINED_BYTES` below.
pub const MAX_RETAINED_MESSAGES: usize = 16;

/// Total bytes of delivered payloads one connection retains for inspection (1 MiB).
///
/// The other half of the bound: with a per-payload maximum of 131072 bytes
/// (`crate::ops::post_to_connection::MAX_DATA_BYTES`), the retained bytes per connection
/// can never exceed this, however many posts arrive. Also an emulator bound, not an AWS
/// limit.
pub const MAX_RETAINED_BYTES: usize = 1024 * 1024;

impl Connection {
    /// Hand `payload` to the task serving this connection's LIVE socket, or say why it could
    /// not be: the channel is full (the client is not reading), closed (the connection has
    /// ended), or absent (this connection is carried on no socket, so a delivery is recorded
    /// and nothing is written).
    ///
    /// Never blocks and never waits, so a caller of `PostToConnection` is not held up by a
    /// client that stopped reading.
    pub fn deliver(&self, payload: Vec<u8>) -> Result<(), DeliveryRefused> {
        let Some(live) = self.live.as_ref() else {
            return Ok(());
        };
        match live.try_send(payload) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(DeliveryRefused::Backlogged),
            Err(TrySendError::Closed(_)) => Err(DeliveryRefused::Closed),
        }
    }

    /// Record one delivered payload: count it, add its bytes, keep it in the bounded
    /// observation window, and evict the oldest retained payloads until both
    /// [`MAX_RETAINED_MESSAGES`] and [`MAX_RETAINED_BYTES`] hold again.
    ///
    /// The counters are never evicted, so what was delivered stays exactly reportable
    /// while the retained bytes stay bounded.
    pub fn push_delivered(&mut self, payload: Vec<u8>) {
        self.delivered_count = self.delivered_count.saturating_add(1);
        self.delivered_bytes = self
            .delivered_bytes
            .saturating_add(payload.len() as u64);
        self.messages.push(payload);

        let mut retained_bytes: usize = self.messages.iter().map(Vec::len).sum();
        while self.messages.len() > MAX_RETAINED_MESSAGES
            || (retained_bytes > MAX_RETAINED_BYTES && self.messages.len() > 1)
        {
            retained_bytes -= self.messages.remove(0).len();
        }
    }
}

/// Per-service state for com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsApigatewaymanagementapiApigatewaymanagementapiState {
    /// The connection registry, keyed by `ConnectionId`.
    pub connections: HashMap<String, Connection>,
    /// Every address that is a data-plane ENDPOINT of an api whose connections this
    /// registry holds — the `host:port` each WebSocket api's own socket landed on, told to
    /// this registry by the data plane that opened it ([`serve_connections_at`]).
    ///
    /// What it is FOR: a request that reached one of these addresses addresses exactly the
    /// connections established THERE ([`Connection::endpoint`]), so another api's live
    /// connection id is `GoneException` on it. An address that is in no entry here is no
    /// api's endpoint, and a request that reached it addresses the registry as a whole.
    pub endpoints: BTreeSet<String>,
}

impl ComAmazonawsApigatewaymanagementapiApigatewaymanagementapiState {
    /// The connection `connection_id` names for a request that reached the address
    /// `addressed_at` ([`addressed_at`]), or `None` when it names none there — which is
    /// `GoneException` for each of the three operations.
    pub fn connection_at(
        &self,
        connection_id: &str,
        addressed_at: Option<&str>,
    ) -> Option<&Connection> {
        let endpoint = self.endpoint_addressed(addressed_at);
        self.connections
            .get(connection_id)
            .filter(|connection| reachable_at(connection, endpoint))
    }

    /// [`Self::connection_at`] for a caller that WRITES to the connection it resolved.
    pub fn connection_at_mut(
        &mut self,
        connection_id: &str,
        addressed_at: Option<&str>,
    ) -> Option<&mut Connection> {
        let endpoint = self.endpoint_addressed(addressed_at).map(str::to_string);
        self.connections
            .get_mut(connection_id)
            .filter(|connection| reachable_at(connection, endpoint.as_deref()))
    }

    /// Forget the connection `connection_id` names at `addressed_at`, answering whether one
    /// was there to forget. The removal is what makes it gone.
    pub fn remove_connection_at(
        &mut self,
        connection_id: &str,
        addressed_at: Option<&str>,
    ) -> bool {
        if self.connection_at(connection_id, addressed_at).is_none() {
            return false;
        }
        self.connections.remove(connection_id).is_some()
    }

    /// The api ENDPOINT the address `addressed_at` is, or `None` when that address is no
    /// api's endpoint — in which case the whole registry is addressed. See
    /// [`Self::endpoints`].
    fn endpoint_addressed<'a>(&self, addressed_at: Option<&'a str>) -> Option<&'a str> {
        addressed_at.filter(|address| self.endpoints.contains(*address))
    }
}

/// Whether `connection` is addressable through the api endpoint `endpoint`: a connection is
/// reached through the endpoint it was established at, and an address that is no api's
/// endpoint (`None`) reaches every connection the registry holds.
fn reachable_at(connection: &Connection, endpoint: Option<&str>) -> bool {
    match endpoint {
        Some(endpoint) => connection.endpoint.as_deref() == Some(endpoint),
        None => true,
    }
}

/// The sentence the three `@connections` operations refuse a `ConnectionId` that is not a
/// connection identifier at all with: fixed text, then the `ConnectionId` the request sent,
/// verbatim.
///
/// One wording for all three, because the refusal is about the request's own
/// `ConnectionId` (`@httpLabel`) and not about what the operation would have done with
/// the connection.
pub fn invalid_connection_id_message(connection_id: &str) -> String {
    format!("Invalid connectionId: {connection_id}")
}

/// The refusal for a `ConnectionId` that is not of the shape this service mints, as the
/// error channel of any of the three operations.
///
/// A value that could never have been minted ([`is_well_formed_connection_id`]) is not a
/// connection identifier at all, so it is a bad request (HTTP 400 `BadRequestException`)
/// rather than a connection that is gone: HTTP 410 `GoneException` is "The connection with
/// the provided id no longer exists", which says the id could have named one. A caller
/// branches on the two differently — the 410 says "stop writing to this connection", the
/// 400 says "this request is malformed" — so a malformed id answered 410 would have the
/// caller prune a connection record over a typo.
pub fn invalid_connection_id<E>(connection_id: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::bad_request(invalid_connection_id_message(connection_id))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsApigatewaymanagementapiApigatewaymanagementapiState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsApigatewaymanagementapiApigatewaymanagementapiState>)
}

/// The SHARED cell naming who judges the `@connections` ENDPOINT a request was addressed
/// through: call it with a handler's `&*self.engine`.
///
/// These operations are served on the owning api's own invoke endpoint, so a request carries
/// that api's stage in its path, and whether a stage is DEPLOYED is API Gateway V2's record
/// — a store this service cannot read, since that service depends on this one for the
/// registry above. So the owner of stages installs its own reading into this cell
/// ([`::service_common::execute_api_connections_endpoint`]) and
/// [`crate::service`] asks it before it dispatches a request.
///
/// LOCK ORDER: this cell is released before the reading it holds is called, and nothing here
/// holds the registry while asking, so this service's store and the owner's are never held
/// together.
///
/// Answer `None` for an engine that carries no state at all: nothing judges the addressing,
/// which is how an emulator the owner of stages is not mounted in reads.
pub fn connections_endpoint_authority<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<
    ::engine::ServiceState<
        ::service_common::execute_api_connections_endpoint::ConnectionsEndpointAuthority,
    >,
> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::execute_api_connections_endpoint::ConnectionsEndpointAuthority,
            >,
        )
}

/// The answer for a request whose engine does not carry this service's registry at
/// all — [`service_state`] returned `None`, so there is no state to compute any
/// response from.
///
/// This service models only client errors (403/410/429), none of which describes "the
/// store is not there", and answering one of them would tell the caller something
/// false about its own request. It renders as HTTP 501 instead, with wording of its
/// own: it is deliberately NOT
/// [`Scaffolded::not_implemented`](crate::scaffolded::Scaffolded::not_implemented),
/// whose message is the machine-readable "this operation is still a stub" record.
pub fn registry_unavailable<E>(operation: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::NotImplemented(crate::scaffolded::NotImplemented {
        message: format!(
            "the engine serving {operation} does not carry this service's connection registry"
        ),
    })
}

/// Register a WebSocket connection, the one seam that makes a connection exist.
///
/// `connected_at` is the instant the handshake completed; it becomes the record's
/// `ConnectedAt` and its initial `LastActiveAt`. Returns `true` when the connection
/// was registered, and `false` without touching anything when `connection_id` is not of
/// the shape this service mints ([`is_well_formed_connection_id`]), when the id is already
/// registered (so `ConnectedAt` cannot be reset) or when `engine` does not carry this
/// service's state.
///
/// Registers a connection carried on NO socket and reached through no api endpoint: what a
/// `PostToConnection` to it does is recorded and nothing is written. A connection the api's
/// own WebSocket data plane is carrying one for is opened through [`open_connection`]
/// instead, which is handed the endpoint that api is reached at and the channel that makes a
/// post a real delivery.
pub fn register_connection<Eng: Send + Sync + 'static>(
    engine: &Eng,
    connection_id: impl Into<String>,
    identity: Identity,
    connected_at: SystemTime,
) -> bool {
    let Some(state) = service_state(engine) else {
        return false;
    };
    open_connection(&state, connection_id, identity, connected_at, None, None)
}

/// Remember that `authority` — a `host:port` — is the data-plane ENDPOINT of an api whose
/// connections this registry holds, so a `@connections` request that reaches it addresses
/// exactly the connections established there.
///
/// Called by the data plane that opened the socket, with the address that socket landed on.
/// See [`ComAmazonawsApigatewaymanagementapiApigatewaymanagementapiState::endpoints`].
pub fn serve_connections_at(
    state: &::engine::ServiceState<ComAmazonawsApigatewaymanagementapiApigatewaymanagementapiState>,
    authority: &str,
) {
    state.lock().endpoints.insert(authority.to_string());
}

/// Register a WebSocket connection on the state handle itself, with the api ENDPOINT it was
/// established at and the LIVE socket it is carried on when it is carried on one.
///
/// This is the seam the WEBSOCKET data plane of the api that owns the connection calls when
/// its handshake completes: it holds the api's records and this registry under the one
/// shared engine, so it hands the channel over rather than registering an id nothing can
/// deliver to. `endpoint` and `live` are `None` for the emulator-internal registration
/// ([`register_connection`]).
///
/// Returns `true` when the connection was registered, and `false` without touching anything
/// when `connection_id` is not of the shape this service mints (such an id addresses no
/// connection, so registering one would hold a record no caller could reach) or when the id
/// is already registered, so `ConnectedAt` cannot be reset.
pub fn open_connection(
    state: &::engine::ServiceState<ComAmazonawsApigatewaymanagementapiApigatewaymanagementapiState>,
    connection_id: impl Into<String>,
    identity: Identity,
    connected_at: SystemTime,
    endpoint: Option<String>,
    live: Option<Sender<Vec<u8>>>,
) -> bool {
    let connection_id = connection_id.into();
    if !is_well_formed_connection_id(&connection_id) {
        return false;
    }
    let mut guard = state.lock();
    if guard.connections.contains_key(&connection_id) {
        return false;
    }
    guard.connections.insert(
        connection_id.clone(),
        Connection {
            connection_id,
            connected_at,
            last_active_at: connected_at,
            identity,
            messages: Vec::new(),
            delivered_count: 0,
            delivered_bytes: 0,
            endpoint,
            live,
        },
    );
    true
}

/// Forget a connection that has ENDED, answering whether it was registered.
///
/// The other half of [`open_connection`], called by the data plane that was carrying the
/// connection when the socket closes: from here on the id names a connection that no longer
/// exists, which is `GoneException` for each of the three modelled operations.
pub fn close_connection(
    state: &::engine::ServiceState<ComAmazonawsApigatewaymanagementapiApigatewaymanagementapiState>,
    connection_id: &str,
) -> bool {
    state.lock().connections.remove(connection_id).is_some()
}

/// This service's registry on the shared engine, as the handle the two seams above take.
///
/// `pub` because the data plane that carries a connection reaches the registry once, when
/// its socket opens, and then holds the handle for the life of that socket.
pub fn registry<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsApigatewaymanagementapiApigatewaymanagementapiState>>
{
    service_state(engine)
}

/// The registry's record for `connection_id`, cloned, or `None` when no such
/// connection is registered.
///
/// The read side of the seam: it is how an emulator-internal caller observes what
/// `PostToConnection` delivered, since the service models no operation that returns
/// a connection's messages.
pub fn connection<Eng: Send + Sync + 'static>(
    engine: &Eng,
    connection_id: &str,
) -> Option<Connection> {
    service_state(engine)?.lock().connections.get(connection_id).cloned()
}
