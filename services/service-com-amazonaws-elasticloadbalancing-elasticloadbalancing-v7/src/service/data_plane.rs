// SPDX-License-Identifier: CC0-1.0

//! The Classic load balancer DATA plane: how a request a client addresses to the name a
//! load balancer published is recognised, and which registered instance it is sent to.
//!
//! A load balancer publishes the address a client sends traffic to —
//! `CreateAccessPointOutput$DNSName`, "the DNS name of the load balancer", reported again on
//! every `LoadBalancerDescription` — and that address is NOT this API's endpoint: its first
//! label names no service, so the emulator's front door can resolve no target service from
//! such a request and answered `404 UnknownService` for the one address this service told
//! the caller to use. So the address a load balancer publishes is a socket of this
//! emulator's own ([`crate::state::LoadBalancerEndpoint`]) and this module is what answers
//! on it: the claim ([`claims_parts`], read by `Service::can_handle`) and the serving
//! ([`DataPlane`], handed back by `Service::override_operation`, which the router reaches
//! before authentication — which is what lets a load balancer's own traffic, carrying no
//! AWS credential, be answered at all).
//!
//! # What a request addresses
//!
//! Two addressings resolve to one load balancer, and each is checked against the records
//! this service holds, so one request still has one claimant:
//!
//! * HOST-STYLE, the addressing the published `DNSName` is for: the request's host IS the
//!   name a load balancer of this store published;
//! * PREFIXED PATH-STYLE: the plane is named by the reserved namespace segment
//!   ([`EMULATOR_PATH_NAMESPACE`]) and [`DATA_PLANE_SEGMENT`], then the REGION and the name
//!   of the load balancer, then the request path —
//!   `/_aws/elasticloadbalancing-classic/{region}/{name}/{path}`. A Classic load balancer's
//!   name is unique within one region only, so the region is part of the addressing: a name
//!   alone would let a request addressed to one load balancer be served out of a same-named
//!   load balancer of another region. The segment begins with `_`, which no modelled path
//!   segment of any service does, so the prefix collides with no control-plane route, and a
//!   path under that namespace naming ANOTHER plane is that plane's, never this one's. It is
//!   how the socket a load balancer's own address is served on hands the request it accepted
//!   to this plane ([`super::listener_socket`]).
//!
//! A request presenting what an AWS API call presents (a SigV4 `Authorization` header) is a
//! control-plane call and is never taken by the host-style addressing: a client of a
//! published DNS name has no AWS credential to sign with.
//!
//! # What it answers
//!
//! Which instance a request is sent to is decided from what the control plane persisted, in
//! the order the service's own documentation states it:
//!
//! * the LISTENER the request reached — the one on the port the client addressed, and
//!   otherwise the load balancer's lowest-numbered listener of the kind the connection
//!   carried ([`listener_addressed`]);
//! * the instances it may be sent to: a registered instance takes traffic only while this
//!   load balancer's health checks report it `InService`, and — while
//!   `CrossZoneLoadBalancing` is off — only while it is in the zone the node that took the
//!   request is in ([`eligible_instances`]);
//! * WHICH of them: the one the client's `AWSELB` pin names where the listener's policies
//!   pin a session to an instance ([`super::stickiness`]), and otherwise one DRAWN per
//!   request, which is what spreads a cookie-less client's requests across them.
//!
//! The health checks the CLOCK has already made due are run before the answer, exactly as
//! `DescribeInstanceHealth` runs them: a check was owed at that instant whether or not
//! anybody called, so which instances are in service does not depend on who read it. Beyond
//! that, this module writes NOTHING: serving traffic moves no status, consumes no quota and
//! bumps no counter.
//!
//! What it does NOT do is invent the instance's own answer. A registered instance is an EC2
//! instance id, and no instance of this emulator runs a server on a port, so the connection
//! the load balancer makes to the instance it selected cannot be established — which is a
//! condition Elastic Load Balancing itself words a status for
//! ([`INSTANCE_UNREACHABLE_STATUS`]), as is having no instance to send the request to at all
//! ([`NO_IN_SERVICE_INSTANCE_STATUS`]). Neither a body nor a status is invented for a
//! backend that never ran.

