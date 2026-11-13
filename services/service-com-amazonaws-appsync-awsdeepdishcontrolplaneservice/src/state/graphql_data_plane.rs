// SPDX-License-Identifier: CC0-1.0

//! A GraphQL API's DATA plane as STATE: the two sockets each `GraphqlApi` is served on,
//! the connections open on the real-time one, and the subscriptions those connections hold.
//!
//! Neither entry of `GraphqlApi$uris` is a label. The `GRAPHQL` one is the address a client
//! POSTs a GraphQL document to, and the `REALTIME` one the address a subscriber dials —
//! completing a WebSocket opening handshake against it and then exchanging frames — so each
//! endpoint is a SOCKET, which the core owns (`engine.listeners()`). This module holds the
//! handles those sockets' lives hang off, together with the records of what is open on
//! them, so the addresses the API publishes are ones this emulator answers on and the
//! records a request is answered from are read under the same lock as the API itself.
//!
//! It is the GraphQL family's counterpart of [`super::event_data_plane`] and keeps the same
//! three invariants — the listeners of one API, opened when the API is created and dropped
//! when it is deleted; a connection id MINTED here rather than taken from what a client
//! sent; and a subscription that belongs to the CONNECTION that asked for it, so closing a
//! connection takes its subscriptions with it.

use std::collections::BTreeMap;
use std::sync::Arc;

use ::emulator_server::ResourceListener;
use ::tokio::sync::mpsc::Sender;

use super::event_data_plane::{
    authority_of, uuid_from, SubscribeRefusal, CONNECTIONS_PER_API, SUBSCRIPTIONS_PER_CONNECTION,
};

/// The sockets one GraphQL API's endpoints are served on, and what is open on them.
struct ApiSocket {
    /// The socket the API's GraphQL endpoint (`GraphqlApi$uris`'s `GRAPHQL` entry) is
    /// served on.
    graphql: Arc<ResourceListener>,
    /// The socket the API's real-time endpoint (`GraphqlApi$uris`'s `REALTIME` entry) is
    /// served on.
    realtime: Arc<ResourceListener>,
    /// The connections open on this API, keyed by the connection id this plane minted for
    /// each.
    connections: BTreeMap<String, ConnectionRecord>,
}

/// One open real-time connection: the channel its frames are written over, and the
/// subscriptions it holds.
struct ConnectionRecord {
    /// Where a frame pushed TO this connection is queued. The connection's own task owns
    /// the receiving half, so a delivery is a real write over that socket.
    frames: Sender<String>,
    /// The subscriptions this connection holds, keyed by the subscription id the client
    /// chose, each the subscription FIELD of the API's schema it was started on.
    subscriptions: BTreeMap<String, String>,
}

/// The GraphQL API data plane's records: the sockets each API is served on, the
/// connections open on them, and the counter this plane's connection ids are minted from.
#[derive(Default)]
pub struct GraphqlDataPlane {
    /// The sockets each GraphQL API's endpoints are served on, keyed by `apiId`.
    /// Holding the handles is what keeps the ports open; removing the entry closes them.
    apis: BTreeMap<String, ApiSocket>,
    /// Drives [`Self::mint_connection_id`]: how many connections this plane has accepted.
    connection_seed: u64,
}

// By hand, because a `ResourceListener` is a live socket and carries no `Debug` of its own:
// what is diagnosable about the plane is which APIs hold a socket, where each landed and
// what is open on it, not a handle's internals.
impl std::fmt::Debug for GraphqlDataPlane {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let apis: BTreeMap<&String, (String, String, usize, usize)> = self
            .apis
            .iter()
            .map(|(api_id, socket)| {
                (
                    api_id,
                    (
                        authority_of(&socket.graphql),
                        authority_of(&socket.realtime),
                        socket.connections.len(),
                        socket
                            .connections
                            .values()
                            .map(|connection| connection.subscriptions.len())
                            .sum(),
                    ),
                )
            })
            .collect();
        formatter
            .debug_struct("GraphqlDataPlane")
            .field("apis", &apis)
            .field("connection_seed", &self.connection_seed)
            .finish()
    }
}

impl GraphqlDataPlane {
    /// Remember the sockets `api_id`'s endpoints are served on, and answer the `host:port`
    /// each landed on — the authorities that API publishes as its `GRAPHQL` and `REALTIME`
    /// endpoints, in that order.
    ///
    /// Replaces any sockets the API already held, so re-opening closes the old ports rather
    /// than leaving two sockets answering for one endpoint of one API.
    pub fn remember_listeners(
        &mut self,
        api_id: &str,
        graphql: Arc<ResourceListener>,
        realtime: Arc<ResourceListener>,
    ) -> (String, String) {
        let authorities = (authority_of(&graphql), authority_of(&realtime));
        self.apis.insert(
            api_id.to_string(),
            ApiSocket {
                graphql,
                realtime,
                connections: BTreeMap::new(),
            },
        );
        authorities
    }

