// SPDX-License-Identifier: CC0-1.0

//! The SOCKET every HTTP api's `ApiEndpoint` is published on.
//!
//! WHY an api's data plane is published on a socket of its own rather than at this emulator's
//! front door — which fields a client reads off an answer, and who writes their names — is
//! [`crate::state::invoke_plane_transport`]'s to state. This module OPENS that socket and says
//! where a request accepted on it goes; nothing else here.
//!
//! The transport itself is not written again: encryption is negotiated in band
//! ([`::service_common::in_band_tls`]), because the address is published for the `https` an HTTP
//! api is invoked over ("clients can invoke your API with the default
//! https://{api_id}.execute-api.{region}.amazonaws.com endpoint",
//! `Api$DisableExecuteApiEndpoint`) and the handshake's first byte is the only thing on the
//! connection that tells a TLS client from a plaintext one; and the request is then read,
//! bounded and handed to this emulator's own router by the ONE HTTP endpoint every core-owned
//! socket of this service shares ([`super::websocket::endpoint_http`]), which is also where the
//! canonical field-name spelling this address exists for is written.
//!
//! What is this socket's own is WHICH requests it answers for: the HTTP apis of this account.
//! Each is PUBLISHED there under THIS SOCKET'S OWN ADDRESS — the `host:port` the core's
//! listener landed on — with the api's own id behind the reserved data-plane marker
//! ([`crate::state::invoke_plane_transport::announced_endpoint`]), which is the path this
//! emulator's front door serves the same api at, so a client that dialled the published value
//! names the api in the path and its request goes to the router AS SENT. A request that
//! carried the api's own `execute-api` HOST instead — the addressing `Api$ApiEndpoint`
//! documents, which a client that resolves such a name sends — is served by re-addressing it
//! onto the api its leading label names ([`published_target`]). A request addressing anything
//! else — the control plane, a host in a zone this plane serves no endpoint in, a
//! path whose dot segment would resolve out of the plane — is refused here, in the envelope
//! every answer of this service's data planes carries, and no service is asked about it.
//! Everything the answer itself is — the stage, the deployment, the route, its authorization
//! and its integration — is [`super::invoke`]'s, which serves the same request however it was
//! addressed.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use ::emulator_server::{ListenerConn, NativeHandler, TlsUpgrader};
use ::smithy_server_runtime::EmbeddedEmulator;

use super::websocket::endpoint_http::{Addressed, Readdress};

/// The status and sentence this address refuses a request that addresses no api of this plane
/// with: the one the `execute-api` endpoint space answers an address that reaches no api of the
/// account with (`super::data_plane`), so this socket refuses such a request exactly as the
/// endpoint space it belongs to does.
const FORBIDDEN: u16 = super::data_plane::FORBIDDEN_STATUS;
const FORBIDDEN_MESSAGE: &str = super::data_plane::FORBIDDEN_MESSAGE;

/// The host a re-addressed request names this emulator's own front door by — the one every
/// core-owned socket of this service re-addresses at
/// (`super::data_plane::SERVICE_ENDPOINT_HOST`), read here for the tables below.
#[cfg(test)]
const SERVICE_ENDPOINT: &str = super::data_plane::SERVICE_ENDPOINT_HOST;

