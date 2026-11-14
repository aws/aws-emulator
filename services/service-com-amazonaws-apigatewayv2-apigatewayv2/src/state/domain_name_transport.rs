// SPDX-License-Identifier: CC0-1.0

//! The TRANSPORT of a custom domain name: the core-owned socket each `DomainName` of this
//! account is reached at.
//!
//! `DomainNameConfiguration$ApiGatewayDomainName` is "the domain name associated with the
//! regional endpoint for this custom domain name" — an address a CLIENT then dials, after
//! pointing its own DNS record at it, not a label it reads. A name under API Gateway's own
//! zone resolves to nothing on the machine this emulator runs on, so a domain name that
//! published one published an address no client could reach, and everything a custom domain
//! name exists for — a request addressed to the domain being served by the api and stage its
//! `ApiMapping` names — was unobservable.
//!
//! So each custom domain name gets a socket of the EMULATOR's own, opened through the core
//! (`engine.listeners().tcp_plaintext(..)`, which owns the socket, the bind interface, the
//! port range and the connection caps), and the record publishes the `host:port` that socket
//! answers on. A client dials exactly what it was told.
//!
//! The socket is the ADDRESS, not the data plane: what a request it accepts is answered BY —
//! the api and stage its api mapping names, that stage's routes, authorizers, throttles and
//! integrations — stays in [`crate::service::invoke`], which serves the same request however
//! it was addressed ([`crate::service::domain_name_socket`]).
//!
//! The socket's life is the domain name's: the handle is held on this service's own store
//! under the record's own key ([`super::listener_registry`], the one registry both kinds of
//! core-owned socket of this service are kept in), and dropping it (a delete) closes the port
//! and severs every connection in flight on it.

/// The `host:port` a core listener is addressed by — re-exported from the one registry both
/// kinds of socket of this service share, so a caller of this module reads an address the same
/// way the WEBSOCKET transport does.
pub use super::listener_registry::authority_of;

/// The listener each custom domain name of this account is reached at, keyed by the
/// `DomainName` the record is stored under.
///
/// A field of [`crate::state::ComAmazonawsApigatewayv2Apigatewayv2State`] rather than a store
/// of its own, for the reason the WebSocket transport is one
/// ([`crate::state::websocket_transport`]): the records that say which domain names exist and
/// the sockets those domain names are reached at are read under ONE lock and cannot drift
/// apart.
pub type DomainNameTransport = super::listener_registry::ListenerRegistry;