use ::std::time::SystemTime;

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use ::aws_smithy_types::body::SdkBody;
// How a client ADDRESSES a data plane, from the one reading of it in the tree: the reserved
// path namespace, the first path segment, the host without its port and the port a client
// addressed.
use ::service_common::data_plane_addressing::{
    addressed_port, first_segment, host_only, EMULATOR_PATH_NAMESPACE,
};
use ::smithy_server_runtime::{http, BoxFuture, ErasedOperation, OperationContext, Service};

use super::stickiness::{self, Stickiness};
use crate::state::{service_state, ListenerEntry, LoadBalancer, STATE_IN_SERVICE};

/// The segment that names THIS service's data plane behind [`EMULATOR_PATH_NAMESPACE`].
///
/// The `elasticloadbalancing` prefix alone is the endpoint prefix BOTH Elastic Load
/// Balancing models publish, and the ELBv2 crate's plane already answers under it; this
/// model's own plane is named apart from it so one request under the reserved namespace has
/// one claimant.
pub const DATA_PLANE_SEGMENT: &str = "elasticloadbalancing-classic";

/// The PREFIXED PATH-STYLE address of one load balancer's plane:
/// `/_aws/elasticloadbalancing-classic/{region}/{name}`, which a request path follows.
///
/// The one place that addressing is WRITTEN, beside [`prefixed_addressing`], which is the
/// one place it is read: the socket a load balancer's own address is served on
/// ([`super::listener_socket`]) hands the request it accepts to this plane under it, so the
/// two cannot drift on how a plane is named.
pub fn data_plane_path(region: &str, load_balancer_name: &str) -> String {
    format!("/{EMULATOR_PATH_NAMESPACE}/{DATA_PLANE_SEGMENT}/{region}/{load_balancer_name}")
}

/// The listener protocols that carry a request sent IN THE CLEAR, from
/// `Listener$Protocol`'s own documentation ("HTTP, HTTPS, TCP, or SSL").
const CLEARTEXT_PROTOCOLS: &[&str] = &["HTTP", "TCP"];

/// And the two that carry one inside TLS, which is what a client dialling `https://` the
/// published address opens.
const ENCRYPTED_PROTOCOLS: &[&str] = &["HTTPS", "SSL"];

/// The status a request earns when the load balancer has no registered instance it may send
/// it to: none registered at all, none its health checks report `InService`, or — while
/// cross-zone load balancing is off — none in the zone of the node that took the request.
///
/// From the Classic Load Balancer's own troubleshooting documentation for `HTTP 503:
/// Service unavailable`: the load balancer has insufficient capacity to serve the request,
/// which is the condition a load balancer with no in-service instance is in.
pub const NO_IN_SERVICE_INSTANCE_STATUS: u16 = 503;

/// The status a request earns once the load balancer HAS selected a registered instance for
/// it and the connection to that instance cannot be established.
///
/// From the Classic Load Balancer's own troubleshooting documentation for `HTTP 504: Gateway
/// Timeout`: the load balancer failed to establish a connection to the registered instance.
/// Every backend this emulator holds is a registration — an EC2 instance id — with no server
/// of this emulator's listening on the instance port, so this is what a request the load
/// balancer did select an instance for is answered with, rather than a body invented for a
/// backend that never ran.
pub const INSTANCE_UNREACHABLE_STATUS: u16 = 504;

/// The status a request this plane resolved no load balancer for is answered with. The
/// claim and the answer resolve by the one rule ([`resolve`]), so this is reachable only for
/// a load balancer deleted between the claim and the answer.
const NOT_SERVED_STATUS: u16 = 404;

/// What a request RESOLVES to: the load balancer it addresses, and the port the client
/// addressed — which is what decides the listener it reached.
#[derive(Debug)]
struct Resolved {
    region: String,
    load_balancer_name: String,
    addressed_port: Option<i32>,
}