/// The `host:port` this emulator publishes every HTTP api's data plane at, opening the socket
/// on first use — or `None` when this emulator opens none, in which case the caller announces
/// the endpoint the request arrived at instead
/// ([`crate::state::emulator_served_api_endpoint`]).
///
/// A socket is opened only where this emulator PUBLISHES a dialable address at all: a process
/// that is served on a port (`served_base`, the base URL `OperationContext::endpoint` reports)
/// hands out addresses a client then dials, and this is the address the api's data plane is
/// dialled at. An emulator driven IN PROCESS is served on no port at all
/// (`smithy_server_runtime::UNSERVED_URL_BASE`): it answers every request its embedder hands
/// it and nobody dials anything, so no port is bound for one and the api is named on the
/// endpoint the caller reached, which the front door serves the same plane on.
///
/// Opening the address changes no api and is idempotent: the first socket is kept, so every
/// api of the account is published on one address for as long as the store lives and a client
/// may keep a URL it was handed. The store guard is taken and released around the bind rather
/// than held across it, because a lock held across an await would make this future non-`Send`.
///
/// `callback` is the in-process transport a request accepted there is served over — the handle
/// the operation that opened the socket was given (`OperationContext::callback`) — kept for the
/// socket's whole life, because a request arriving on it is answered by this emulator's own
/// router.
pub async fn ensure_published<Eng: Send + Sync + 'static>(
    engine: &Eng,
    callback: &EmbeddedEmulator,
    served_base: &str,
) -> Option<String> {
    crate::state::served_endpoint_authority(served_base)?;
    let state = crate::state::service_state(engine)?;
    let existing = state.lock().invoke_plane_transport.published();
    if existing.is_some() {
        return existing;
    }
    let shared = (engine as &dyn ::std::any::Any).downcast_ref::<::engine::InMemoryEngine>()?;
    let listeners = shared.listeners();
    let handler = ApiDataPlaneAddress {
        callback: callback.clone(),
        // The core's own TLS server, lent to this plaintext socket so the `https` half of the
        // address is terminated by the core rather than here. An emulator that could prepare
        // none serves the request made in the clear alone.
        upgrader: listeners.tls_upgrader().ok(),
    };
    // Opened on a task of its OWN, so a runtime that cannot bind a socket at all — one built
    // with no IO driver, which is what a host driving this emulator in process may hand a
    // request — leaves this call without an address of its own instead of taking the request
    // down with it.
    let opened = ::tokio::spawn(async move { listeners.tcp_plaintext(handler).await.ok() }).await;
    let listener = Arc::new(opened.ok().flatten()?);

    let mut store = state.lock();
    // Another call may have opened one while this one was binding; keep the first, so a
    // published address never changes under a client. Dropping the loser closes its port.
    if let Some(published) = store.invoke_plane_transport.published() {
        return Some(published);
    }
    let authority = store.invoke_plane_transport.remember_listener(listener);
    // An address this service opened resolves to this service: nobody can read a service off a
    // `host:port`, so the one register such a reader looks in is told who answers there
    // ([`::service_common::published_data_planes`]).
    //
    // LOCK ORDER: this service's own store first and that shared register second, the order
    // every publisher of a data-plane address takes them in; nothing holds the register while
    // taking a service store, so the order cannot invert.
    if let Some(planes) = ::service_common::published_data_planes::PublishedPlanes::of(shared) {
        planes.publish(&authority, crate::ROUTE_KEY);
    }
    ::tracing::info!(
        %authority,
        "opened the socket this emulator publishes an HTTP api's data plane on"
    );
    Some(authority)
}

/// The handler the core hands each connection accepted on the socket this plane is published
/// on.
///
/// It carries the in-process transport a request is served over and the core's own TLS server;
/// nothing else, because nothing else about such a connection is this module's.
struct ApiDataPlaneAddress {
    /// The emulator the call that opened this socket was served by — the same router, the same
    /// engine, the same state. It is an IN-PROCESS handle: a request this socket accepts is
    /// dispatched inside this process and nothing here dials out, so the only address involved
    /// is the one the core bound.
    callback: EmbeddedEmulator,
    upgrader: Option<TlsUpgrader>,
}

impl NativeHandler for ApiDataPlaneAddress {
    /// One call per accepted connection. The handles are cloned into the connection's own
    /// future, which is what the core's `'static` handler contract asks for.
    fn serve(&self, conn: ListenerConn) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let callback = self.callback.clone();
        let upgrader = self.upgrader.clone();
        Box::pin(async move {
            let Some(conn) = ::service_common::in_band_tls::negotiated(conn, upgrader).await
            else {
                return;
            };
            let readdress: Readdress = Arc::new(published_target);
            super::websocket::endpoint_http::serve_readdressed(
                callback,
                conn,
                Some(readdress),
                // Every request this address serves is an invoke of an api of this account:
                // the path names the api ([`published_target`]) and anything else is refused
                // above, so a request a browser sent from another site is served here and
                // recorded as having been, and the api's own `CorsConfiguration` decides the
                // decoration of the answer.
                super::websocket::endpoint_http::Arrival::ApiDataPlane,
            )
            .await;
        })
    }
}

