// SPDX-License-Identifier: CC0-1.0

//! The workspace's own (native) Grafana HTTP API: the data plane the endpoint
//! `DescribeWorkspace` reports answers on.
//!
//! `CreateWorkspaceApiKey` mints a key "used to authenticate requests sent to the
//! workspace's HTTP API", and `CreateWorkspaceServiceAccountToken` mints a key
//! "used when calling Grafana APIs". That API is not in the Smithy model — the
//! model describes the control plane only — so it names no modelled operation and
//! is served from [`crate::service::EmulatedService::override_routing_error`],
//! exactly as `service-com-amazonaws-codecommit-codecommit-20150413` serves git's
//! smart-HTTP transport.
//!
//! The address it answers on is the one [`crate::state::announced_endpoint`] announces: the
//! `host[:port]` a control-plane request reached this emulator at, which the control plane
//! reports as `WorkspaceDescription$endpoint`. A client that reads `endpoint` and appends a
//! Grafana route to it reaches this module:
//!
//! | request | meaning |
//! |---|---|
//! | `GET https://<endpoint>/api/org` | the workspace's organization: its `id` and `name` |
//!
//! That address names no workspace — this process answers its whole surface on one host —
//! so WHICH workspace such a request addresses is said by the CREDENTIAL it presents, the
//! bearer key the control plane minted for one workspace
//! ([`crate::state::ComAmazonawsGrafanaAwsgrafanacontrolplaneState::workspace_holding_bearer_token`]).
//!
//! Two further addressings name a workspace in the ADDRESS itself and reach the same API: a
//! path under this emulator's own endpoint ([`crate::state::workspace_at_announced_path`],
//! `/_aws/grafana/<workspaceId>`), and the host Amazon Managed Grafana itself publishes for
//! a workspace ([`crate::state::workspace_id_at_endpoint_host`]), for a client whose own
//! resolver points that name at this process; there the whole path is the Grafana route,
//! because such a host carries no path.
//!
//! Every other route under the endpoint is answered with **501**, never an empty
//! 200: a client cannot tell an unimplemented route from an implemented one that
//! has nothing to say.
//!
//! # Authentication
//!
//! It verifies no request signature, and cannot: a caller presents
//! `Authorization: Bearer <key>` — an API key or a service-account token, which
//! are not SigV4 credentials — so there is nothing for the framework's SigV4
//! authenticator to check, and refusing every such request instead is exactly the
//! defect this data plane fixes. A presented token is recognised by its
//! [`crate::state::bearer_token_digest`], held on the key record the control plane
//! wrote, so the store never holds the token itself. A token the workspace no
//! longer holds — deleted, expired, or belonging to a deleted workspace or
//! disabled service account — authenticates nothing, which is what makes
//! revocation observable at the destination.
//!
//! Both faces of the service read ONE store: the keys this module authenticates
//! are the records `CreateWorkspaceApiKey` / `CreateWorkspaceServiceAccountToken`
//! persisted and `DeleteWorkspaceApiKey` removes, and a service-account token that
//! authorizes here has its `lastUsedAt` set, which
//! `ListWorkspaceServiceAccountTokens` then reports.

use std::time::SystemTime;

use crate::state::{
    ComAmazonawsGrafanaAwsgrafanacontrolplaneState as Store, Workspace,
    WORKSPACE_ACTIVE as ACTIVE,
};

/// The id of the one organization a workspace's Grafana has.
///
/// Grafana numbers organizations from 1 and a workspace is a single Grafana
/// server with a single organization, so that organization is organization 1.
const WORKSPACE_ORG_ID: u64 = 1;

/// What one request to a workspace's Grafana HTTP API named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// How the request said which workspace it addresses.
    pub addressed: Addressed,
    /// The route it asked for.
    pub route: Route,
}