/// What a data-plane request ADDRESSES, as the wire carries it: the host it was sent to, its
/// path, and whether it presents what an AWS API call presents.
///
/// Built from BOTH shapes the router hands this module — the buffered parts the claim reads
/// and the runtime-API request the serving seam reads — so one rule resolves both and the
/// claim cannot resolve a different load balancer than the answer does.
#[derive(Debug)]
struct Addressed<'a> {
    host: Option<&'a str>,
    path: &'a str,
    /// Whether the request is signed as an AWS API call: a client of the address a load
    /// balancer publishes presents no AWS credential, and a request that does is a
    /// control-plane call of this shared endpoint.
    signed: bool,
}

/// The authentication scheme an AWS API call signs with, which a load balancer's own
/// traffic never carries.
const SIGV4_SCHEME: &str = "AWS4-HMAC-SHA256";

/// Whether an `Authorization` header is an AWS SigV4 signature.
fn sigv4_signed(authorization: Option<&str>) -> bool {
    authorization.is_some_and(|value| {
        value
            .get(..SIGV4_SCHEME.len())
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case(SIGV4_SCHEME))
    })
}

impl<'a> Addressed<'a> {
    /// The addressing of the buffered request these parts came from.
    fn of_parts(parts: &'a http::request::Parts) -> Self {
        Self {
            host: parts
                .headers
                .get("host")
                .and_then(|host| host.to_str().ok())
                .or_else(|| parts.uri.authority().map(http::uri::Authority::as_str)),
            path: parts.uri.path(),
            signed: sigv4_signed(
                parts
                    .headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok()),
            ),
        }
    }

    /// The addressing of a runtime-API request, whose URI arrives as a string and is parsed
    /// by the `http` crate (`uri`) rather than split by hand.
    fn of_request(req: &'a RtRequest, uri: &'a http::Uri) -> Self {
        Self {
            host: req
                .headers()
                .get("host")
                .or_else(|| uri.authority().map(http::uri::Authority::as_str)),
            path: uri.path(),
            signed: sigv4_signed(req.headers().get("authorization")),
        }
    }
}

/// The request URI of a runtime-API request, parsed by the `http` crate.
///
/// A URI this parse rejects is one the server would not have accepted, and it resolves to
/// `http::Uri`'s own default (`/`), which names no load balancer and claims nothing.
fn parsed_uri(uri: &str) -> http::Uri {
    uri.parse::<http::Uri>().unwrap_or_default()
}

/// The region, the load balancer name and the request path of a PREFIXED PATH-STYLE
/// addressing: `/_aws/elasticloadbalancing-classic/{region}/{name}/{path}`, by exact segment
/// equality.
fn prefixed_addressing(path: &str) -> Option<(&str, &str, &str)> {
    let (namespace, rest) = first_segment(path)?;
    if namespace != EMULATOR_PATH_NAMESPACE {
        return None;
    }
    let (segment, rest) = first_segment(rest)?;
    if segment != DATA_PLANE_SEGMENT {
        return None;
    }
    let (region, rest) = first_segment(rest)?;
    let (named, rest) = first_segment(rest)?;
    (!region.is_empty() && !named.is_empty()).then_some((region, named, rest))
}

/// Whether a published `DNSName` is the address `addressed` names.
///
/// A published name that carries a PORT is the address of one socket, so the port is part of
/// it; one that carries none is a host, and the port a client addressed it on is not. A DNS
/// name is case-insensitive either way.
fn addresses_published_name(dns_name: &str, addressed: &str) -> bool {
    if addressed_port(Some(dns_name)).is_some() {
        return dns_name.eq_ignore_ascii_case(addressed);
    }
    dns_name.eq_ignore_ascii_case(host_only(addressed))
}

