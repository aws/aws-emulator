// SPDX-License-Identifier: CC0-1.0

//! The ADDRESS a Classic load balancer is reached at, and the socket that answers there.
//!
//! `CreateAccessPointOutput$DNSName` is "the DNS name of the load balancer", and
//! `DescribeLoadBalancers` reports the same name: it is the address a CLIENT is handed, and
//! a client takes that name and sends its traffic to it. A name under Elastic Load
//! Balancing's own zone resolves to nothing on the machine this emulator runs on, so a load
//! balancer that reported one published an address no client could reach — the one thing a
//! published endpoint must never be. So each load balancer gets a socket of the EMULATOR's
//! own, opened through the core (`engine.listeners().tcp_plaintext(..)`, which owns the
//! socket, the bind interface, the port range and the connection caps), and publishes the
//! `host:port` that socket answers on. A client dials exactly what it was told.
//!
//! The socket is the ADDRESS, not the data plane: what a request is answered BY — the
//! listener it reached, the registered instances it may be sent to, the stickiness policy
//! attached to that listener — stays in [`crate::service::data_plane`], which serves the
//! same request however it was addressed. What the socket itself owns is the TRANSPORT
//! ([`crate::service::listener_socket`]).
//!
//! The socket's life is the load balancer's: the handle is held on the store under the load
//! balancer's region and name, and dropping it (the delete) closes the port and severs every
//! connection in flight on it.

use ::smithy_server_runtime::EmbeddedEmulator;

/// One load balancer's own address: where it answers, and the handle that keeps it open.
///
/// Dropping it closes the port, so the record is held for exactly as long as the load
/// balancer it belongs to.
pub struct LoadBalancerEndpoint {
    /// The `host:port` a client dials, which is what the load balancer publishes as its
    /// `DNSName`.
    authority: String,
    /// The listener itself. Never read: it is held because dropping it closes the port.
    _listener: ::emulator_server::ResourceListener,
}

impl ::std::fmt::Debug for LoadBalancerEndpoint {
    /// Everything about this record that is not a live socket: the address it answers on.
    /// `ResourceListener` is not `Debug`.
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.debug_struct("LoadBalancerEndpoint")
            .field("authority", &self.authority)
            .finish_non_exhaustive()
    }
}

impl LoadBalancerEndpoint {
    /// The `host:port` a client reaches this load balancer at — its `DNSName`.
    pub fn authority(&self) -> &str {
        &self.authority
    }
}

/// Open the socket the load balancer `load_balancer_name` of `region` is served on, and
/// report the address it answers at — or `None` when this engine vends no listeners, in
/// which case the caller falls back to the name Elastic Load Balancing's own zone would
/// carry ([`super::dns_name`]).
///
/// `callback` is the handle into the emulator this request is running within
/// (`ctx.callback()`), which is how a request the socket accepts reaches the data plane: the
/// socket is not a second emulator, it is a second ADDRESS of this one.
pub async fn open_load_balancer_endpoint<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    load_balancer_name: &str,
    callback: EmbeddedEmulator,
) -> Option<LoadBalancerEndpoint> {
    let shared = (engine as &dyn ::std::any::Any).downcast_ref::<::engine::InMemoryEngine>()?;
    let listeners = shared.listeners();
    let handler = crate::service::listener_socket::ListenerSocket::new(
        region.to_string(),
        load_balancer_name.to_string(),
        callback,
        // The core's own TLS server, for a client that dials the published address with
        // `https://` — which is what a client of an HTTPS or SSL listener does. The service
        // never touches a key or a certificate of its own.
        listeners.tls_upgrader().ok(),
    );
    // Opened on a task of its OWN, so a runtime that cannot bind a socket at all — one
    // built with no IO driver, which is what a host that drives this emulator in process
    // may hand a request — leaves this create without an address of its own instead of
    // taking the request down with it: the join answers `Err`, this answers `None`, and the
    // caller falls back to the name Elastic Load Balancing's own zone would carry.
    let opened = ::tokio::spawn(async move { listeners.tcp_plaintext(handler).await.ok() }).await;
    let listener = opened.ok().flatten()?;
    Some(LoadBalancerEndpoint {
        authority: ::service_common::data_plane_addressing::listener_authority(&listener),
        _listener: listener,
    })
}
