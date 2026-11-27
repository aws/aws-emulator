// SPDX-License-Identifier: CC0-1.0

//! WHERE a workspace's Grafana console and native Grafana HTTP API are ANNOUNCED: the
//! value `WorkspaceDescription$endpoint` / `WorkspaceSummary$endpoint` carries, and the
//! addressings [`crate::service::workspace_api`] reads back off a request.
//!
//! # The rule this module implements
//!
//! `endpoint` is "The URL that users can use to access the Grafana console in the
//! workspace" (`smithy_query(query="shape", shape="WorkspaceDescription")`), so it is not a
//! label a client displays but an ADDRESS it dials: it splices it into a request of its own
//! (`https://<endpoint>/api/org`) to reach that workspace's Grafana HTTP API, or hands it
//! to something that takes a host and nothing else (a resolver, SNI, a `Host` header, an
//! allow-list). So it is a bare `host[:port]` — no scheme and no path, the form the real
//! service's own value is in — and the host is one THIS emulator answers on: the address a
//! request reached it at ([`announced_authority`]).
//!
//! A name in a zone Amazon Web Services publishes its own endpoints in resolves nowhere
//! near this process, or at genuine infrastructure: a client handed one leaves the emulated
//! environment carrying the credentials it holds and reaches no workspace at all. So no
//! such name is announced, however the real service spells its own.
//!
//! # Which workspace a request at that address addresses
//!
//! The real service gives each workspace a host of its own, and this process serves its
//! whole surface at ONE endpoint, so the announced address cannot carry the workspace in
//! its host. What a Grafana request to it carries instead is the CREDENTIAL the control
//! plane minted for one workspace — an API key or a service-account token, presented as a
//! bearer token, which is the only thing such a request has — and that credential says
//! which workspace it addresses (`crate::state::workspace_holding_bearer_token`), exactly
//! as the per-workspace host does on the real service.
//!
//! Beside it, a client that wants to name a workspace in the ADDRESS has the path this
//! emulator also answers ([`workspace_endpoint_path`], read back by
//! [`workspace_at_announced_path`]), and a client whose own resolver points the real
//! service's published host at this process is answered there too
//! ([`crate::state::workspace_id_at_endpoint_host`]).
//!
//! Nothing here opens a connection or resolves a name: every function reads request parts
//! or a string and returns a string.

use ::service_common::data_plane_addressing as addressing;
use ::smithy_server_runtime::OperationContext;

/// The path segment naming THIS service's data plane, under the reserved namespace every
/// plane this emulator serves on its own endpoint sits beneath
/// ([`addressing::EMULATOR_PATH_NAMESPACE`]): this service's own `arnNamespace`, so no other
/// service's plane can claim the same path.
pub const ENDPOINT_PLANE_SEGMENT: &str = crate::state::GRAFANA_ARN_NAMESPACE;

/// The `host[:port]` this emulator is reached at, for the request `ctx` is answering.
///
/// The three readings are the shared ones, taken in the order every data plane served at
/// the front door announces an address in:
///
/// * the EMULATOR'S OWN endpoint this request addressed, port included
///   ([`addressing::reached_emulator_endpoint_of_parts`]) — a client configured with one
///   local endpoint resolves nothing else, so it is handed back the very address it is
///   already talking to;
/// * the address this process is served at ([`addressing::served_endpoint_authority`]) for
///   a request that addressed a host of the real service's instead, so an in-process call
///   of another service announces an address that service's caller can reach too;
/// * the base an emulator serving no port at all reports
///   ([`addressing::emulator_endpoint_authority`]) — a host nothing resolves, which is the
///   truth about such an emulator rather than a name of the real service's.
#[must_use]
pub fn announced_authority(ctx: &OperationContext<'_>) -> String {
    addressing::reached_emulator_endpoint_of_parts(ctx.parts())
        .or_else(|| addressing::served_endpoint_authority(ctx.endpoint().as_str()))
        .unwrap_or_else(|| addressing::emulator_endpoint_authority(ctx))
}

/// The endpoint announced for a workspace on an emulator reached at `authority`: that
/// authority itself, as a bare `host[:port]`.
///
/// It is what every projection of a workspace reports, so the address a create publishes
/// and the address every later read reports cannot drift from one another — and it is an
/// address the client reading it can dial, with the Grafana route spliced straight onto it.
/// WHICH workspace such a request addresses is said by the credential it presents (see the
/// module docs), not by the address.
///
/// Any trailing `/` is dropped, because the value carries no path at all and a client
/// splices its own route onto it.
#[must_use]
pub fn workspace_endpoint(authority: &str) -> String {
    authority.trim_end_matches('/').to_string()
}

/// The path a workspace's Grafana HTTP API is also served under on this emulator's own
/// endpoint: `/_aws/grafana/<workspaceId>`.
///
/// The addressing for a client that names the workspace in the ADDRESS rather than through
/// the credential it presents: it carries what the real service's published host carries —
/// WHICH workspace is addressed — in a path, since this process answers on one host.
#[must_use]
pub fn workspace_endpoint_path(workspace_id: &str) -> String {
    format!(
        "/{}/{ENDPOINT_PLANE_SEGMENT}/{workspace_id}",
        addressing::EMULATOR_PATH_NAMESPACE
    )
}