/// How one request said WHICH workspace of this service it addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Addressed {
    /// The workspace the ADDRESS named: the endpoint path under this emulator's own
    /// endpoint, or the host the real service publishes for that workspace.
    Workspace(String),
    /// The workspace the CREDENTIAL names. A request to the announced endpoint is addressed
    /// to this emulator's own `host[:port]`, which names no workspace, so the bearer key it
    /// presents — minted for ONE workspace — is what names the one it addresses
    /// ([`crate::state::ComAmazonawsGrafanaAwsgrafanacontrolplaneState::workspace_holding_bearer_token`]).
    Credential,
}

/// The parts of one request this plane reads to decide whether it is addressed to a
/// workspace of this service, and to which.
#[derive(Debug, Clone, Copy)]
pub struct Dialled<'a> {
    /// The request's method, which decides the route it names.
    pub method: &'a str,
    /// The `host[:port]` it addressed, as [`addressed_host`] reads it.
    pub host: Option<&'a str>,
    /// Its path, with no query string.
    pub path: &'a str,
    /// Its `Authorization` header, which is where a bearer credential travels.
    pub authorization: Option<&'a str>,
    /// The signing name of THIS service, so a request addressed to a host the front door
    /// resolves to ANOTHER service is left to that service.
    pub this_service: &'a str,
}

/// The path every route of the Grafana HTTP API sits beneath: Grafana serves its whole HTTP
/// API under `/api` (`/api/org`, `/api/datasources`, …), which is what a client splices onto
/// the announced endpoint.
///
/// It is what makes a credential-addressed request recognisable at an address that names no
/// workspace: a request to this emulator's own endpoint under this prefix presenting a
/// BEARER credential — which a SigV4-signed request of any service does not
/// ([`bearer_token`] takes the `Bearer` scheme alone) — is a Grafana request and nothing
/// else.
pub const GRAFANA_API_PREFIX: &str = "/api";

/// Which route of the native API a request named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// `GET /api/org`: the organization of this workspace.
    Org,
    /// A route under the workspace endpoint this emulator does not serve, kept so
    /// the refusal can name what was asked for.
    Unserved { method: String, path: String },
}

/// One rendered answer: JSON, like every Grafana HTTP API response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

/// The content type every answer below carries.
pub const CONTENT_TYPE: &str = "application/json";

/// The request `dialled` names, or `None` when it is not addressed to a workspace of this
/// service at all.
///
/// A request is this data plane's when it addresses a workspace ([`addressed_workspace`]).
/// That is the one thing that identifies such a request: a Grafana client sends no SigV4
/// credential and no `X-Amz-Target`, so it is also what
/// [`crate::service::EmulatedService::can_handle`] claims on.
pub fn route(dialled: &Dialled<'_>) -> Option<Request> {
    let (addressed, route_path) = addressed_workspace(dialled)?;
    let route = if dialled.method.eq_ignore_ascii_case("GET") && route_path == "/api/org" {
        Route::Org
    } else {
        Route::Unserved {
            method: dialled.method.to_string(),
            path: route_path,
        }
    };
    Some(Request { addressed, route })
}