/// Where a request `parts` addresses goes on this address.
///
/// The endpoint published on this socket is this socket's own address with the api behind the
/// reserved data-plane marker ([`crate::state::invoke_plane_transport::announced_endpoint`]),
/// so a client that dialled the published URL names the api in its PATH — with the stage name
/// and route path appended, the composition `Api$ApiEndpoint` documents ("the stage name is
/// typically appended to this URI") — and such a request goes to the router AS SENT, where
/// [`super::invoke`] answers it with the api's stages, routes, authorizers and integrations.
/// A client that carried the api's own `execute-api` HOST instead, which is the addressing AWS
/// publishes, is RE-ADDRESSED onto the api that host's leading label names, at the same
/// reserved data-plane path ([`readdressed`]), so it is answered by the same records.
///
/// Refused here, and nothing else is: a request that addresses no api of this plane at all —
/// neither in its host nor in its path — and one carrying a dot segment, which could resolve
/// out of the path an api is published at, and that path is the only thing that says which api
/// must answer.
///
/// A request a browser sent from ANOTHER SITE is served on this address rather than refused,
/// and recorded as having been, because what is refused here leaves nothing it carries but an
/// invoke of an api of this account
/// ([`super::websocket::endpoint_http::Arrival::ApiDataPlane`], where the whole rule is
/// stated): the api's own `CorsConfiguration` decides the decoration of the answer, and the
/// router this endpoint hands the request to still refuses such a request for every request
/// that names an operation there.
///
/// The `Host` ALLOWLIST — the core's DNS-rebinding guard, which the shared endpoint applies to
/// every request served AS SENT — stays binding on the host-style branch too, through the two
/// zones that branch admits: the zone AWS ITSELF publishes the api's endpoint in and the
/// loopback zone RFC 6761 reserves for the interface the core binds on ([`addressed_api_id`],
/// `crate::state::serves_execute_api_zone`). No resolver an attacker controls answers for a
/// name in either — AWS answers for the one, and RFC 6761 reserves the other and every name
/// under it for the loopback interface — so admitting them widens the set of names that reach
/// this address by exactly the names the api's own endpoint IS, which is what
/// `Api$ApiEndpoint` documents, while a name in any other zone, which only somebody else's
/// resolver could point here, is refused exactly as it was before. What a request that IS
/// admitted may reach is narrowed to the api its host names and the path under that api, which
/// is what [`readdressed`] renders and reads back.
fn published_target(parts: &::http::request::Parts) -> Addressed {
    let path = parts.uri.path();
    if ::service_common::data_plane_addressing::carries_a_dot_segment(path) {
        return Addressed::Refused(FORBIDDEN, FORBIDDEN_MESSAGE);
    }
    if addresses_this_plane(path) {
        return Addressed::AsSent;
    }
    match addressed_api_id(parts) {
        Some(api_id) => readdressed(&api_id, &parts.uri),
        None => Addressed::Refused(FORBIDDEN, FORBIDDEN_MESSAGE),
    }
}