/// Which load balancer `addressed` addresses, or `None` when this plane serves the request
/// not at all. The two addressings are the module documentation's, in that order.
fn resolve<Eng: Send + Sync + 'static>(
    engine: &Eng,
    addressed: &Addressed<'_>,
) -> Option<Resolved> {
    let state = service_state(engine)?;
    let store = state.lock();

    // PREFIXED PATH-STYLE, the one addressing that is explicit about naming this plane, and
    // so the one read whatever the request's host is and whatever it presents. The REGION is
    // part of it, and the load balancer is resolved in that region alone: a Classic load
    // balancer's name is unique per region, so a lookup by name across regions could answer
    // a request out of a same-named load balancer the client never addressed.
    if let Some((region, named, _path)) = prefixed_addressing(addressed.path) {
        let record = store.load_balancers_in(region)?.get(named)?;
        return Some(Resolved {
            region: region.to_string(),
            load_balancer_name: record.name.clone(),
            addressed_port: addressed_port(addressed.host),
        });
    }
    // A path under the reserved namespace that did NOT name this plane names another one,
    // which serves its own requests: reading it as this load balancer's would make two
    // services claim one request, which the router answers `500 AmbiguousService`.
    if first_segment(addressed.path).is_some_and(|(first, _)| first == EMULATOR_PATH_NAMESPACE) {
        return None;
    }
    // HOST-STYLE: the published `DNSName` itself, for a client that dialled it.
    if addressed.signed {
        return None;
    }
    let host = addressed.host?;
    let (region, load_balancer_name) = store.regions.iter().find_map(|(region, records)| {
        records
            .values()
            .find(|record| addresses_published_name(&record.dns_name, host))
            .map(|record| (region.clone(), record.name.clone()))
    })?;
    Some(Resolved {
        region,
        load_balancer_name,
        addressed_port: addressed_port(Some(host)),
    })
}

/// Whether this data plane serves the request these `parts` came from — read by
/// `Service::can_handle`, so a request addressed to a load balancer this service holds
/// reaches this service instead of the front door's `UnknownService`.
pub fn claims_parts<Eng: Send + Sync + 'static>(
    engine: &Eng,
    parts: &http::request::Parts,
) -> bool {
    resolve(engine, &Addressed::of_parts(parts)).is_some()
}

/// [`claims_parts`] for the runtime-API request the serving seam is handed, resolved by the
/// same rule.
pub fn claims_request<Eng: Send + Sync + 'static>(engine: &Eng, req: &RtRequest) -> bool {
    let uri = parsed_uri(req.uri());
    resolve(engine, &Addressed::of_request(req, &uri)).is_some()
}

/// The listener a request on `addressed_port` reached: the one listening on that port and,
/// where that port names none of them — a client reaching the published address addresses
/// the SOCKET's port, not the load balancer's — the lowest-numbered one.
///
/// Only a listener whose protocol carries traffic of the kind the connection carried is
/// considered, so an `https://` client reaches an HTTPS or SSL listener and a client in the
/// clear an HTTP or TCP one.
fn listener_addressed<'r>(
    record: &'r LoadBalancer,
    addressed_port: Option<i32>,
    protocols: &[&str],
) -> Option<&'r ListenerEntry> {
    let mut listeners: Vec<&ListenerEntry> = record
        .listeners
        .iter()
        .filter(|listener| protocols.contains(&listener.protocol.as_str()))
        .collect();
    listeners.sort_by_key(|listener| listener.load_balancer_port);
    addressed_port
        .and_then(|port| {
            listeners
                .iter()
                .copied()
                .find(|listener| listener.load_balancer_port == port)
        })
        .or_else(|| listeners.first().copied())
}