/// HOW a request says which workspace it addresses, and the Grafana route beneath that
/// workspace's endpoint — and `None` for a request that addresses no workspace of this
/// service.
///
/// Three addressings reach one workspace, because a client reaches this emulator either at
/// the address the control plane announced or at a name that carries the workspace itself:
///
/// * the ANNOUNCED endpoint ([`crate::state::workspace_endpoint`]) — this emulator's own
///   `host[:port]`, which names no workspace — under the Grafana HTTP API's own path
///   ([`GRAFANA_API_PREFIX`]), presenting a bearer credential: the credential names the
///   workspace ([`Addressed::Credential`]). Both of those are required, because at such an
///   address nothing else distinguishes a Grafana request from any other request this
///   emulator serves: a request carrying no bearer credential names no workspace, and one
///   outside Grafana's own API path is not a Grafana request. A host Amazon Web Services
///   publishes ANOTHER service's endpoints at is left to that service for the same reason.
///
///   What that leaves unclaimed is every other plane's traffic: a request another service
///   is routed by its SigV4 credential scope, an `X-Amz-Target` or its own endpoint host
///   carries no `Bearer` credential ([`bearer_token`] takes that scheme alone), and a plane
///   of another service that does serve bearer-authenticated requests under the same prefix
///   is served on a SOCKET OF ITS OWN rather than at this endpoint — the Kubernetes API of
///   `service-com-amazonaws-eks-awswesleyfrontend` (`state::kubernetes_api`) and the InfluxDB
///   plane of `service-com-amazonaws-timestreaminfluxdb-amazontimestreaminfluxdb`
///   (`state::influx_data_plane`) both open one through the core, so no request of theirs
///   reaches this reading;
/// * the endpoint PATH ([`crate::state::workspace_at_announced_path`]), with the Grafana
///   route beneath it, for a client that names the workspace in the address;
/// * the host the real service publishes for the workspace
///   ([`crate::state::workspace_id_at_endpoint_host`]), for a client whose own resolver
///   points that name here; such a host carries no path, so the whole path is the route.
fn addressed_workspace(dialled: &Dialled<'_>) -> Option<(Addressed, String)> {
    use ::service_common::data_plane_addressing as addressing;

    if let Some((workspace_id, route)) = crate::state::workspace_at_announced_path(dialled.path)
    {
        return Some((Addressed::Workspace(workspace_id), route));
    }
    if let Some(workspace_id) = dialled
        .host
        .and_then(crate::state::workspace_id_at_endpoint_host)
    {
        return Some((
            Addressed::Workspace(workspace_id),
            dialled.path.to_string(),
        ));
    }
    let on_a_grafana_route = dialled.path == GRAFANA_API_PREFIX
        || dialled
            .path
            .starts_with(&format!("{GRAFANA_API_PREFIX}/"));
    let presents_a_credential = bearer_token(dialled.authorization).is_some();
    let addresses_another_service = dialled.host.is_some_and(|host| {
        addressing::addresses_an_aws_endpoint(host)
            && addressing::service_named_by_host(host)
                .is_some_and(|named| named != dialled.this_service)
    });
    (on_a_grafana_route && presents_a_credential && !addresses_another_service)
        .then(|| (Addressed::Credential, dialled.path.to_string()))
}

/// The authority a request addresses: its `Host` header, or — on HTTP/2 and an
/// absolute-form request line, which carry none — the authority of its URI.
///
/// Each source is read in that order and the first one that names an address wins,
/// so a `Host` header carrying nothing leaves the URI's authority still to be
/// read. It is what [`route`] matches a workspace endpoint against.
pub fn addressed_host<'a>(
    host_header: Option<&'a str>,
    uri_authority: Option<&'a str>,
) -> Option<&'a str> {
    [host_header, uri_authority]
        .into_iter()
        .flatten()
        .find(|value| !value.is_empty())
}

/// The path of a URI that may be absolute or origin-form.
///
/// Parsed by the `http` crate, which is what the rest of this service's routing
/// reads a URI with (`can_handle` uses `parts.uri.path()`), so an absolute form
/// with no path, userinfo, or a fragment is split by the parser rather than by
/// hand. A request target the parser does not accept falls back to the raw string
/// with its query and fragment cut off, so such a request is still routed by its
/// path rather than silently unclaimed.
pub fn uri_path(uri: &str) -> String {
    if let Ok(parsed) = uri.parse::<::http::Uri>() {
        return parsed.path().to_string();
    }
    let without_fragment = uri.split_once('#').map_or(uri, |(before, _)| before);
    without_fragment
        .split_once('?')
        .map_or(without_fragment, |(path, _)| path)
        .to_string()
}