    /// Close `api_id`'s sockets and forget everything open on them — what an API being
    /// deleted does to its data plane. Dropping the handles severs every connection, and
    /// the task serving each one then ends.
    pub fn forget_api(&mut self, api_id: &str) {
        self.apis.remove(api_id);
    }

    /// Mint the next connection id: a UUID, which is the shape AppSync's own real-time
    /// endpoint identifies a connection by.
    ///
    /// Minted from this plane's own counter through the service's one UUID rendering
    /// ([`uuid_from`]), so an id is never reused and nothing a client sends chooses it.
    pub fn mint_connection_id(&mut self) -> String {
        self.connection_seed = self.connection_seed.wrapping_add(1);
        uuid_from(CONNECTION_ID_MIX_DOMAIN, self.connection_seed)
    }

    /// Record that `connection_id` is open on `api_id`, with `frames` as the queue a
    /// delivery to it is written through.
    ///
    /// `false` when the API holds no socket (it was deleted) or already holds
    /// [`CONNECTIONS_PER_API`] connections, which is what refuses the handshake instead of
    /// admitting a connection this plane does not account for.
    pub fn open_connection(
        &mut self,
        api_id: &str,
        connection_id: String,
        frames: Sender<String>,
    ) -> bool {
        let Some(socket) = self.apis.get_mut(api_id) else {
            return false;
        };
        if socket.connections.len() >= CONNECTIONS_PER_API {
            return false;
        }
        socket.connections.insert(
            connection_id,
            ConnectionRecord {
                frames,
                subscriptions: BTreeMap::new(),
            },
        );
        true
    }

    /// Forget `connection_id` and answer how many subscriptions went with it: a connection
    /// that is gone holds none.
    pub fn close_connection(&mut self, api_id: &str, connection_id: &str) -> usize {
        let Some(socket) = self.apis.get_mut(api_id) else {
            return 0;
        };
        socket
            .connections
            .remove(connection_id)
            .map_or(0, |connection| connection.subscriptions.len())
    }

    /// Record that `connection_id` holds a subscription on the schema field `field` under
    /// the id the client chose.
    pub fn start_subscription(
        &mut self,
        api_id: &str,
        connection_id: &str,
        subscription_id: &str,
        field: &str,
    ) -> Result<(), SubscribeRefusal> {
        let connection = self
            .apis
            .get_mut(api_id)
            .and_then(|socket| socket.connections.get_mut(connection_id))
            .ok_or(SubscribeRefusal::NoConnection)?;
        if connection.subscriptions.contains_key(subscription_id) {
            return Err(SubscribeRefusal::DuplicateId);
        }
        if connection.subscriptions.len() >= SUBSCRIPTIONS_PER_CONNECTION {
            return Err(SubscribeRefusal::TooMany);
        }
        connection
            .subscriptions
            .insert(subscription_id.to_string(), field.to_string());
        Ok(())
    }

    /// Forget one subscription of `connection_id`, answering the field it was on — `None`
    /// is a stop for a subscription this connection does not hold.
    pub fn stop_subscription(
        &mut self,
        api_id: &str,
        connection_id: &str,
        subscription_id: &str,
    ) -> Option<String> {
        self.apis
            .get_mut(api_id)
            .and_then(|socket| socket.connections.get_mut(connection_id))
            .and_then(|connection| connection.subscriptions.remove(subscription_id))
    }

    /// Every subscription of `api_id` on the field `field`: the queue its frames go to,
    /// paired with the subscription id a `data` frame is addressed by.
    ///
    /// The queues are CLONED out, so a delivery is made with no store lock held. This is
    /// what a mutation of the API's schema publishes through.
    pub fn deliveries(&self, api_id: &str, field: &str) -> Vec<(String, Sender<String>)> {
        let Some(socket) = self.apis.get(api_id) else {
            return Vec::new();
        };
        socket
            .connections
            .values()
            .flat_map(|connection| {
                connection
                    .subscriptions
                    .iter()
                    .filter(|(_, subscribed)| subscribed.as_str() == field)
                    .map(|(id, _)| (id.clone(), connection.frames.clone()))
            })
            .collect()
    }
}

/// What a GraphQL API connection id's bytes are mixed with, so one can never coincide with
/// an id the Event API plane minted from the same counter value.
const CONNECTION_ID_MIX_DOMAIN: u64 = 0x4170_7053_796E_6303;