/// The api id the HOST of `parts` names, or `None` for a request whose host is not an api's
/// own `execute-api` endpoint in a zone this plane serves — this socket's own address among
/// them.
///
/// The host is read the way every front door of this emulator reads it: the `Host` field, and
/// the authority of an absolute-form target for a request that carries its endpoint there. Any
/// userinfo and `:port` and the trailing root dot are removed, and the whole name is folded to
/// lower case before anything is matched, because a DNS name is case-insensitive (RFC 4343) —
/// so is the api id, which is answered folded, as the id the store holds. The leading label and
/// the endpoint label behind it are taken through the data planes' one reading of an
/// `execute-api` host (`super::data_plane::api_id_of_host`, which admits the `execute-api-…`
/// variant spellings AWS publishes), so the address published and the address served cannot
/// drift apart.
///
/// Narrowed to the zones an api's own `execute-api` host can only lead back to
/// ([`crate::state::serves_execute_api_zone`]): exactly one Region label, then the zone AWS
/// publishes that endpoint in or the loopback zone RFC 6761 reserves for the interface the
/// core binds on — `{api-id}.execute-api.{region}.amazonaws.com`, and that same name written
/// over the zone a resolver answers with this interface. Admitting a host
/// is admitting a name that may reach this address without the core's `Host` allowlist, and
/// those are the two zones no resolver somebody else controls can answer for; a name carrying
/// no Region, a deeper zone, or any other zone is one only such a resolver could point at this
/// machine, which is the rebinding the allowlist exists to refuse. The address this plane
/// ANNOUNCES carries the api in the PATH instead, which this address serves as sent.
fn addressed_api_id(parts: &::http::request::Parts) -> Option<String> {
    let host = parts
        .headers
        .get(::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .or_else(|| parts.uri.host())?;
    let named = super::data_plane::host_name(host)
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let api_id = super::data_plane::api_id_of_host(&named)?;
    // What follows the endpoint label: it has to be one Region label and then the zone, so a
    // name carrying no Region, or a deeper name inside the zone, names no served endpoint.
    let (region, zone) = named.splitn(3, '.').nth(2)?.split_once('.')?;
    (!region.is_empty() && crate::state::serves_execute_api_zone(zone)).then(|| api_id.to_string())
}

/// The request a host-style invoke is handed to the router as: the api named by the reserved
/// data-plane path, the invoke path the client sent carried under it, and the query string
/// carried on unchanged.
///
/// Composed through the one statement of that re-addressing every socket of this service
/// shares (`super::data_plane::readdressed_within`), which also reads the rendered target back
/// and refuses one that does not stay UNDER the api's own path — the one thing a path the
/// CLIENT chose must not be able to leave.
fn readdressed(api_id: &str, target: &::http::Uri) -> Addressed {
    let query = target
        .query()
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    super::data_plane::readdressed_within(
        super::data_plane::Readdressing {
            label: crate::state::EXECUTE_API_LABEL,
            resource_id: api_id,
            within: target.path(),
            query: &query,
        },
        (FORBIDDEN, FORBIDDEN_MESSAGE),
    )
}

/// Whether `path` addresses an api of this plane: the reserved data-plane marker, the endpoint
/// label this service publishes an api behind
/// ([`crate::state::invoke_plane_transport::announced_endpoint`]), and an api id after it.
///
/// Read with the tree's one reading of a path's segments
/// (`service_common::data_plane_addressing`), so the address published and the address served
/// cannot drift apart.
fn addresses_this_plane(path: &str) -> bool {
    let (marker, rest) =
        ::service_common::data_plane_addressing::first_segment(path).unwrap_or_default();
    if marker != crate::state::LOCAL_ENDPOINT_PREFIX {
        return false;
    }
    let (label, rest) =
        ::service_common::data_plane_addressing::first_segment(rest).unwrap_or_default();
    label == crate::state::EXECUTE_API_LABEL
        && ::service_common::data_plane_addressing::first_segment(rest)
            .is_some_and(|(api_id, _within)| !api_id.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: this address serves exactly the requests it is the address OF — a path naming an
    /// api behind the reserved data-plane marker — and refuses everything else itself.
    #[test]
    fn only_a_path_naming_an_api_of_this_plane_is_served() {
        // The rows span what the reading generalises over: the announced path itself, a stage
        // and a route path under it, a trailing separator, and the paths that name no api of
        // this plane at all (the marker with no api, another plane's label, this service's
        // control-plane path, a path of an api's own carrying no marker, and the root).
        let cases: &[(&str, bool)] = &[
            ("/_aws/execute-api/abc1230000", true),
            ("/_aws/execute-api/abc1230000/config", true),
            ("/_aws/execute-api/abc1230000/prod/orders/7", true),
            ("/_aws/execute-api/abc1230000/", true),
            ("/_aws/execute-api/", false),
            ("/_aws/execute-api", false),
            ("/_aws/connections/abc1230000", false),
            ("/v2/apis", false),
            ("/config", false),
            ("/", false),
            ("", false),
        ];
        for (path, served) in cases {
            assert_eq!(
                addresses_this_plane(path),
                *served,
                "case {path:?}: expected served={served}"
            );
        }
    }

    /// RULE: a target that could resolve OUT of the path an api is published at is refused
    /// here, whatever it would have reached — the path is the only thing that says which api
    /// must answer.
    #[test]
    fn a_target_carrying_a_dot_segment_is_refused() {
        // The rows span the forms a dot segment is written in, each under a path that would
        // otherwise address an api of this plane, against the plain path as the control.
        let cases: &[(&str, bool)] = &[
            ("/_aws/execute-api/abc1230000/config", true),
            ("/_aws/execute-api/abc1230000/../v2/apis", false),
            ("/_aws/execute-api/abc1230000/./config", false),
            ("/_aws/execute-api/abc1230000/%2e%2e/v2/apis", false),
        ];
        for (path, served) in cases {
            let request = ::http::Request::builder()
                .uri(format!("https://127.0.0.1:1{path}"))
                .body(())
                .expect("the case's target is a well-formed request");
            let addressed = published_target(&request.into_parts().0);
            let expected = if *served {
                Addressed::AsSent
            } else {
                Addressed::Refused(FORBIDDEN, FORBIDDEN_MESSAGE)
            };
            assert_eq!(
                addressed, expected,
                "case {path:?}: expected {expected:?}, got {addressed:?}"
            );
        }
    }

    /// RULE: a request that arrives under an api's own `execute-api` host, in a zone this
    /// plane serves, is placed on that api at the reserved data-plane path, with the invoke
    /// path and query string the client sent carried under it; a host in any other zone —
    /// which only somebody else's resolver could point at this address — is refused, as the
    /// core's own `Host` allowlist refuses it.
    #[test]
    fn a_host_style_invoke_is_readdressed_onto_the_api_its_leading_label_names() {
        // The rows span what the reading generalises over: the invoke path (a stage and route
        // path, a route path alone, the root, a stage alone), a query string, the endpoint
        // variant label AWS publishes, whether the client wrote the port, a trailing root dot
        // and the case the name is written in; and the hosts that name no endpoint this
        // plane serves (this socket's own address with no plane path, a second label that is
        // not the endpoint's, a name carrying no Region label, a deeper name inside either
        // zone, and the SAME api named in a name the operator may serve under and in a zone
        // of somebody else's).
        let cases: &[(&str, &str, &str, Option<&str>)] = &[
            (
                "abc1230000.execute-api.us-west-2.amazonaws.com",
                "/prod/orders/7",
                "",
                Some("/_aws/execute-api/abc1230000/prod/orders/7"),
            ),
            (
                "abc1230000.execute-api.us-west-2.amazonaws.com:34567",
                "/items",
                "?page=2&size=10",
                Some("/_aws/execute-api/abc1230000/items"),
            ),
            (
                "zzz9990000.execute-api.eu-central-1.amazonaws.com",
                "/",
                "",
                Some("/_aws/execute-api/zzz9990000/"),
            ),
            (
                "0123456789.execute-api-fips.ap-southeast-2.amazonaws.com.",
                "/prod",
                "",
                Some("/_aws/execute-api/0123456789/prod"),
            ),
            ("127.0.0.1:34567", "/items", "", None),
            (
                "abc1230000.execute-apifoo.us-west-2.amazonaws.com",
                "/items",
                "",
                None,
            ),
            (
                // A DNS name is case-insensitive (RFC 4343), so the same name in any case is
                // the same endpoint, and the api id it names is the one the store holds.
                "ABC1230000.EXECUTE-API.US-WEST-2.AMAZONAWS.COM",
                "/items",
                "",
                Some("/_aws/execute-api/abc1230000/items"),
            ),
            (
                // No Region label: no endpoint AWS publishes is named, so this is not the
                // name admitted without the core's allowlist.
                "abc1230000.execute-api.amazonaws.com",
                "/items",
                "",
                None,
            ),
            (
                // A deeper name inside the zone is not the published endpoint either.
                "abc1230000.execute-api.us-west-2.sub.amazonaws.com",
                "/items",
                "",
                None,
            ),
            (
                // The zone this plane ANNOUNCES an api's endpoint in: RFC 6761 reserves it
                // and every name under it for the loopback interface the core binds on, so
                // the name leads back here and nowhere else.
                "abc1230000.execute-api.us-west-2.localhost",
                "/items",
                "",
                Some("/_aws/execute-api/abc1230000/items"),
            ),
            (
                "abc1230000.execute-api.us-west-2.localhost:34567",
                "/prod/orders/7",
                "",
                Some("/_aws/execute-api/abc1230000/prod/orders/7"),
            ),
            (
                // A deeper name inside the loopback zone is no endpoint this plane announces
                // either, so it is not admitted without the core's allowlist.
                "abc1230000.execute-api.us-west-2.sub.localhost",
                "/items",
                "",
                None,
            ),
            (
                "abc1230000.execute-api.us-west-2.emulator.internal",
                "/items",
                "",
                None,
            ),
            (
                "abc1230000.execute-api.us-west-2.attacker.example.com",
                "/items",
                "",
                None,
            ),
        ];
        for (host, path, query, expected) in cases {
            let request = ::http::Request::builder()
                .uri(format!("https://{host}{path}{query}"))
                .header(::http::header::HOST, *host)
                .body(())
                .expect("the case's target is a well-formed request");
            let addressed = published_target(&request.into_parts().0);
            match expected {
                Some(expected_path) => {
                    let Addressed::At { uri, host: named } = &addressed else {
                        panic!("case {host:?}{path:?}: expected a re-addressed request, got {addressed:?}");
                    };
                    assert_eq!(
                        named, SERVICE_ENDPOINT,
                        "case {host:?}{path:?}: expected the front door's host, got {named:?}"
                    );
                    assert_eq!(
                        uri,
                        &format!("https://{SERVICE_ENDPOINT}{expected_path}{query}"),
                        "case {host:?}{path:?}: expected the api's reserved data-plane path"
                    );
                }
                None => assert_eq!(
                    addressed,
                    Addressed::Refused(FORBIDDEN, FORBIDDEN_MESSAGE),
                    "case {host:?}{path:?}: expected a refusal, got {addressed:?}"
                ),
            }
        }
    }
}
