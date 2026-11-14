// SPDX-License-Identifier: CC0-1.0

//! The core-owned LISTENERS this service's resources are reached on, keyed by the record each
//! belongs to.
//!
//! Two kinds of resource of this service are reached at an address of their own rather than
//! through this emulator's front door: a WEBSOCKET api, whose data plane is a socket
//! ([`super::websocket_transport`]), and a custom domain name, whose published
//! `ApiGatewayDomainName` is an address a client dials
//! ([`super::domain_name_transport`]). Both need the same three things of the handle the core
//! hands back — hold it while the record exists, report the `host:port` it landed on, and drop
//! it when the record goes, which closes the port and severs every connection on it — so the
//! registry that keeps them lives here once rather than once per resource kind.
//!
//! What stays with each resource kind is what is specific to it: which operation opens the
//! socket, what the address is announced as, and whatever else that kind's transport holds.

use std::collections::BTreeMap;
use std::sync::Arc;

use ::emulator_server::ResourceListener;

/// The `host:port` a core listener is addressed by, written as a URL parses it — the ONE
/// reading of a listener's address (`service_common::data_plane_addressing`).
pub fn authority_of(listener: &ResourceListener) -> String {
    ::service_common::data_plane_addressing::listener_authority(listener)
}

/// The listeners one kind of resource of this service is reached on, keyed by the record's own
/// key.
///
/// Holding a handle is what keeps its port open; removing the entry closes it.
#[derive(Default)]
pub struct ListenerRegistry {
    listeners: BTreeMap<String, Arc<ResourceListener>>,
}

// By hand, because a `ResourceListener` is a live socket and carries no `Debug` of its own:
// what is diagnosable about a registry is which records hold a socket and where each landed,
// not the handle's internals.
impl std::fmt::Debug for ListenerRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let listeners: BTreeMap<&String, String> = self
            .listeners
            .iter()
            .map(|(key, listener)| (key, authority_of(listener)))
            .collect();
        formatter
            .debug_struct("ListenerRegistry")
            .field("listeners", &listeners)
            .finish()
    }
}

impl ListenerRegistry {
    /// Remember the listener the record `key` is reached on, and answer the `host:port` it
    /// landed on.
    ///
    /// Replaces any listener that record already held, so re-opening one closes the old port
    /// rather than leaving two sockets answering for one record.
    pub fn remember(&mut self, key: &str, listener: Arc<ResourceListener>) -> String {
        let authority = authority_of(&listener);
        self.listeners.insert(key.to_string(), listener);
        authority
    }

    /// The `host:port` the record `key` is reached at, or `None` when it holds no listener.
    pub fn authority(&self, key: &str) -> Option<String> {
        self.listeners.get(key).map(|listener| authority_of(listener))
    }

    /// The record reached at `authority`, or `None` when that address is no record's socket —
    /// the reverse of [`Self::authority`], for a request that arrived at one of these
    /// addresses and is placed by the address it reached.
    pub fn key_at(&self, authority: &str) -> Option<&str> {
        self.listeners
            .iter()
            .find(|(_key, listener)| authority_of(listener) == authority)
            .map(|(key, _listener)| key.as_str())
    }

    /// Close the record's socket — what the record being deleted does to its address. Dropping
    /// the handle severs every connection that was open on it.
    pub fn forget(&mut self, key: &str) {
        self.listeners.remove(key);
    }
}