/// The registered instances this load balancer may send the request to, in registration
/// order.
///
/// Two rules narrow the registered set, both the service's own:
///
/// * health — an instance takes traffic only while this load balancer's health checks
///   report it `InService`, which is the one state in which Elastic Load Balancing routes
///   requests to a registered instance;
/// * placement — `CrossZoneLoadBalancing$Enabled` is what says whether a load balancer NODE
///   spreads requests beyond its own Availability Zone. This emulator publishes ONE address
///   for a load balancer, so a request reaches the one node, which is the node in the first
///   of the zones the load balancer is in; with the attribute off, only the in-service
///   instances EC2 reports in that zone receive requests, and with it on, every in-service
///   instance does, whichever zone it is in.
fn eligible_instances(
    record: &LoadBalancer,
    zones: &::std::collections::BTreeMap<String, String>,
) -> Vec<String> {
    let node_zone = record.zones().into_iter().next();
    record
        .instances
        .iter()
        .filter(|instance_id| record.instance_health(instance_id).state == STATE_IN_SERVICE)
        .filter(|instance_id| {
            if record.attributes.cross_zone_load_balancing_enabled {
                return true;
            }
            match (&node_zone, zones.get(instance_id.as_str())) {
                // The node's zone and the instance's, both known: the node serves its own.
                (Some(node_zone), Some(instance_zone)) => node_zone == instance_zone,
                // A load balancer in no zone at all has no node to take the request, and an
                // instance EC2 reports no zone for is in none this node serves.
                _ => false,
            }
        })
        .cloned()
        .collect()
}

/// What this plane answers one request with: which registered instance the load balancer
/// selected for it, and the stickiness cookie the answer hands the client back.
#[derive(Debug)]
struct Answered {
    /// The instance the load balancer selected, or `None` when it had none to select.
    instance_id: Option<String>,
    /// The `Set-Cookie` field pinning the client to that instance, where the listener's
    /// policies mint one for this request.
    set_cookie: Option<String>,
}

/// Which instance the load balancer selects for `request_cookie`, and the cookie the answer
/// carries, out of the record the control plane persisted.
fn answered(
    record: &LoadBalancer,
    listener: Option<&ListenerEntry>,
    zones: &::std::collections::BTreeMap<String, String>,
    request_cookie: Option<&str>,
    drawn: f64,
) -> Answered {
    // A load balancer with no listener of the kind the connection carried has nothing
    // listening for the request at all, so there is no instance to send it to.
    let Some(listener) = listener else {
        return Answered {
            instance_id: None,
            set_cookie: None,
        };
    };
    let eligible = eligible_instances(record, zones);
    let stickiness = stickiness::listener_stickiness(record, listener);
    // The pin the client carried back, where the listener pins sessions at all and the
    // instance it names is still one the load balancer may send requests to. An instance
    // that has left that set takes no traffic for a pin either: the session is re-pinned.
    let pinned = (stickiness != Stickiness::None)
        .then(|| {
            let token = stickiness::pinned_token(request_cookie)?;
            eligible
                .iter()
                .find(|instance_id| {
                    stickiness::instance_token(&record.name, instance_id) == token
                })
                .cloned()
        })
        .flatten();
    // One draw per request, which is what spreads a client that carries no pin across the
    // instances the load balancer may send requests to. Nothing of the draw is written: it
    // is a choice made for this request alone.
    let selected = pinned.clone().or_else(|| {
        let index = (drawn.clamp(0.0, 1.0) * eligible.len() as f64) as usize;
        eligible
            .get(index.min(eligible.len().saturating_sub(1)))
            .cloned()
    });
    let Some(instance_id) = selected else {
        return Answered {
            instance_id: None,
            set_cookie: None,
        };
    };
    // The cookie the answer hands back names the instance this request was SELECTED for,
    // which is what the policy governs. A client that carried a pin already holds it, and an
    // application-controlled policy mints one only on "a new application cookie" in the
    // application's own response, which a load balancer with no reachable backend server
    // receives none of.
    let set_cookie = match &stickiness {
        Stickiness::Duration {
            expiration_period_seconds,
        } if pinned.is_none() => Some(stickiness::set_cookie(
            &stickiness::instance_token(&record.name, &instance_id),
            *expiration_period_seconds,
        )),
        _ => None,
    };
    Answered {
        instance_id: Some(instance_id),
        set_cookie,
    }
}

