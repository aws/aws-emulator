// SPDX-License-Identifier: CC0-1.0

//! The ADDRESS a load balancer is reached at, and the socket that answers there.
//!
//! `LoadBalancer$DNSName` is "the public DNS name of the load balancer", and it is the
//! address a CLIENT is handed: a client takes that name and sends its traffic to it. A name
//! under Elastic Load Balancing's own zone resolves to nothing on the machine this emulator
//! runs on, so a load balancer that reported one published an address no client could
//! reach — the one thing a published endpoint must never be. So each load balancer that
//! serves traffic of its own gets a socket of the EMULATOR's own, opened through the core
//! (`engine.listeners().tcp_plaintext(..)`, which owns the socket, the bind interface and
//! the port range), and reports the `host:port` that socket answers on as its DNS name. A
//! client dials exactly what it was told.
//!
//! Only an APPLICATION Load Balancer gets one: that is the family a client ON THIS MACHINE
//! dials directly, because its listeners answer the client's own request. A Network or
//! Gateway Load Balancer keeps the name Elastic Load Balancing's own zone would give it. A
//! Network Load Balancer's traffic is served all the same — a listener of its hands the
//! connection to what its target group names ([`crate::service::data_plane`]) — and the
//! client of an internal one sits inside this emulator, which is where that name resolves:
//! a port published for a client that cannot be its client would be worse than a name.
//!
//! The socket is the ADDRESS, not the data plane: everything a request is answered BY — the
//! listener it reached, the rules, the web ACL in front of the load balancer and the target
//! it forwards to — stays in [`crate::service::data_plane`], which serves the same request
//! however it was addressed. What the socket itself owns is the TRANSPORT: which listener a
//! connection reached, and the TLS termination an HTTPS or TLS listener performs under the
//! certificate ACM holds for it ([`crate::service::listener_socket`]).
//!
//! The socket's life is the load balancer's: the handle is held on the store under the load
//! balancer's ARN, and dropping it (the delete) closes the port and severs every connection
//! in flight on it.

use std::sync::{Arc, OnceLock};

use ::smithy_server_runtime::EmbeddedEmulator;

/// One load balancer's own address: where it answers, and the handle that keeps it open.
///
/// Dropping it closes the port, so the record is held for exactly as long as the load
/// balancer it belongs to.
pub struct LoadBalancerEndpoint {
    /// The `host:port` a client dials, which is what the load balancer reports as its
    /// `DNSName`.
    authority: String,
    /// The load balancer this socket serves, published to the handler once the create that
    /// opened the socket has minted the ARN. A connection that arrives before then is
    /// served no load balancer.
    load_balancer: Arc<OnceLock<String>>,
    /// The listener itself. Never read: it is held because dropping it closes the port.
    _listener: ::emulator_server::ResourceListener,
}

impl std::fmt::Debug for LoadBalancerEndpoint {
    /// Everything about this record that is not a live socket: the address it answers on
    /// and the load balancer it serves. `ResourceListener` is not `Debug`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadBalancerEndpoint")
            .field("authority", &self.authority)
            .field("load_balancer", &self.load_balancer.get())
            .finish()
    }
}

impl LoadBalancerEndpoint {
    /// The `host:port` a client reaches this load balancer at — its `DNSName`.
    pub fn authority(&self) -> &str {
        &self.authority
    }

    /// Bind this socket to the load balancer it was opened for, once the create that opened
    /// it has minted the ARN. Called once; a second call changes nothing.
    pub fn serves(&self, load_balancer_arn: &str) {
        let _ = self.load_balancer.set(load_balancer_arn.to_string());
    }
}

/// Open the socket one load balancer is served on, and report the address it answers at —
/// or `None` when this engine vends no listeners, in which case the caller falls back to
/// the name Elastic Load Balancing's own zone would carry.
///
/// `callback` is the handle into the emulator this request is running within
/// (`ctx.callback()`), which is how a request the socket accepts reaches the data plane: the
/// socket is not a second emulator, it is a second ADDRESS of this one.
pub async fn open_load_balancer_endpoint<Eng: Send + Sync + 'static>(
    engine: &Eng,
    callback: EmbeddedEmulator,
) -> Option<LoadBalancerEndpoint> {
    let shared = (engine as &dyn ::std::any::Any).downcast_ref::<::engine::InMemoryEngine>()?;
    let load_balancer: Arc<OnceLock<String>> = Arc::new(OnceLock::new());
    let listeners = shared.listeners();
    let handler = crate::service::listener_socket::ListenerSocket::new(
        Arc::clone(&load_balancer),
        shared.state::<super::ComAmazonawsElasticloadbalancingv2ElasticloadbalancingV10State>(),
        super::certificate_key_material_directory(engine),
        callback,
        // The core's own TLS server, for a TLS connection whose listener names no
        // certificate this emulator holds key material for. The account's own certificates
        // are served by the handler itself, which is the only way the certificate ACM holds
        // can be the one presented.
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
        load_balancer,
        _listener: listener,
    })
}

/// The SHARED certificate key-material directory in the shared engine: call it with a
/// handler's `&*self.engine`.
///
/// What an HTTPS or TLS listener PRESENTS is the certificate Certificate Manager holds for
/// its `Certificates[].CertificateArn` — that service's resource, in that service's store,
/// which this one cannot see. The material a handshake needs (the leaf, its chain and the
/// private key that matches it) is held in ONE directory under one shared type
/// ([`::service_common::acm_certificate_key_material::CertificateKeyMaterialDirectory`]),
/// written by the service that holds the certificate and read here. The engine keys state
/// by TYPE, so a copy of its own in this service's state would be invisible to the writer.
///
/// `None` on an engine carrying no directory: a TLS connection is then served by the core's
/// own TLS server, under the emulator's own certificate, rather than refused.
pub fn certificate_key_material_directory<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<
    ::engine::ServiceState<
        ::service_common::acm_certificate_key_material::CertificateKeyMaterialDirectory,
    >,
> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::acm_certificate_key_material::CertificateKeyMaterialDirectory,
            >,
        )
}