/// The workspace an endpoint path addresses, and the Grafana route beneath it — and `None`
/// for a path that is not a workspace endpoint of this service.
///
/// The inverse of [`workspace_endpoint_path`]: the first label of the real service's host
/// travels in a path segment here, so the segment must satisfy `WorkspaceId`
/// ([`crate::state::is_workspace_id`]) exactly as that label does. A request that named no
/// route under the endpoint addresses the Grafana API's own root (`/`).
#[must_use]
pub fn workspace_at_announced_path(path: &str) -> Option<(String, String)> {
    let (namespace, rest) = addressing::first_segment(path)?;
    if namespace != addressing::EMULATOR_PATH_NAMESPACE {
        return None;
    }
    let (plane, rest) = addressing::first_segment(rest)?;
    if plane != ENDPOINT_PLANE_SEGMENT {
        return None;
    }
    let (workspace_id, route) = addressing::first_segment(rest)?;
    crate::state::is_workspace_id(workspace_id)
        .then(|| (workspace_id.to_string(), route.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: an announced endpoint is the address this emulator is reached at and nothing
    /// else — a bare `host[:port]`, carrying no scheme, no path and no name of Amazon Web
    /// Services' own — and it is the same value for every workspace of that emulator.
    ///
    /// The rows span the forms an emulator's address takes: loopback by address with a
    /// port, loopback by name, a bracketed IPv6 literal, a routable address, a name with no
    /// port, and the base of an emulator served on no port at all.
    #[test]
    fn an_announced_endpoint_is_the_address_this_emulator_is_reached_at() {
        let rows: &[(&str, &str)] = &[
            ("loopback with a port", "127.0.0.1:9297"),
            ("loopback by name", "localhost:39355"),
            ("a bracketed IPv6 literal", "[::1]:8443"),
            ("a routable address", "198.51.100.7:8443"),
            ("a name with no port", "emulator-host"),
            ("an emulator served nowhere", "emulator.invalid"),
        ];
        for (case, authority) in rows {
            let announced = workspace_endpoint(authority);
            assert_eq!(
                announced, *authority,
                "case {case:?}: expected {authority:?}, got {announced:?}"
            );
            assert!(
                !announced.contains('/') && !announced.contains("://"),
                "case {case:?}: an announced endpoint carries no scheme and no path, got \
                 {announced:?}"
            );
            assert!(
                !addressing::addresses_an_aws_endpoint(&announced),
                "case {case:?}: an announced endpoint names no host Amazon Web Services \
                 publishes its own endpoints at, got {announced:?}"
            );
            // A trailing `/` on the address this emulator reports is not part of the host.
            let slashed = workspace_endpoint(&format!("{authority}/"));
            assert_eq!(
                slashed, *authority,
                "case {case:?}: expected {authority:?} from a trailing slash, got {slashed:?}"
            );
        }
    }

    /// RULE: the path addressing names its workspace and the Grafana route beneath it, and
    /// reads back as exactly that workspace.
    #[test]
    fn the_path_addressing_reads_back_as_the_workspace_it_names() {
        let rows: &[(&str, &str)] = &[
            ("a minted id", "g-0123456789"),
            ("an id of hex letters", "g-abcdef0123"),
            ("an id ending in letters", "g-00000000ff"),
            ("the lowest id", "g-0000000000"),
        ];
        for (case, workspace_id) in rows {
            let path = workspace_endpoint_path(workspace_id);
            let want = format!("/_aws/grafana/{workspace_id}");
            assert_eq!(
                path, want,
                "case {case:?}: expected {want:?}, got {path:?}"
            );
            let read_back = workspace_at_announced_path(&format!("{path}/api/org"));
            assert_eq!(
                read_back,
                Some(((*workspace_id).to_string(), "/api/org".to_string())),
                "case {case:?}: the endpoint path must read back as its workspace and \
                 route, got {read_back:?}"
            );
        }
    }

    /// RULE: only a path under this service's own plane segment that names a `WorkspaceId`
    /// is a workspace endpoint path of this service; everything else is claimed by nothing
    /// here.
    #[test]
    fn a_path_that_is_no_workspace_endpoint_names_no_workspace() {
        /// One row: the case, the path, and the workspace and route it names.
        type Row = (&'static str, &'static str, Option<(&'static str, &'static str)>);
        let rows: &[Row] = &[
            (
                "the endpoint itself",
                "/_aws/grafana/g-0123456789",
                Some(("g-0123456789", "/")),
            ),
            (
                "a route beneath it",
                "/_aws/grafana/g-0123456789/api/org",
                Some(("g-0123456789", "/api/org")),
            ),
            (
                "a deeper route beneath it",
                "/_aws/grafana/g-0123456789/api/org/users",
                Some(("g-0123456789", "/api/org/users")),
            ),
            ("a control-plane route", "/workspaces/g-0123456789", None),
            ("another service's plane", "/_aws/athena/g-0123456789", None),
            ("no namespace segment", "/grafana/g-0123456789/api/org", None),
            (
                "an id outside WorkspaceId",
                "/_aws/grafana/not-a-workspace/api/org",
                None,
            ),
            (
                "an id in upper case, which WorkspaceId admits nowhere",
                "/_aws/grafana/G-0123456789/api/org",
                None,
            ),
            ("no workspace at all", "/_aws/grafana", None),
            ("a relative path", "_aws/grafana/g-0123456789", None),
        ];
        for (case, path, want) in rows {
            let read = workspace_at_announced_path(path);
            let expected =
                want.map(|(id, route)| (id.to_string(), route.to_string()));
            assert_eq!(
                read, expected,
                "case {case:?}: expected {expected:?} for {path:?}, got {read:?}"
            );
        }
    }
}