/// Serve one request addressed to a load balancer of this store.
async fn serve<Eng: Send + Sync + 'static>(engine: &Eng, req: &RtRequest) -> RtResponse {
    let uri = parsed_uri(req.uri());
    let Some(resolved) = resolve(engine, &Addressed::of_request(req, &uri)) else {
        return rendered(NOT_SERVED_STATUS, None);
    };
    let Some(state) = service_state(engine) else {
        return rendered(NOT_SERVED_STATUS, None);
    };
    let encrypted = req
        .headers()
        .get(super::listener_socket::ENCRYPTED_CONNECTION_HEADER)
        .is_some();
    let protocols = if encrypted {
        ENCRYPTED_PROTOCOLS
    } else {
        CLEARTEXT_PROTOCOLS
    };

    // Which instances this load balancer checks, read and RELEASED before EC2 is asked:
    // this service's guard is never held while another service's store is read.
    let registered: ::std::collections::BTreeSet<String> = {
        let guard = state.lock();
        let Some(record) = guard
            .load_balancers_in(&resolved.region)
            .and_then(|records| records.get(&resolved.load_balancer_name))
        else {
            return rendered(NOT_SERVED_STATUS, None);
        };
        record.instances.iter().cloned().collect()
    };
    // What one check of each of them observes, and which zone EC2 reports each of them in:
    // both are facts EC2 owns.
    let observed = crate::state::observe_instances(engine, &registered);
    let zones = crate::state::observe_instance_zones(engine, &registered);

    let now = SystemTime::now();
    let drawn = ::rand::random::<f64>();
    let answer = {
        let mut guard = state.lock();
        let Some(record) = guard
            .load_balancers_in_mut(&resolved.region)
            .and_then(|records| records.get_mut(&resolved.load_balancer_name))
        else {
            return rendered(NOT_SERVED_STATUS, None);
        };
        // The health checks the clock had already made due at `now` — owed at that instant
        // whether or not anybody called — which is what decides who takes traffic.
        record.settle_instance_health(&observed, now);
        let record = &*record;
        let listener = listener_addressed(record, resolved.addressed_port, protocols);
        answered(
            record,
            listener,
            &zones,
            req.headers().get(stickiness::COOKIE_HEADER),
            drawn,
        )
    };

    // The record of one request this address answered, as a load balancer's own access log
    // carries it. Reported through the emulator's own logging facility, so a client can see
    // which plane claimed a request it believed went elsewhere.
    let status = match &answer.instance_id {
        Some(_) => INSTANCE_UNREACHABLE_STATUS,
        None => NO_IN_SERVICE_INSTANCE_STATUS,
    };
    ::tracing::info!(
        load_balancer = %resolved.load_balancer_name,
        region = %resolved.region,
        method = %req.method(),
        path = %uri.path(),
        instance = answer.instance_id.as_deref().unwrap_or("none"),
        status,
        "the load balancer answered a request on the name it published"
    );
    rendered(status, answer.set_cookie.as_deref())
}

/// One rendered answer: the status the load balancer itself produced, and the stickiness
/// cookie it hands back with it.
fn rendered(status: u16, set_cookie: Option<&str>) -> RtResponse {
    let mut response = RtResponse::new(
        StatusCode::try_from(status).unwrap_or_else(|_| {
            StatusCode::try_from(NO_IN_SERVICE_INSTANCE_STATUS).expect("503 is a status")
        }),
        SdkBody::empty(),
    );
    if let Some(field) = set_cookie {
        if let Err(refused) = response
            .headers_mut()
            .try_insert("set-cookie", field.to_string())
        {
            ::tracing::warn!(
                error = %refused,
                "the stickiness cookie cannot be sent on the wire and is left off the answer"
            );
        }
    }
    let _ = response.headers_mut().try_insert("content-length", "0");
    response
}