/// Serve one native-API request from this service's own state.
///
/// `authorization` is the request's `Authorization` header, which is where the
/// bearer key travels. `now` is the instant the request is served at: it settles
/// what the clock owes the workspace (an expired API key is gone, a due status
/// change has happened) before anything is read, so a key past its
/// `secondsToLive` authenticates nothing even if no control-plane call has run
/// since it expired.
pub fn serve(
    store: &mut Store,
    request: &Request,
    authorization: Option<&str>,
    now: SystemTime,
) -> Response {
    let presented = bearer_token(authorization);
    let workspace_id = match &request.addressed {
        Addressed::Workspace(id) => id.clone(),
        // The address named no workspace, so the credential does: a bearer key that names
        // none authenticates nothing, which is the same answer a wrong key gets below.
        Addressed::Credential => {
            let Some(presented) = presented.as_deref() else {
                return json(401, UNAUTHORIZED);
            };
            match store.workspace_holding_bearer_token(presented, now) {
                Some(id) => id,
                None => return json(401, UNAUTHORIZED),
            }
        }
    };
    let Some(workspace) = store.settled_mut(&workspace_id, now) else {
        // The address exists (it is this emulator's own), but no workspace
        // answers on it: one that was never created, or has been deleted.
        return json(
            404,
            &format!("no workspace {workspace_id} is served at this endpoint"),
        );
    };

    // A workspace announces its endpoint from the moment it is created, but its
    // Grafana server is running only while the workspace is ACTIVE — not before it
    // is up, and not once a deletion has been accepted for it.
    if workspace.status != ACTIVE {
        return json(
            503,
            &format!(
                "workspace {} is {} and its Grafana HTTP API is not serving",
                workspace.id, workspace.status
            ),
        );
    }

    let Some(presented) = presented else {
        return json(401, UNAUTHORIZED);
    };
    if !workspace.authorize_bearer_digest(&crate::state::bearer_token_digest(&presented), now) {
        return json(401, UNAUTHORIZED);
    }

    match &request.route {
        Route::Org => org_response(workspace),
        // The caller authenticated; the route is simply not written yet. 501
        // rather than an empty 200, for the reason `crate::scaffolded` gives.
        Route::Unserved { method, path } => {
            // A request to the endpoint itself carries no route path at all; it
            // still names the Grafana API's root.
            let path = if path.is_empty() { "/" } else { path.as_str() };
            json(
                501,
                &format!(
                    "{method} {path} of the Grafana HTTP API is not implemented in this \
                     emulator"
                ),
            )
        }
    }
}

/// The bearer token an `Authorization` header presents, or `None` when it carries
/// no non-empty `Bearer` credential.
fn bearer_token(authorization: Option<&str>) -> Option<String> {
    let value = authorization?.trim();
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("Bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then(|| token.to_string())
}

/// The message the Grafana HTTP API states a refused credential in: a request with no
/// bearer key, with one no workspace of this emulator holds, or with one that is not a live
/// credential of the workspace it addressed, is answered 401 and nothing is said about which
/// of those it was.
const UNAUTHORIZED: &str = "Unauthorized";

/// `GET /api/org`: the organization of this workspace, read off the workspace
/// record.
///
/// A workspace is one Grafana server with one organization
/// ([`WORKSPACE_ORG_ID`]), and the organization is the workspace — so its name is
/// the workspace's `name`, or the workspace id when the create named none
/// (`CreateWorkspaceRequest$workspaceName` is optional).
fn org_response(workspace: &Workspace) -> Response {
    let name = workspace.name.clone().unwrap_or_else(|| workspace.id.clone());
    let body = ::serde_json::json!({
        "id": WORKSPACE_ORG_ID,
        "name": name,
    });
    Response {
        status: 200,
        body: body.to_string().into_bytes(),
    }
}

/// One `{"message": ...}` answer, the envelope the Grafana HTTP API states a
/// refusal in.
fn json(status: u16, message: &str) -> Response {
    let body = ::serde_json::json!({ "message": message });
    Response {
        status,
        body: body.to_string().into_bytes(),
    }
}