/// The data plane as the ROUTER serves it: the operation `Service::override_operation` hands
/// back for a request addressed to a load balancer this service holds.
///
/// A load balancer's own traffic is no modelled operation — no routing key names it — so it
/// is mounted the way an emulated data plane is: claimed by `Service::can_handle`
/// ([`claims_parts`]) and served here. It declares NO auth scheme, because a client of the
/// name a load balancer publishes carries no AWS credential and has none to sign with, and
/// it still claims only a request addressed to a load balancer this store holds.
pub struct DataPlane<Eng> {
    /// The ONE shared engine this service's operations read and write, so a request served
    /// here sees exactly the configuration the control plane persisted.
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> DataPlane<Eng> {
    /// Mount the plane over the shared engine.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }

    /// Whether this plane serves `req`, by the one rule the claim uses
    /// ([`claims_request`]).
    pub fn claims(&self, req: &RtRequest) -> bool {
        claims_request(&*self.engine, req)
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for DataPlane<Eng> {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        req: &'r RtRequest,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move { serve(&*self.engine, req).await })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // A client that sends traffic to the name a load balancer published presents no AWS
        // credential: this plane serves the request however it arrived.
        Some(&[])
    }
}

#[cfg(test)]
mod tests {
    //! The pure readings of this module. What the plane SERVES is pinned end to end over
    //! the wire in `tests/all/classic_data_plane.rs`.

    use super::*;

    /// One row of the addressing table: the path, and what it names — the region, the load
    /// balancer and the request path within its plane — or nothing.
    type AddressingRow = (&'static str, Option<(&'static str, &'static str, &'static str)>);

    /// A prefixed addressing names this plane, the region, the load balancer and the request
    /// path, and nothing else is read as one.
    #[test]
    fn a_prefixed_addressing_names_this_plane_and_one_load_balancer() {
        // The rows span what the reading generalises over: the plane's own prefix with and
        // without a path, a deeper path, another plane's prefix, the namespace alone, a path
        // naming no load balancer, one naming no region, and a path outside the namespace.
        let cases: &[AddressingRow] = &[
            (
                "/_aws/elasticloadbalancing-classic/us-east-1/web-elb/",
                Some(("us-east-1", "web-elb", "/")),
            ),
            (
                "/_aws/elasticloadbalancing-classic/us-east-1/web-elb",
                Some(("us-east-1", "web-elb", "/")),
            ),
            (
                "/_aws/elasticloadbalancing-classic/eu-west-1/web-elb/a/b",
                Some(("eu-west-1", "web-elb", "/a/b")),
            ),
            ("/_aws/elasticloadbalancing/us-east-1/web-elb/", None),
            ("/_aws/elasticloadbalancing-classic/us-east-1/", None),
            ("/_aws/elasticloadbalancing-classic/web-elb", None),
            ("/_aws", None),
            ("/web-elb/", None),
        ];
        for (path, expected) in cases {
            let read = prefixed_addressing(path);
            assert_eq!(
                read, *expected,
                "case {path:?}: expected {expected:?}, got {read:?}"
            );
        }
    }

    /// A published name is addressed as it was published: with its port when it carries
    /// one, and by host alone when it does not.
    #[test]
    fn a_published_name_is_addressed_as_it_was_published() {
        // The rows span the two shapes a published `DNSName` takes — the `host:port` of the
        // socket this emulator answers on, and the name Elastic Load Balancing's own zone
        // would give — against the hosts a client can address each by.
        let cases: &[(&str, &str, bool)] = &[
            ("127.0.0.1:34567", "127.0.0.1:34567", true),
            ("127.0.0.1:34567", "127.0.0.1:9297", false),
            ("127.0.0.1:34567", "127.0.0.1", false),
            (
                "web-elb-1234567890.us-east-1.elb.amazonaws.com",
                "web-elb-1234567890.us-east-1.elb.amazonaws.com",
                true,
            ),
            (
                "web-elb-1234567890.us-east-1.elb.amazonaws.com",
                "WEB-ELB-1234567890.US-EAST-1.ELB.AMAZONAWS.COM:80",
                true,
            ),
            (
                "web-elb-1234567890.us-east-1.elb.amazonaws.com",
                "api-elb-1234567890.us-east-1.elb.amazonaws.com",
                false,
            ),
        ];
        for (published, addressed, expected) in cases {
            let read = addresses_published_name(published, addressed);
            assert_eq!(
                read, *expected,
                "case ({published:?}, {addressed:?}): expected {expected}, got {read}"
            );
        }
    }
}
