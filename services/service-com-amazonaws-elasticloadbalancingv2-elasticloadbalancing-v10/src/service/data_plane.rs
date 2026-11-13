// SPDX-License-Identifier: CC0-1.0

//! The load balancer DATA plane: how a request a client addresses to a load balancer's own
//! name is recognised, and how it is answered out of the listener configuration this service
//! holds.
//!
//! A load balancer this service creates publishes the name a client sends traffic to —
//! `LoadBalancer$DNSName`, "the public DNS name of the load balancer" — and that name is
//! NOT this API's endpoint: its first label is the load balancer's own, so the emulator's
//! front door can resolve no target service from such a request and answers `404
//! UnknownService` for the one address this service told the caller to use. Nothing
//! answered a load balancer's own traffic at all, so a client that polls it waits for an
//! answer that never comes.
//!
//! What an Application Load Balancer answers is decided by the LISTENER the request
//! reaches, out of records the control plane already holds: "rules are evaluated in
//! priority order, from the lowest value to the highest value ... When the conditions for
//! a rule are met, its actions are performed. If the conditions for no rules are met, the
//! actions for the default rule are performed" (`CreateRule`), and a default rule's
//! actions ARE its listener's `DefaultActions`. A Network Load Balancer's listener carries no
//! rules and reads no request out of the connection: it HANDS THE CONNECTION ON to whatever
//! its target group selects, so what answers a request addressed to such a load balancer is
//! the target — and where that target is an Application Load Balancer of this very store (an
//! `alb` target group), that load balancer's own listener. So this module is the second half
//! of [`crate::service::EmulatedService`]: the claim ([`claims_parts`], read by
//! `Service::can_handle`) and the serving ([`serve`], reached from
//! `Service::override_routing_error`, which the router reaches BEFORE authentication —
//! which is what lets a load balancer's own traffic, carrying no AWS credential, be
//! answered at all).
//!
//! Every status, header and byte it answers is computed from what a create or a modify
//! persisted, and serving traffic moves no status, consumes no quota and bumps no counter.
//! The ONE thing it writes is the RECORD of the request it has just answered, where the load
//! balancer's own attributes have it deliver one: its access log ([`super::access_logs`]),
//! which is the load balancer's own account of what it did with a request and is delivered
//! for the same reason the web ACL's log beside it is.
//!
//! # What a request addresses, and how it is resolved
//!
//! A client on the machine this emulator runs on resolves a load balancer's DNS name
//! nowhere at all, so three addressings resolve beside the published one. Each is checked
//! against the records this service holds, so one request still has one claimant:
//!
//! * HOST-STYLE, the addressing `LoadBalancer$DNSName` publishes: the request's host IS
//!   the DNS name of a load balancer this store holds, and the whole path is the request
//!   path;
//! * PREFIXED PATH-STYLE: the data plane is named by the reserved namespace segment
//!   [`EMULATOR_PATH_NAMESPACE`] and [`DATA_PLANE_SEGMENT`], then the load balancer's DNS
//!   name or its name, then the request path —
//!   `/_aws/elasticloadbalancing/{dns-name-or-name}/{path}`. The segment begins with `_`,
//!   which no modelled path segment of any service does, so the prefix collides with no
//!   control-plane route;
//! * PATH-STYLE: the load balancer's DNS name or name is the FIRST PATH SEGMENT and the
//!   request path is what follows it, read only on a host that names NO service (the
//!   emulator's own endpoint), so a first segment that happens to equal a load balancer's
//!   name never takes a request away from the service its host names;
//! * ENDPOINT-ONLY, for a client that carries the request path alone: on a host that names
//!   no service, a store holding exactly ONE load balancer that answers from rules of its own
//!   has no other load balancer the request could be meant for — and the path has to be one
//!   that load balancer has a RULE written for, so a path it does not mention is left to
//!   whatever the request was really addressed to rather than taken from another plane that
//!   reads a host-independent addressing of its own. With more than one such load balancer, or
//!   a path no rule matches, the request must say which — by host, by prefix or by first
//!   segment.
//!
//! The last two are read only for a request that presents none of what an AWS API call
//! presents ([`AWS_API_CALL_HEADERS`], [`AWS_API_CALL_QUERY_PARAMETERS`]): a signed or
//! presigned call on the emulator's own endpoint is a control-plane request, and this plane
//! never takes one.
//!
//! # What it performs, and what it answers instead
//!
//! A `fixed-response` action is answered in full — its status code, its content type and
//! its message body are the response — and so is a `redirect`, whose `Location` is built
//! by substituting the request's own components into the placeholders the action carries.
//!
//! A `forward` to a `lambda` target group IS performed: the registered function is invoked
//! in process and the client is answered from what the run returned, or with the load
//! balancer's own `502` when that answer breaks the response contract
//! ([`super::lambda_target`]).
//!
//! A `forward` to an `alb` target group IS performed too: "alb - Register a single
//! Application Load Balancer as a target" names a load balancer of this very store, so the
//! request is passed on to it and answered by ITS listener — the rules, the actions and the
//! web ACL of the load balancer the connection ended at, which is what a Network Load
//! Balancer in front of an Application one really serves. A chain that leads back to a load
//! balancer already in it is answered as a target that cannot be connected to, and the walk
//! is capped by [`MAX_PASSED_ON_HOPS`] either way.
//!
//! A `forward` to any other target group names targets this emulator has no server behind: a
//! registered target is an instance id or an address, and nothing in this emulator listens
//! on it. Neither a body nor a status is invented for a backend that never ran, so such a
//! forward is answered the way a load balancer answers those two conditions —
//! [`NO_HEALTHY_TARGET_STATUS`] when the group has no target to send the request to, and
//! [`TARGET_UNREACHABLE_STATUS`] when it has one and the connection to it cannot be made.
//!
//! An `authenticate-oidc`, `authenticate-cognito` or `jwt-validation` action needs the
//! identity provider it names, and this emulator makes no outbound connection to one, so
//! such a rule is answered [`ACTION_NOT_PERFORMED_STATUS`] rather than by letting an
//! unauthenticated request through to the action behind it. What AWS answers an
//! unauthenticated client — a 302 to the action's own authorization endpoint — is NOT
//! rendered: the client would be sent to a provider this emulator cannot reach, and the
//! session cookie it would come back with is one nothing here can issue, so the redirect
//! would only move the failure one hop away from its cause.

use std::time::SystemTime;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{http, BoxFuture, ErasedOperation, OperationContext, Service};

use crate::ops::types::{Action, RuleCondition};
use crate::state::{
    service_state, ComAmazonawsElasticloadbalancingv2ElasticloadbalancingV10State as ElbState,
    ListenerRecord, LoadBalancerFamily, LoadBalancerRecord, RegisteredTarget, TargetGroupRecord,
    DEFAULT_FIXED_RESPONSE_CONTENT_TYPE,
};
// How a client ADDRESSES a data plane, from the one reading of it in the tree: the
// reserved path namespace, the first path segment, the host without its port, the port a
// client addressed, and whether a host names a service at all.
use ::service_common::data_plane_addressing::{
    addressed_port, first_segment, host_only, names_a_service, service_named_by_host,
    EMULATOR_PATH_NAMESPACE,
};

/// The segment that names THIS service's data plane behind [`EMULATOR_PATH_NAMESPACE`]:
/// the `arnNamespace`/`endpointPrefix` both Elastic Load Balancing models publish.
const DATA_PLANE_SEGMENT: &str = "elasticloadbalancing";

/// The PREFIXED PATH-STYLE address of one load balancer's plane:
/// `/_aws/elasticloadbalancing/{dns-name-or-name}`, which a request path follows.
///
/// The one place that addressing is WRITTEN, beside [`prefixed_addressing`], which is the
/// one place it is read: the socket a load balancer's own address is served on
/// ([`super::listener_socket`]) hands the request it accepts to this plane under it, so the
/// two cannot drift on how a plane is named.
pub fn data_plane_path(named: &str) -> String {
    format!("/{EMULATOR_PATH_NAMESPACE}/{DATA_PLANE_SEGMENT}/{named}")
}

/// The protocols whose listeners have rules to evaluate and a response to render — the
/// two `ProtocolEnum` values an Application Load Balancer's listeners carry.
const HTTP_PROTOCOLS: &[&str] = &["HTTP", "HTTPS"];

/// The protocols whose listeners PASS A CONNECTION ON rather than reading a request out of
/// it — the `ProtocolEnum` values a Network Load Balancer's listeners carry that carry a TCP
/// stream, and so the request this plane is serving. `UDP`'s datagrams, and the `GENEVE` a
/// Gateway Load Balancer's listeners carry, carry no request of this kind at all.
const TCP_PROTOCOLS: &[&str] = &["TCP", "TCP_UDP", "TLS"];

/// The listener protocols of one FAMILY that carry the request this plane serves: an
/// Application Load Balancer's HTTP listeners, whose rules decide the answer, and a Network
/// Load Balancer's TCP ones, which hand the connection to whatever the listener's target
/// group selects. A Gateway Load Balancer's `GENEVE` listeners carry none, so its load
/// balancers serve nothing here.
fn served_protocols(family: LoadBalancerFamily) -> &'static [&'static str] {
    match family {
        LoadBalancerFamily::Application => HTTP_PROTOCOLS,
        LoadBalancerFamily::Network => TCP_PROTOCOLS,
        LoadBalancerFamily::Gateway => &[],
    }
}

/// The `RuleCondition$Field` values this plane matches on, from that member's own list.
const PATH_PATTERN_FIELD: &str = "path-pattern";
const HOST_HEADER_FIELD: &str = "host-header";
const HTTP_REQUEST_METHOD_FIELD: &str = "http-request-method";
const HTTP_HEADER_FIELD: &str = "http-header";
const QUERY_STRING_FIELD: &str = "query-string";
const SOURCE_IP_FIELD: &str = "source-ip";

/// The `ActionTypeEnum` values that END a rule's action list by answering the client,
/// from `Action$Type`'s own list: the action that renders a response, the one that sends
/// the client elsewhere, and the one that passes the request to a target group.
const FIXED_RESPONSE_ACTION: &str = "fixed-response";
const REDIRECT_ACTION: &str = "redirect";
const FORWARD_ACTION: &str = crate::state::FORWARD_ACTION_TYPE;

/// What a load balancer's own access log records as the action performed for a request its
/// web ACL answered at the edge: the inspection itself, rather than any action of the
/// listener — none of which ran ([`super::access_logs`]).
const WAF_ACTION_RECORD: &str = "waf";

/// The two request headers a load balancer's own access-log record reports: the client's
/// `User-Agent`, and the `X-Amzn-Trace-Id` the request carried — both as the client sent
/// them, and absent from the record where the request carried neither.
const USER_AGENT_HEADER: &str = "user-agent";
const TRACE_ID_HEADER: &str = "x-amzn-trace-id";

/// The status a load balancer answers while it is still coming up: it is publishing a
/// name, and its listeners are not yet serving traffic on it.
const PROVISIONING_STATUS: u16 = 503;

/// The status a `forward` to a target group with no healthy target earns: "if a target
/// group contains no registered targets, or if a target group contains only unhealthy
/// targets, the load balancer returns an HTTP 503".
pub const NO_HEALTHY_TARGET_STATUS: u16 = 503;

/// The status a `forward` whose target cannot be CONNECTED to earns: a load balancer that
/// cannot establish a connection to the target it chose answers HTTP 502. Every target
/// this emulator holds is a registration — an instance id or an address — with no server
/// of this emulator's behind it, so this is what a forward to a healthy target answers
/// rather than a body invented for a backend that never ran.
pub const TARGET_UNREACHABLE_STATUS: u16 = 502;

/// The status an action this emulator cannot perform earns, rather than serving what is
/// behind it: an `authenticate-oidc`, `authenticate-cognito` or `jwt-validation` action
/// has to reach the identity provider it names, and this emulator makes no outbound
/// connection.
pub const ACTION_NOT_PERFORMED_STATUS: u16 = 500;

/// How many times one request may be passed from a load balancer on to a load balancer of
/// its own — a `forward` to an `alb` target group is one such hop
/// ([`Performed::PassedOn`]) — past which this plane stops following the chain: a request
/// that has already been passed on this many times is answered rather than passed on again.
///
/// A chain is followed because each hop is a real registration: what answers at the end of
/// it is the last load balancer's own listener. What ends the walk is a load balancer
/// ALREADY in the chain — a configuration that leads back to one answers nothing, so the
/// request is answered as a target that cannot be connected to the moment it is revisited —
/// and this bound beside it caps the walk itself however long a chain of distinct load
/// balancers a caller configures. Both sit where the next hop is TAKEN rather than where it
/// is configured, because a registration naming a load balancer is not a loop until it is
/// walked.
const MAX_PASSED_ON_HOPS: usize = 4;

/// The `TargetHealthStateEnum` value a target that can be sent requests reports.
const HEALTHY_TARGET_STATE: &str = "healthy";

/// The `TargetHealthStateEnum` value a registration of a target group whose health checks
/// are OFF reports: `unavailable`, "health checks are not enabled for this target group"
/// (`crate::state::target_health`). Nothing is checking such a target, so nothing marks it
/// unhealthy either and the load balancer sends it requests. Only a `lambda` target group
/// reaches it — every other target type is refused a disabled health check at create.
const HEALTH_CHECKS_DISABLED_STATE: &str = "unavailable";

/// The longest condition value this plane matches a request against: `RuleCondition$Values`
/// documents a host name and a path pattern as each "up to 128 characters in length", so a
/// longer one is a value no rule of a load balancer carries. It is matched against nothing
/// rather than walked, which bounds the PATTERN side of every match below.
const MAX_CONDITION_VALUE_CHARS: usize = 128;

/// The longest run of request text one condition is matched against — the path, the host, a
/// header value or a query parameter.
///
/// The emulator's OWN bound, as [`crate::state::LOAD_BALANCER_QUOTA`] is: far above any URL
/// a workflow serves, and what stops one request's matching work, and the characters it
/// materialises to do it, growing with the size of the text the request carries.
const MAX_MATCHED_CHARS: usize = 1024;

/// The character comparisons ONE request's whole rule walk may cost.
///
/// Each match is charged the product of the two lengths before it runs — the bound on the
/// cost of a walk with one backtrack point per `*` — so the work of the request as a WHOLE
/// is bounded, and not merely each value's. The budget is far above what any configuration
/// charges: a listener at the published maximum of
/// [`crate::state::RULES_PER_LOAD_BALANCER`] rules, each carrying a condition value of the
/// full [`MAX_CONDITION_VALUE_CHARS`], matched against a 256-character path, charges about
/// 3.3 million. A walk that spends it stops, and the listener's default actions answer —
/// the same answer a request no rule matches gets.
const MAX_MATCH_STEPS: usize = 4_000_000;

/// The characters ONE request's redirect rendering may produce, across all five components
/// of the `Location` it builds.
///
/// The emulator's OWN bound, as [`MAX_MATCHED_CHARS`] is: far above any URL a redirect
/// sends a client to, and what stops the placeholders a stored template holds multiplying
/// the request's own host, path or query into a response header of the caller's choosing.
/// Every character the rendering produces is charged to it, so nothing past the bound is
/// allocated; a redirect that does not fit is an action this plane could not perform.
const MAX_REDIRECT_RENDER_CHARS: usize = 8 * 1024;

/// What makes a request an AWS API CALL rather than a load balancer's own traffic: an
/// `X-Amz-Target` or the date a signed request carries, the two a PRESIGNED one carries in
/// its query string instead of a header, and an `Authorization` naming the SIGV4 SCHEME.
///
/// The scheme is what is read on that header, not its presence: ordinary traffic through a
/// load balancer carries `Authorization: Basic …` or `Bearer …`, and such a request is the
/// load balancer's to answer.
const AWS_API_CALL_HEADERS: &[&str] = &["x-amz-target", "x-amz-date"];
const AWS_API_CALL_QUERY_PARAMETERS: &[&str] = &["X-Amz-Credential", "X-Amz-Signature"];
const SIGV4_SCHEME: &str = "AWS4-HMAC-SHA256";

/// The header the host a CLIENT addressed travels in, for a request this plane did not
/// receive from that client directly: a load balancer's own socket re-addresses the request
/// to this service's own endpoint so the plane resolves out of the reserved path, and carries
/// the name the client dialled here ([`super::listener_socket`]). The socket SETS it, so a
/// client's own value never travels with the request it forwards.
pub(super) const FORWARDED_HOST_HEADER: &str = "x-forwarded-host";

/// The five placeholders a `redirect` action's components may be written with, from
/// `RedirectActionConfig`: "You can reuse URI components using the following reserved
/// keywords: #{protocol}, #{host}, #{port}, #{path} (the leading "/" is not included) and
/// #{query} (the leading "?" is not included)."
///
/// Named through the ONE reading of those keywords
/// ([`crate::state::rule_actions`]), so the plane that RESOLVES a component, the read that
/// REPORTS an unmodified one and the validation that judges a redirect cannot drift on how
/// a reserved keyword is spelled. The path's is the bare keyword, which this plane
/// substitutes without the leading "/" the member itself carries.
use crate::state::rule_actions::{
    KEEP_HOST as HOST_PLACEHOLDER, KEEP_PATH_KEYWORD as PATH_PLACEHOLDER,
    KEEP_PORT as PORT_PLACEHOLDER, KEEP_PROTOCOL as PROTOCOL_PLACEHOLDER,
    KEEP_QUERY as QUERY_PLACEHOLDER,
};

/// What a data-plane request ADDRESSES, as the wire carries it: the host it was sent to —
/// absent on a request that carries neither a `Host` header nor an absolute URI — its
/// path, and whether it presents what an AWS API call presents.
///
/// Built from BOTH shapes the router hands this module, the buffered parts the claim reads
/// and the runtime-API request the serving seam reads, so one rule resolves both and the
/// claim cannot resolve a different load balancer than the answer does.
#[derive(Debug)]
struct Addressed<'a> {
    host: Option<&'a str>,
    path: &'a str,
    /// What an AWS API call presents ([`AWS_API_CALL_HEADERS`],
    /// [`AWS_API_CALL_QUERY_PARAMETERS`]): a load balancer's own traffic presents none of
    /// it, and a request that does is a control-plane call.
    aws_api_call: bool,
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
            aws_api_call: AWS_API_CALL_HEADERS
                .iter()
                .any(|name| parts.headers.contains_key(*name))
                || sigv4_signed(
                    parts
                        .headers
                        .get("authorization")
                        .and_then(|value| value.to_str().ok()),
                )
                || presigned(parts.uri.query()),
        }
    }

    /// The addressing of a runtime-API request, whose URI arrives as a string and is
    /// parsed by the `http` crate (`uri`) rather than split by hand.
    fn of_request(req: &'a RtRequest, uri: &'a http::Uri) -> Self {
        Self {
            host: req
                .headers()
                .get("host")
                .or_else(|| uri.authority().map(http::uri::Authority::as_str)),
            path: uri.path(),
            aws_api_call: AWS_API_CALL_HEADERS
                .iter()
                .any(|name| req.headers().get(*name).is_some())
                || sigv4_signed(req.headers().get("authorization"))
                || presigned(uri.query()),
        }
    }
}

/// Whether an `Authorization` header names the SigV4 scheme, which is what an AWS API
/// call's credential is presented under — as opposed to the `Basic`/`Bearer` credential
/// ordinary traffic through a load balancer carries.
fn sigv4_signed(authorization: Option<&str>) -> bool {
    authorization.is_some_and(|value| {
        value
            .get(..SIGV4_SCHEME.len())
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case(SIGV4_SCHEME))
    })
}

/// Whether a query string carries the SigV4 scope a PRESIGNED request puts there instead
/// of in a header, which is an AWS API call however little else it presents.
fn presigned(query: Option<&str>) -> bool {
    let Some(query) = query else {
        return false;
    };
    query.split('&').any(|pair| {
        let name = pair.split('=').next().unwrap_or(pair);
        AWS_API_CALL_QUERY_PARAMETERS.contains(&name)
    })
}

/// The request URI of a runtime-API request, parsed by the `http` crate: the authority —
/// absent on the origin-form URI a client sends on the wire — the path and the query.
///
/// A URI this parse rejects is one the server would not have accepted, and it resolves to
/// `http::Uri`'s own default (`/`), which names no load balancer and claims nothing.
fn parsed_uri(uri: &str) -> http::Uri {
    uri.parse::<http::Uri>().unwrap_or_default()
}

/// What a request resolves to: the load balancer it addresses, and the path WITHIN that
/// load balancer's own endpoint — which is what the rules are evaluated against.
#[derive(Debug)]
struct Resolved {
    load_balancer_arn: String,
    request_path: String,
}

/// Whether this data plane serves the request these `parts` came from — read by
/// `Service::can_handle`, so a request addressed to a load balancer this service holds
/// reaches this service instead of the front door's `UnknownService`.
pub fn claims_parts<Eng: Send + Sync + 'static>(engine: &Eng, parts: &http::request::Parts) -> bool {
    resolve(engine, &Addressed::of_parts(parts)).is_some()
}

/// [`claims_parts`] for the runtime-API request the serving seam is handed, resolved by
/// the same rule.
pub fn claims_request<Eng: Send + Sync + 'static>(engine: &Eng, req: &RtRequest) -> bool {
    let uri = parsed_uri(req.uri());
    resolve(engine, &Addressed::of_request(req, &uri)).is_some()
}

/// Which load balancer `addressed` addresses, and the path within its endpoint, or `None`
/// when this data plane serves the request not at all.
///
/// The four addressings are the module documentation's, in that order. Each one resolves
/// only to a load balancer that SERVES TRAFFIC OF THIS KIND ([`serves_traffic`]) — a Gateway
/// Load Balancer's listeners carry no request of it — so the claim and the answer cannot
/// disagree about which requests this plane can answer.
fn resolve<Eng: Send + Sync + 'static>(engine: &Eng, addressed: &Addressed<'_>) -> Option<Resolved> {
    let state = service_state(engine)?;
    let store = state.lock();

    // PREFIXED PATH-STYLE, the one addressing that is explicit about naming this plane,
    // and so the one read whatever the request's host is and whatever it presents.
    if let Some((named, path)) = prefixed_addressing(addressed.path) {
        return resolved(&store, named_load_balancer(&store, named)?, path);
    }
    // A path under the reserved namespace that did NOT name this plane names another one,
    // which serves its own requests: reading it as this load balancer's would make two
    // services claim one request, which the router answers `500 AmbiguousService`.
    if first_segment(addressed.path).is_some_and(|(first, _)| first == EMULATOR_PATH_NAMESPACE) {
        return None;
    }

    let host = addressed.host.map(host_only);

    // HOST-STYLE: the published `DNSName` itself.
    if let Some(addressed_authority) = addressed.host {
        if let Some(record) = store
            .load_balancers
            .values()
            .find(|record| addresses_published_name(&record.dns_name, addressed_authority))
        {
            return resolved(&store, record, addressed.path);
        }
    }

    // The remaining two addressings are the emulator's own endpoint, which is the only
    // address a client on this machine can reach. A host that names some service — this
    // service's own control plane, or another service's endpoint — keeps answering as that
    // service, and a request presenting AWS API credentials is a control-plane call.
    if addressed.aws_api_call || host.is_some_and(names_a_service) {
        return None;
    }

    // PATH-STYLE: the load balancer named by the first path segment.
    if let Some((first, rest)) = first_segment(addressed.path) {
        if let Some(record) = named_load_balancer(&store, first) {
            return resolved(&store, record, rest);
        }
    }

    // ENDPOINT-ONLY: the request path alone, for a client that carries nothing else. It is
    // read only when the store holds exactly ONE load balancer that answers from rules of
    // its own, and only for a path that load balancer has a RULE written for: the other
    // planes that read a host-independent addressing require the path to name something they
    // own (a deployed stage, for one), and a path no rule of the only load balancer mentions
    // is left to whatever the request was really addressed to rather than claimed here — two
    // claimants is what the router answers `500 AmbiguousService`.
    let mut serving = store
        .load_balancers
        .values()
        .filter(|record| answers_from_rules(&store, record));
    let only = serving.next()?;
    if serving.next().is_some() {
        return None;
    }
    if !a_rule_is_written_for(&store, only, addressed.path) {
        return None;
    }
    resolved(&store, only, addressed.path)
}

/// Whether a rule of `record`'s addressed listener is written for this PATH: a
/// `path-pattern` condition of one of its rules matches it.
///
/// This is the one thing the ENDPOINT-ONLY claim asks of a path, and it asks only about
/// the path: the claim is handed no method, no header and no query, so the other condition
/// fields are not read here. A rule whose conditions name no path pattern therefore does
/// not make the claim, and neither does the listener's default action — a client that wants
/// those names the load balancer, by host, by the reserved prefix or by the first segment.
fn a_rule_is_written_for(store: &ElbState, record: &LoadBalancerRecord, path: &str) -> bool {
    let Some(listener) = listener_addressed(store, record, None) else {
        return false;
    };
    let mut budget = MAX_MATCH_STEPS;
    listener.rules.iter().any(|rule| {
        rule.conditions
            .iter()
            .filter(|condition| condition.field.as_deref() == Some(PATH_PATTERN_FIELD))
            .any(|condition| {
                let configured = condition
                    .path_pattern_config
                    .as_ref()
                    .and_then(|config| config.values.as_deref())
                    .unwrap_or_default();
                condition
                    .values
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .chain(configured)
                    .any(|pattern| matches_within(pattern, path, CASE_SENSITIVE, &mut budget))
            })
    })
}

/// One resolution, for a load balancer that serves traffic of this kind; `None` for one that
/// does not, so no addressing can claim a request this plane cannot answer.
fn resolved(store: &ElbState, record: &LoadBalancerRecord, path: &str) -> Option<Resolved> {
    serves_traffic(store, record).then(|| Resolved {
        load_balancer_arn: record.arn.clone(),
        request_path: path.to_string(),
    })
}

/// Whether this load balancer has traffic of this kind to serve: a listener on a protocol
/// that carries the request ([`served_protocols`]) — an Application Load Balancer's HTTP
/// listener, whose rules and actions render the answer, or a Network Load Balancer's TCP
/// one, which hands the connection to whatever its target group selects.
///
/// A Network Load Balancer is reached on the name it publishes exactly as an Application one
/// is: a client sends traffic to `LoadBalancer$DNSName` and the listener on the port it
/// reached decides what happens to it. What differs is what the listener does with the
/// connection — it passes it on rather than answering from rules of its own — which is
/// [`forward`]'s business, not the claim's.
fn serves_traffic(store: &ElbState, record: &LoadBalancerRecord) -> bool {
    listener_addressed(store, record, None).is_some()
}

/// Whether this load balancer answers from RULES of its own: an Application Load Balancer —
/// the family whose listeners carry rules and whose actions render a response — with at
/// least one listener on an HTTP protocol to answer with.
///
/// This is the narrower question the ENDPOINT-ONLY addressing asks, because that addressing
/// is read only for a path one load balancer has a rule written for, and only an HTTP
/// listener carries rules at all.
fn answers_from_rules(store: &ElbState, record: &LoadBalancerRecord) -> bool {
    record.family() == LoadBalancerFamily::Application
        && listener_addressed(store, record, None).is_some()
}

/// Whether the authority a request ADDRESSED is the name a load balancer published.
///
/// A load balancer publishes one of two kinds of `DNSName`, and each is addressed by what
/// it states: the `host:port` of the socket this emulator opened for it names the PORT it
/// answers on, so a request addresses it by the whole authority — the emulator's own
/// endpoint is on the same host at another port, and matching the host alone would take
/// every request to that endpoint away from the service it was meant for — while a name
/// under Elastic Load Balancing's own zone states no port, so a request addresses it by the
/// host alone, whichever of the load balancer's listener ports it dialled. Both are read
/// case-insensitively, as a DNS name compares.
fn addresses_published_name(dns_name: &str, addressed: &str) -> bool {
    if addressed_port(Some(dns_name)).is_some() {
        return dns_name.eq_ignore_ascii_case(addressed);
    }
    dns_name.eq_ignore_ascii_case(host_only(addressed))
}

/// The load balancer `named` names, by its DNS name or by its own name, both read
/// case-insensitively (a DNS name is case-insensitive, and a load balancer's name is
/// unique per Region).
fn named_load_balancer<'s>(store: &'s ElbState, named: &str) -> Option<&'s LoadBalancerRecord> {
    store.load_balancers.values().find(|record| {
        record.dns_name.eq_ignore_ascii_case(named) || record.name.eq_ignore_ascii_case(named)
    })
}

/// The load balancer name and the request path of a PREFIXED PATH-STYLE addressing:
/// `/_aws/elasticloadbalancing/{dns-name-or-name}/{path}`, by exact segment equality.
fn prefixed_addressing(path: &str) -> Option<(&str, &str)> {
    let (namespace, rest) = first_segment(path)?;
    if namespace != EMULATOR_PATH_NAMESPACE {
        return None;
    }
    let (segment, rest) = first_segment(rest)?;
    if segment != DATA_PLANE_SEGMENT {
        return None;
    }
    let (named, rest) = first_segment(rest)?;
    (!named.is_empty()).then_some((named, rest))
}

/// Everything the rules and the actions are evaluated against: what the client sent.
///
/// `pub(super)` because a `forward` to a `lambda` target group hands the request to the
/// function as an event ([`super::lambda_target`]), which is built out of exactly these
/// members.
#[derive(Debug, Default)]
pub(super) struct DataPlaneRequest {
    pub(super) method: String,
    /// The path within the load balancer's own endpoint, as [`Resolved`] read it.
    pub(super) path: String,
    /// The query string, without the leading `?`.
    pub(super) query: String,
    /// The host the client addressed, without its port.
    pub(super) host: String,
    /// Every header, with its name lower-cased: a header condition names its header
    /// case-insensitively.
    pub(super) headers: Vec<(String, String)>,
    /// The port the client addressed, when it named one.
    pub(super) port: Option<i32>,
}

impl DataPlaneRequest {
    /// What the runtime-API request carries, for the path [`resolve`] read out of it and
    /// the `uri` the `http` crate parsed out of it.
    ///
    /// The HOST is the one the CLIENT addressed and the PORT the one the request reached a
    /// listener on, and the two are read apart: a request a load balancer's own socket
    /// accepted is handed to this plane re-addressed to this service's own endpoint carrying
    /// the PORT of the listener the connection reached, with the client's own host carried
    /// beside it ([`FORWARDED_HOST_HEADER`]), so the listener is selected by the port the
    /// socket states while `#{host}` and a `host-header` condition read the name the client
    /// dialled.
    fn of(req: &RtRequest, uri: &http::Uri, path: &str) -> Self {
        let addressed = req
            .headers()
            .get("host")
            .or_else(|| uri.authority().map(http::uri::Authority::as_str));
        let client_host = req
            .headers()
            .get(FORWARDED_HOST_HEADER)
            .filter(|host| !host.trim().is_empty())
            .or(addressed);
        Self {
            method: req.method().to_string(),
            path: path.to_string(),
            query: uri.query().unwrap_or_default().to_string(),
            host: client_host.map(host_only).unwrap_or_default().to_string(),
            headers: req
                .headers()
                .iter()
                .map(|(name, value)| (name.to_ascii_lowercase(), value.to_string()))
                .collect(),
            port: addressed_port(addressed),
        }
    }

    /// The first value of `name`, read case-insensitively.
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(held, _)| held == name)
            .map(|(_, value)| value.as_str())
    }

    /// Carry the headers a web ACL's `CustomRequestHandling` inserted onto the request the
    /// rules are evaluated against and the targets would see.
    ///
    /// An inserted header REPLACES one the client sent under the same name: the prefix WAF
    /// gives it is there so what is behind the edge can trust it came from WAF.
    fn insert_headers(&mut self, inserted: Vec<(String, String)>) {
        for (name, value) in inserted {
            let name = name.to_ascii_lowercase();
            self.headers.retain(|(held, _)| held != &name);
            self.headers.push((name, value));
        }
    }

    /// Read the host this request ADDRESSED as the name the load balancer published, where
    /// the host it arrived under is THIS API's own endpoint rather than an address a client
    /// dialled, and nothing carried the client's own host beside it
    /// ([`FORWARDED_HOST_HEADER`]).
    ///
    /// Every answer that reports the host the client addressed — the `#{host}` a `redirect`
    /// action substitutes, and a `host-header` condition's own subject — is answered from
    /// this one value, so it has to be the name the CLIENT used. A request re-addressed to
    /// this service's own endpoint carries the client's host beside it and is read from that;
    /// one that carries none — an in-process caller of this plane — dialled the address the
    /// load balancer PUBLISHES (`LoadBalancer$DNSName`, "the public DNS name of the load
    /// balancer"), so that is the host it addressed. Every OTHER host is the client's own
    /// addressing — the published name, the emulator's own endpoint, or any name a client
    /// resolves to the load balancer, which is what a `host-header` condition is written
    /// for — and is left exactly as it arrived.
    fn addressed_the_published_name(&mut self, dns_name: &str) {
        let this_api =
            service_named_by_host(&self.host).is_some_and(|named| named == super::ROUTE_KEY);
        if !this_api {
            return;
        }
        self.host = host_only(dns_name).to_string();
    }
}

/// One answer this plane renders: the status, the content type, `Location` and any other
/// headers it carries, and the body.
///
/// `pub(super)` because the answer a `lambda` target's own run produced is one of these
/// ([`super::lambda_target`]).
#[derive(Debug, Default)]
pub(super) struct Served {
    pub(super) status: u16,
    pub(super) content_type: Option<String>,
    pub(super) location: Option<String>,
    /// Every other header the answer carries: what a web ACL's own custom response
    /// configures, and what a `lambda` target's response named. A listener's actions
    /// configure none.
    pub(super) headers: Vec<(String, String)>,
    pub(super) body: Vec<u8>,
    /// What the load balancer OBSERVED while producing this answer, which is what its own
    /// access log records about the request ([`super::access_logs`]).
    pub(super) observed: Observed,
}

/// What answering one request told the load balancer about it: the facts its own access-log
/// record carries that the response itself does not ([`super::access_logs`]).
///
/// Every field is left at its absent value for an answer that has nothing to say about it —
/// a request no target was chosen for has no target group, no target status and no target
/// timing — so nothing is recorded that was not observed.
#[derive(Debug, Default)]
pub(super) struct Observed {
    /// The actions the listener performed, in the order they ran: the record's
    /// `actions_executed`.
    pub(super) actions: Vec<String>,
    /// The target group the request was forwarded to, where it was forwarded at all.
    pub(super) target_group_arn: Option<String>,
    /// The status the TARGET answered, where a target answered one — which a
    /// load-balancer-generated answer never has.
    pub(super) target_status: Option<u16>,
    /// The failure class of an answer the LOAD BALANCER generated itself
    /// ([`super::access_logs::reason`]).
    pub(super) error_reason: Option<String>,
    /// How long the target took, where one was called.
    pub(super) target_elapsed: Option<std::time::Duration>,
}

impl Served {
    /// A status with no body: what a load balancer answers for a request it could not pass
    /// on, where no body of its own is this emulator's to invent.
    pub(super) fn status(status: u16) -> Self {
        Self {
            status,
            ..Self::default()
        }
    }

    /// What the web ACL in front of the load balancer answered, which is served as it
    /// leaves the edge: none of the listener's actions ran.
    fn at_the_edge(answer: super::web_acl::EdgeAnswer) -> Self {
        Self {
            status: answer.status,
            content_type: answer.content_type,
            location: None,
            headers: answer.headers,
            body: answer.body,
            // The web ACL answered, so no action of the listener ran: that is what Elastic
            // Load Balancing records as the action performed for such a request.
            observed: Observed {
                actions: vec![WAF_ACTION_RECORD.to_string()],
                ..Observed::default()
            },
        }
    }
}

/// Answer a request this plane claimed, out of the records the control plane holds.
///
/// Reached from the plane `Service::override_operation` hands back for a request
/// [`claims_request`] resolved, so the resolution here repeats that one; a resolution that
/// has gone away between the two (the load balancer was deleted by another request in
/// between) is a load balancer no longer serving the name, which is [`PROVISIONING_STATUS`]
/// rather than a panic.
///
/// The WEB ACL in front of the load balancer decides the request first ([`super::web_acl`]):
/// a request it refuses is answered at the edge and none of the listener's actions runs,
/// and one it allows is matched and answered carrying the headers it inserted. This
/// service's own store is released before the web ACL is asked, so the two services'
/// stores are never held at the same time.
///
/// `ctx` is what a `forward` to a `lambda` target group needs: the in-process transport the
/// function is invoked over ([`super::lambda_target`]). The store's guard is released before
/// that invocation, so one client's handler cannot stall another client's request.
///
/// A load balancer whose own attribute collection has it drop invalid header fields removes
/// them from the request here, before its listener's rules or its targets see any of it
/// ([`super::invalid_header_fields`]).
///
/// A listener that HANDS THE CONNECTION ON — a `forward` to an `alb` target group — is
/// served by going round again on the load balancer it was handed to: the same request,
/// decided by that load balancer's own web ACL and answered by its own listener, which is
/// what the client would have been answered had it addressed that load balancer directly. A
/// chain that leads back to a load balancer already in it is answered as a target that
/// cannot be connected to, and the walk is capped by [`MAX_PASSED_ON_HOPS`] either way.
pub async fn serve<Eng: Send + Sync + 'static>(
    engine: &Eng,
    req: &RtRequest,
    ctx: &OperationContext<'_>,
) -> RtResponse {
    let uri = parsed_uri(req.uri());
    let Some(resolved) = resolve(engine, &Addressed::of_request(req, &uri)) else {
        return rendered(&Served::status(PROVISIONING_STATUS));
    };
    let mut request = DataPlaneRequest::of(req, &uri, &resolved.request_path);
    let Some(state) = service_state(engine) else {
        return rendered(&Served::status(PROVISIONING_STATUS));
    };
    let now = SystemTime::now();
    // The load balancer serving the request, and the port it was addressed on: the one the
    // client dialled to begin with and, for a request one load balancer passed on to a load
    // balancer of its own, the next one's ([`Performed::PassedOn`]).
    let mut load_balancer_arn = resolved.load_balancer_arn.clone();
    let mut addressed_port = request.port;
    // The listener that took the request, for the record this plane leaves of what answered:
    // set by every iteration before anything reads it.
    let mut listener_arn;
    // The priority of the rule that answered, which the load balancer's own access-log record
    // carries: the last load balancer in the chain is the one that answered, so the last
    // iteration's is the one recorded.
    let mut matched_rule_priority = 0;
    // Every load balancer this request has already been served against, so a chain that
    // leads back to one of them is answered rather than walked again.
    let mut walked: Vec<String> = Vec::new();
    let served = loop {
        // What this service holds about the addressed load balancer and the listener the
        // request reached, under a lock that is RELEASED before anything else is asked.
        //
        // A load balancer the clock still owes its move out of `provisioning` is not serving
        // traffic on the name it published yet. Read, never written: this plane moves no
        // status.
        let (published_name, drops_invalid_header_fields) = {
            let store = state.lock();
            let Some(record) = store.load_balancers.get(&load_balancer_arn) else {
                return rendered(&Served::status(PROVISIONING_STATUS));
            };
            if now < record.active_at {
                return rendered(&Served::status(PROVISIONING_STATUS));
            }
            let Some(listener) = listener_addressed(&store, record, addressed_port) else {
                return rendered(&Served::status(PROVISIONING_STATUS));
            };
            listener_arn = listener.arn.clone();
            (
                record.dns_name.clone(),
                super::invalid_header_fields::drops_invalid_header_fields(record),
            )
        };
        // The host the CLIENT addressed, for a request this plane did not receive from the
        // client directly ([`DataPlaneRequest::addressed_the_published_name`]).
        request.addressed_the_published_name(&published_name);
        // What this load balancer's own attribute collection has it REMOVE from the request
        // before anything behind it sees it ([`super::invalid_header_fields`]): the headers
        // whose names are no valid header field. Each load balancer in a chain reads its
        // own attribute, because the removal is the load balancer's.
        if drops_invalid_header_fields {
            super::invalid_header_fields::dropped(&mut request.headers);
        }

        // The web ACL `AssociateWebACL` bound to this load balancer, which decides the
        // request BEFORE the listener's own action — the load balancer's ARN is the one the
        // binding was written under, so each load balancer in a chain is decided by the one
        // bound to IT.
        let (decided, logged) = super::web_acl::decision(
            engine,
            &load_balancer_arn,
            req,
            &request.path,
            &request.query,
        );
        // What that web ACL LOGS about the request it has just decided, delivered to the
        // destination its own `LoggingConfiguration` names: WAF records the request it
        // inspected, so the record is delivered for a request it refused at the edge as much
        // as for one it let through to the listener — and once per load balancer the request
        // was decided by, each by the ACL bound to that one.
        if let Some(logged) = logged.as_ref() {
            ::service_com_amazonaws_wafv2_awswaf_20190729::service::logs::deliver(ctx, logged)
                .await;
        }
        let (performed, matched_priority) = match decided {
            super::web_acl::Decision::Refused(answer) => break Served::at_the_edge(answer),
            decided => {
                if let super::web_acl::Decision::Allowed(inserted) = decided {
                    request.insert_headers(inserted);
                }
                let store = state.lock();
                let Some(listener) = store.listeners.get(&listener_arn) else {
                    return rendered(&Served::status(PROVISIONING_STATUS));
                };
                answered(&store, listener, &request, now)
            }
        };
        matched_rule_priority = matched_priority;
        match performed {
            Performed::Answered(served) => break served,
            // The store's guard is released above, so the function RUNS with no lock of
            // this service's held.
            Performed::Invoke(forwarded) => {
                // The request's own bytes, which the event the function is invoked with
                // carries. A body this plane cannot read is not a body it may invent an
                // empty one for, so the request reaches no function at all.
                let Some(body) = req.body().bytes() else {
                    tracing::warn!(
                        load_balancer = %load_balancer_arn,
                        "the request carries a body this emulator cannot read, so the \
                         Lambda target was not invoked"
                    );
                    return rendered(&Served::status(super::lambda_target::BAD_GATEWAY_STATUS));
                };
                break super::lambda_target::served(ctx, &forwarded, &request, body).await;
            }
            // The listener handed the connection to a load balancer of its own, which is
            // what answers it: the same request is served against that load balancer's
            // listeners, on the port its registration receives traffic on.
            Performed::PassedOn(passed) => {
                walked.push(load_balancer_arn.clone());
                if walked.contains(&passed.load_balancer_arn) {
                    tracing::warn!(
                        load_balancer = %load_balancer_arn,
                        "the load balancer this one passes the request on to has already \
                         served it, so the chain leads nowhere and is not followed further"
                    );
                    break Served::status(TARGET_UNREACHABLE_STATUS);
                }
                if walked.len() >= MAX_PASSED_ON_HOPS {
                    tracing::warn!(
                        load_balancer = %load_balancer_arn,
                        "the request has been passed from one load balancer to another \
                         {MAX_PASSED_ON_HOPS} times and this one passes it on again, so the \
                         chain is not followed further"
                    );
                    break Served::status(TARGET_UNREACHABLE_STATUS);
                }
                load_balancer_arn = passed.load_balancer_arn;
                addressed_port = passed.port;
            }
        }
    };
    // The record of one request this endpoint answered: which load balancer and listener
    // took it, what was asked, and what was answered. Reported through the emulator's own
    // logging facility, so a client can see which plane claimed a request it believed went
    // elsewhere.
    tracing::info!(
        load_balancer = %load_balancer_arn,
        listener = %listener_arn,
        method = %request.method,
        host = %request.host,
        path = %request.path,
        status = served.status,
        "the load balancer answered a request on the name it published"
    );
    // ... and the record the load balancer's OWN access log carries for it, delivered into
    // the bucket its attributes name ([`super::access_logs`]). A load balancer whose
    // attributes switch logging off delivers nothing, which is their documented default.
    deliver_access_log(
        ctx,
        &state,
        &load_balancer_arn,
        &listener_arn,
        &request,
        req.body().bytes().map_or(0, <[u8]>::len),
        &served,
        matched_rule_priority,
        now,
    )
    .await;
    rendered(&served)
}

/// Deliver the access-log record of one answered request, where the load balancer that
/// answered it has access logging on ([`super::access_logs`]).
///
/// Everything the record carries is read here, under one lock of the store that is released
/// before the delivery is made: the load balancer's own resource id and the address it
/// answers on, the protocol and port of the listener that took the request, and the
/// destination its attributes name. The delivery itself is best effort and changes nothing
/// about the answer the client has already been computed.
#[allow(clippy::too_many_arguments)]
async fn deliver_access_log(
    ctx: &OperationContext<'_>,
    state: &::engine::ServiceState<ElbState>,
    load_balancer_arn: &str,
    listener_arn: &str,
    request: &DataPlaneRequest,
    received_bytes: usize,
    served: &Served,
    matched_rule_priority: i32,
    received_at: SystemTime,
) {
    let Some((destination, load_balancer_id, node_address, protocol, port)) = ({
        let store = state.lock();
        store
            .load_balancers
            .get(load_balancer_arn)
            .and_then(|record| {
                let destination = super::access_logs::destination(record)?;
                let listener = store.listeners.get(listener_arn);
                Some((
                    destination,
                    super::access_logs::load_balancer_id(&record.arn),
                    host_only(&record.dns_name).to_string(),
                    listener.and_then(|listener| listener.protocol.clone()),
                    listener.and_then(|listener| listener.port),
                ))
            })
    }) else {
        return;
    };
    let answered = super::access_logs::AnsweredRequest {
        load_balancer_id,
        node_address,
        listener_protocol: protocol,
        listener_port: port,
        received_at,
        answered_at: SystemTime::now(),
        target_elapsed: served.observed.target_elapsed,
        method: request.method.clone(),
        host: request.host.clone(),
        path: request.path.clone(),
        query: request.query.clone(),
        user_agent: request.header(USER_AGENT_HEADER).map(str::to_string),
        trace_id: request.header(TRACE_ID_HEADER).map(str::to_string),
        received_bytes,
        sent_bytes: served.body.len(),
        elb_status: served.status,
        target_status: served.observed.target_status,
        target_group_arn: served.observed.target_group_arn.clone(),
        actions: served.observed.actions.clone(),
        redirect_url: served.location.clone(),
        error_reason: served.observed.error_reason.clone(),
        matched_rule_priority,
    };
    super::access_logs::deliver(ctx, &destination, &answered).await;
}

/// The data plane as the ROUTER serves it: the operation `Service::override_operation` hands
/// back for a request addressed to a load balancer this service holds.
///
/// A load balancer's own traffic is no modelled operation — no routing key names it — so it
/// is mounted the way an emulated data plane is: claimed by `Service::can_handle`
/// ([`claims_parts`]) and served here. It declares NO auth scheme, because a client of the
/// name a load balancer publishes carries no AWS credential and has none to sign with; that
/// is the same reach the plane had when it was answered from
/// `Service::override_routing_error`, which the router reaches before authentication, and it
/// still claims only a request addressed to a load balancer this store holds. Being an
/// operation is what hands the plane the in-process transport a `forward` to a `lambda`
/// target group invokes the function over ([`super::lambda_target`]).
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
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move { serve(&*self.engine, req, ctx).await })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // A client that sends traffic to the name a load balancer published presents no AWS
        // credential: this plane serves the request however it arrived.
        Some(&[])
    }
}

/// The listener the request reached: the one listening on the port the client addressed
/// and, where that port names none of them — a client reaching the emulator's own endpoint
/// addresses the EMULATOR's port, not the load balancer's — the lowest-numbered one, which
/// is the port a load balancer's first listener is given.
///
/// Only a listener on a protocol that carries the request is considered
/// ([`served_protocols`]), so a load balancer whose listeners carry none of it has none
/// here — which is what makes the claim and the answer agree.
fn listener_addressed<'s>(
    store: &'s ElbState,
    record: &LoadBalancerRecord,
    port: Option<i32>,
) -> Option<&'s ListenerRecord> {
    let protocols = served_protocols(record.family());
    let mut listeners: Vec<&ListenerRecord> = record
        .listener_arns
        .iter()
        .filter_map(|arn| store.listeners.get(arn))
        .filter(|listener| {
            listener
                .protocol
                .as_deref()
                .is_some_and(|protocol| protocols.contains(&protocol))
        })
        .collect();
    listeners.sort_by(|left, right| left.port.cmp(&right.port).then_with(|| left.arn.cmp(&right.arn)));
    port.and_then(|port| {
        listeners
            .iter()
            .copied()
            .find(|listener| listener.port == Some(port))
    })
    .or_else(|| listeners.first().copied())
}

/// What `listener` answers `request` with: the actions of the first rule whose conditions
/// are all met, in ascending priority order, and otherwise the listener's own default
/// actions — which are its default rule's.
///
/// The PRIORITY of the rule that matched travels beside the answer, because the load
/// balancer's own access log records it ([`super::access_logs`]) and this is where it is
/// known: 0 for a request no rule matched, which its listener's default actions answered.
fn answered(
    store: &ElbState,
    listener: &ListenerRecord,
    request: &DataPlaneRequest,
    now: SystemTime,
) -> (Performed, i32) {
    let matched = matched_actions(listener, request);
    (
        performed(store, listener, matched.actions, request, now),
        matched.priority,
    )
}

/// The actions one request matched, and the priority of the rule they belong to.
struct Matched<'l> {
    actions: &'l [Action],
    /// The matched rule's own priority, and 0 for the listener's default actions — the value
    /// Elastic Load Balancing records for a request no rule of the listener matched.
    priority: i32,
}

/// The actions the request matched: "rules are evaluated in priority order, from the
/// lowest value to the highest value", and the default actions when no rule's conditions
/// are met. The ARN breaks a tie only so the walk is stable whatever the store's
/// iteration order; two rules of one listener never share a priority.
///
/// The whole walk shares ONE matching budget ([`MAX_MATCH_STEPS`]), so what a request
/// costs is bounded however many rules the listener carries and however long their
/// condition values are. A walk that spends it stops where it is, and the default actions
/// answer.
fn matched_actions<'l>(listener: &'l ListenerRecord, request: &DataPlaneRequest) -> Matched<'l> {
    let mut budget = MAX_MATCH_STEPS;
    for rule in crate::state::rules_in_evaluation_order(listener) {
        if budget == 0 {
            tracing::warn!(
                rule = %rule.arn,
                "the request spent its whole matching budget before this rule, so the \
                 listener's default actions answer it"
            );
            break;
        }
        if !rule.conditions.is_empty() && conditions_met(&rule.conditions, request, &mut budget) {
            return Matched {
                actions: &rule.actions,
                priority: rule.priority,
            };
        }
    }
    Matched {
        actions: &listener.default_actions,
        priority: 0,
    }
}

/// Whether EVERY condition of a rule is met — "each rule ... include[s] up to one of each
/// of the following conditions", and a rule matches when all of them do.
fn conditions_met(
    conditions: &[RuleCondition],
    request: &DataPlaneRequest,
    budget: &mut usize,
) -> bool {
    conditions
        .iter()
        .all(|condition| condition_met(condition, request, budget))
}

/// Whether one condition is met. A condition's values are alternatives — any one of them
/// matching is a match — and each is written the wildcard language
/// [`matches_within`] reads.
///
/// Two condition forms are NOT evaluated here, and each is left UNMET with a warning
/// naming it rather than matched on a guess — so a rule passed over for one of them is
/// traceable to the form, instead of looking like a rule the request simply did not match:
///
/// * `RegexValues`: this emulator has no regular-expression engine for that dialect, and a
///   hand-written approximation of one would match the wrong requests silently;
/// * `source-ip`: AWS evaluates it against the address of the CONNECTION, and documents
///   that the addresses in `X-Forwarded-For` do not satisfy it (an `http-header` condition
///   is how that header is matched). A request served in-process carries no connection
///   address for this plane to read, and matching a request-supplied header instead would
///   let any client satisfy a rule written to gate traffic by CIDR.
fn condition_met(
    condition: &RuleCondition,
    request: &DataPlaneRequest,
    budget: &mut usize,
) -> bool {
    let direct = condition.values.as_deref().unwrap_or_default();
    let field = condition.field.as_deref().unwrap_or_default();
    if carries_regex_values(condition) {
        tracing::warn!(
            field,
            "the condition matches on regular expressions, which this plane does not \
             evaluate, so it is left unmet and its rule is passed over"
        );
        return false;
    }
    match field {
        // "path-pattern ... the comparison is case sensitive", and the pattern is matched
        // against the path alone, not the query string.
        PATH_PATTERN_FIELD => {
            let configured = condition
                .path_pattern_config
                .as_ref()
                .and_then(|config| config.values.as_deref())
                .unwrap_or_default();
            direct
                .iter()
                .chain(configured)
                .any(|pattern| matches_within(pattern, &request.path, CASE_SENSITIVE, budget))
        }
        // A host name is case-insensitive, so the comparison is made without regard to
        // case rather than by folding either side here.
        HOST_HEADER_FIELD => {
            let configured = condition
                .host_header_config
                .as_ref()
                .and_then(|config| config.values.as_deref())
                .unwrap_or_default();
            direct
                .iter()
                .chain(configured)
                .any(|pattern| matches_within(pattern, &request.host, CASE_INSENSITIVE, budget))
        }
        // "http-request-method ... the comparison is case sensitive", and a method is
        // upper case on the wire. No wildcard is documented for it, so the value is the
        // method itself — and the values are read from its own configuration shape alone:
        // `RuleCondition$Values` is documented "specify only when Field is host-header or
        // path-pattern".
        HTTP_REQUEST_METHOD_FIELD => condition
            .http_request_method_config
            .as_ref()
            .and_then(|config| config.values.as_deref())
            .unwrap_or_default()
            .iter()
            .any(|value| value == &request.method),
        // The header NAME is case-insensitive; its values are patterns.
        HTTP_HEADER_FIELD => {
            let Some(config) = condition.http_header_config.as_ref() else {
                return false;
            };
            let Some(name) = config.http_header_name.as_deref() else {
                return false;
            };
            let Some(value) = request.header(&name.to_ascii_lowercase()) else {
                return false;
            };
            config
                .values
                .as_deref()
                .unwrap_or_default()
                .iter()
                .any(|pattern| matches_within(pattern, value, CASE_SENSITIVE, budget))
        }
        // A query-string condition's pair matches when the request carries a parameter
        // whose key and value both match; an entry with no key matches on the value alone.
        QUERY_STRING_FIELD => condition
            .query_string_config
            .as_ref()
            .and_then(|config| config.values.as_deref())
            .unwrap_or_default()
            .iter()
            .any(|pair| {
                request.query.split('&').filter(|raw| !raw.is_empty()).any(|raw| {
                    let (key, value) = raw.split_once('=').unwrap_or((raw, ""));
                    pair.key
                        .as_deref()
                        .is_none_or(|pattern| {
                            matches_within(pattern, key, CASE_SENSITIVE, budget)
                        })
                        && pair.value.as_deref().is_none_or(|pattern| {
                            matches_within(pattern, value, CASE_SENSITIVE, budget)
                        })
                })
            }),
        // "source-ip ... you must specify an IP address block in CIDR", evaluated against
        // the address of the CONNECTION — which a request served in-process does not
        // carry, so the condition is left unmet (see this function's own documentation).
        SOURCE_IP_FIELD => {
            tracing::warn!(
                "the condition matches on the client's connection address, which a request \
                 served in-process does not carry, so it is left unmet and its rule is \
                 passed over"
            );
            false
        }
        // A field this service does not match on names no condition a rule could have
        // been created with.
        _ => false,
    }
}

/// Whether this condition matches on REGULAR EXPRESSIONS — in `RegexValues`, or in the
/// `RegexValues` of the field's own configuration shape — which is what
/// [`condition_met`] leaves unmet rather than approximating.
fn carries_regex_values(condition: &RuleCondition) -> bool {
    let listed = |values: Option<&Vec<String>>| values.is_some_and(|values| !values.is_empty());
    listed(condition.regex_values.as_ref())
        || condition
            .path_pattern_config
            .as_ref()
            .is_some_and(|config| listed(config.regex_values.as_ref()))
        || condition
            .host_header_config
            .as_ref()
            .is_some_and(|config| listed(config.regex_values.as_ref()))
        || condition
            .http_header_config
            .as_ref()
            .is_some_and(|config| listed(config.regex_values.as_ref()))
}

/// Whether `value` matches `pattern` in the wildcard language every condition's values
/// are written in — "? (matches exactly 1 character)" and "* (matches 0 or more
/// characters)", with the pattern matching the WHOLE value — charged to this request's
/// matching `budget`.
///
/// The walk itself is `service_common::iam_authorization::wildcard_match`, which is the one
/// implementation of that language in the tree (the same two metacharacters, the same
/// whole-value match, and the case flag that serves both the case-sensitive comparisons and
/// the case-insensitive host one). What is added here is the BOUND: a value longer than
/// AWS publishes for a condition, or request text longer than this emulator matches
/// against, is matched against nothing, and the product of the two lengths — the bound on
/// a walk with one backtrack point per `*` — is charged to the budget before the walk runs,
/// so the whole request's matching cost is capped rather than growing with what the caller
/// sent and stored.
fn matches_within(pattern: &str, value: &str, case_sensitive: bool, budget: &mut usize) -> bool {
    // Measured through a bounded probe, so the measuring itself cannot be made to walk
    // text of the caller's choosing, and CHARGED before the lengths are judged, so the
    // rejected case cannot be repeated for free either.
    let pattern_chars = pattern.chars().take(MAX_CONDITION_VALUE_CHARS + 1).count();
    let value_chars = value.chars().take(MAX_MATCHED_CHARS + 1).count();
    // At least one step per match, so a walk cannot repeat for free on empty text.
    let cost = pattern_chars.saturating_mul(value_chars).max(1);
    if cost > *budget {
        *budget = 0;
        return false;
    }
    *budget -= cost;
    if pattern_chars > MAX_CONDITION_VALUE_CHARS || value_chars > MAX_MATCHED_CHARS {
        tracing::warn!(
            pattern_chars,
            value_chars,
            "the condition value or the request text is longer than this plane matches, so \
             the condition is left unmet"
        );
        return false;
    }
    ::service_common::iam_authorization::wildcard_match(pattern, value, case_sensitive)
}

/// What [`matches_within`]'s case flag means at each call site: a host name compares
/// without regard to case, and every other condition value compares exactly.
const CASE_SENSITIVE: bool = true;
const CASE_INSENSITIVE: bool = false;

/// What performing the matched action list resolved to.
#[derive(Debug)]
enum Performed {
    /// The answer, computed from the records this service holds and nothing else.
    Answered(Served),
    /// The action forwards to a `lambda` target group, so the client is answered from an
    /// INVOCATION of the function registered with it ([`super::lambda_target`]).
    Invoke(Box<super::lambda_target::Forwarded>),
    /// The action forwards to an `alb` target group, so the request is PASSED ON to the
    /// Application Load Balancer registered with it and answered by that load balancer's own
    /// listener ([`PassedOn`]).
    PassedOn(Box<PassedOn>),
}

/// The load balancer a request was passed on to, and the port the registration says it
/// receives traffic on — which is what selects the listener of ITS that answers.
#[derive(Debug)]
struct PassedOn {
    load_balancer_arn: String,
    port: Option<i32>,
}

/// Perform the matched action list: the terminating action answers the client, and an
/// action this emulator cannot perform answers [`ACTION_NOT_PERFORMED_STATUS`] rather than
/// serving what is behind it.
///
/// "The last action to be performed must be one of the following types of actions: a
/// forward, fixed-response, or redirect" — so the answer is the terminating action with
/// the highest `Order`, which is the last one performed.
fn performed(
    store: &ElbState,
    listener: &ListenerRecord,
    actions: &[Action],
    request: &DataPlaneRequest,
    now: SystemTime,
) -> Performed {
    if let Some(action) = actions
        .iter()
        .find(|action| !is_terminating(&action.r#type))
    {
        tracing::warn!(
            action = %action.r#type,
            "the action needs an identity provider this emulator makes no connection to, \
             so the request it guards is not served"
        );
        return Performed::Answered(Served::status(ACTION_NOT_PERFORMED_STATUS));
    }
    let Some(action) = actions
        .iter()
        .filter(|action| is_terminating(&action.r#type))
        .max_by_key(|action| action.order.unwrap_or_default())
    else {
        // A listener with no default action and a matched rule with no action have nothing
        // to answer with, which no create admits.
        return Performed::Answered(Served::status(NO_HEALTHY_TARGET_STATUS));
    };
    match action.r#type.as_str() {
        FIXED_RESPONSE_ACTION => Performed::Answered(acted(
            fixed_response(action),
            FIXED_RESPONSE_ACTION,
        )),
        REDIRECT_ACTION => Performed::Answered(acted(
            redirect(action, listener, request),
            REDIRECT_ACTION,
        )),
        FORWARD_ACTION => forward(store, listener, action, request, now),
        // Unreachable: every type that is not terminating was answered above, and
        // `Action$Type` carries no other terminating one.
        _ => Performed::Answered(Served::status(ACTION_NOT_PERFORMED_STATUS)),
    }
}

/// The same answer, recording the action that produced it — what the load balancer's own
/// access log carries as `actions_executed` ([`super::access_logs`]).
fn acted(mut served: Served, action_type: &str) -> Served {
    served.observed.actions.push(action_type.to_string());
    served
}

/// Whether this action type ANSWERS the client, as opposed to guarding what follows it.
fn is_terminating(action_type: &str) -> bool {
    matches!(
        action_type,
        FIXED_RESPONSE_ACTION | REDIRECT_ACTION | FORWARD_ACTION
    )
}

/// The response a `fixed-response` action renders: "the HTTP response code (2XX, 4XX, or
/// 5XX)", the content type, and the message body — all three as they were persisted.
fn fixed_response(action: &Action) -> Served {
    let Some(config) = action.fixed_response_config.as_ref() else {
        return Served::status(NO_HEALTHY_TARGET_STATUS);
    };
    let Ok(status) = config.status_code.parse::<u16>() else {
        // `FixedResponseActionConfig$StatusCode` is a three-digit code; a record carrying
        // anything else is one no create admits.
        return Served::status(NO_HEALTHY_TARGET_STATUS);
    };
    let body = config.message_body.clone().unwrap_or_default().into_bytes();
    Served {
        status,
        content_type: Some(
            config
                .content_type
                .clone()
                .unwrap_or_else(|| DEFAULT_FIXED_RESPONSE_CONTENT_TYPE.to_string()),
        ),
        location: None,
        headers: Vec::new(),
        body,
        observed: Observed::default(),
    }
}

/// The response a `redirect` action renders: its status code, and a `Location` built by
/// substituting the request's own components into the placeholders each component carries.
///
/// The defaults are the ones the action's own documentation gives for a component left
/// out — the request's own protocol, host, port, path and query — so a redirect that
/// changes one component keeps the rest.
fn redirect(action: &Action, listener: &ListenerRecord, request: &DataPlaneRequest) -> Served {
    let Some(config) = action.redirect_config.as_ref() else {
        return Served::status(NO_HEALTHY_TARGET_STATUS);
    };
    let Some(status) = config
        .status_code
        .strip_prefix("HTTP_")
        .and_then(|code| code.parse::<u16>().ok())
    else {
        // `RedirectActionConfig$StatusCode` is `HTTP_301` or `HTTP_302`; a record carrying
        // anything else is one no create admits.
        return Served::status(NO_HEALTHY_TARGET_STATUS);
    };
    // ONE budget for the whole redirect, charged every character the rendering produces,
    // so the URL this plane builds cannot grow with the stored template's placeholders
    // times the request's own path or query.
    let mut budget = MAX_REDIRECT_RENDER_CHARS;
    let mut component = |held: Option<&String>, default: &str| {
        substituted(
            held.map(String::as_str).unwrap_or(default),
            listener,
            request,
            &mut budget,
        )
    };
    let rendered = (
        component(config.protocol.as_ref(), PROTOCOL_PLACEHOLDER),
        component(config.host.as_ref(), HOST_PLACEHOLDER),
        component(config.port.as_ref(), PORT_PLACEHOLDER),
        component(config.path.as_ref(), &format!("/{PATH_PLACEHOLDER}")),
        component(config.query.as_ref(), QUERY_PLACEHOLDER),
    );
    let (Some(protocol), Some(host), Some(port), Some(path), Some(query)) = rendered else {
        tracing::warn!(
            "the redirect's URL does not fit the characters one request's rendering may \
             produce, so the action is not performed"
        );
        return Served::status(ACTION_NOT_PERFORMED_STATUS);
    };

    // A URI scheme is case-insensitive and is written in lower case, while the protocol
    // the action and the listener carry is the model's own upper-case `ProtocolEnum`.
    let scheme = protocol.to_ascii_lowercase();
    let mut location = format!("{scheme}://{host}");
    // The port is left off when it is the one the scheme implies, which is the URL a
    // client is sent to for a redirect that keeps the default port.
    if !port.is_empty() && Some(port.as_str()) != default_port(&scheme) {
        location.push(':');
        location.push_str(&port);
    }
    if !path.starts_with('/') {
        location.push('/');
    }
    location.push_str(&path);
    if !query.is_empty() {
        location.push('?');
        location.push_str(&query);
    }
    Served {
        status,
        content_type: None,
        location: Some(location),
        headers: Vec::new(),
        body: Vec::new(),
        observed: Observed::default(),
    }
}

/// One redirect component with every placeholder replaced by what the request carried:
/// the listener's protocol and port — the protocol the request arrived on and the port its
/// listener serves — and the request's own host, path (without its leading `/`) and query
/// (without its leading `?`).
///
/// Rendered under `budget`, which every character the output carries is charged to —
/// whether it came from the stored template or from the request — so a template holding
/// many placeholders cannot multiply the request's own text into a header of the caller's
/// choosing, and nothing is allocated past the bound rather than after it. `None` where
/// the budget runs out: a redirect this plane could not build, which [`redirect`] answers
/// as an action it could not perform.
fn substituted(
    template: &str,
    listener: &ListenerRecord,
    request: &DataPlaneRequest,
    budget: &mut usize,
) -> Option<String> {
    let protocol = listener.protocol.clone().unwrap_or_default();
    let port = listener
        .port
        .map(|port| port.to_string())
        .unwrap_or_default();
    let replacements: [(&str, &str); 5] = [
        (PROTOCOL_PLACEHOLDER, protocol.as_str()),
        (HOST_PLACEHOLDER, request.host.as_str()),
        (PORT_PLACEHOLDER, port.as_str()),
        (
            PATH_PLACEHOLDER,
            request.path.strip_prefix('/').unwrap_or(&request.path),
        ),
        (QUERY_PLACEHOLDER, request.query.as_str()),
    ];
    let mut rendered = String::new();
    let mut rest = template;
    'template: while !rest.is_empty() {
        for (placeholder, value) in &replacements {
            if let Some(tail) = rest.strip_prefix(*placeholder) {
                if value.len() > *budget {
                    *budget = 0;
                    return None;
                }
                *budget -= value.len();
                rendered.push_str(value);
                rest = tail;
                continue 'template;
            }
        }
        let mut characters = rest.chars();
        let Some(next) = characters.next() else {
            break;
        };
        if next.len_utf8() > *budget {
            *budget = 0;
            return None;
        }
        *budget -= next.len_utf8();
        rendered.push(next);
        rest = characters.as_str();
    }
    Some(rendered)
}

/// The port a URL of `protocol` implies, and so leaves out.
fn default_port(protocol: &str) -> Option<&'static str> {
    match protocol {
        "HTTP" | "http" => Some("80"),
        "HTTPS" | "https" => Some("443"),
        _ => None,
    }
}

/// What a `forward` answers. The target groups it names are this store's own, the DRAW over
/// the weights it distributes them at decides which of them this request reaches
/// ([`super::target_group_selection`]), and the registration that group holds decides what
/// happens next: a `lambda` target group's function is INVOKED ([`super::lambda_target`]), an
/// `alb` target group's load balancer is the one the request is PASSED ON to, and a target of
/// any other group is a registration this emulator has no server behind, so the answer is the
/// status a load balancer gives for the condition that holds — no target to choose, or a
/// target chosen whose connection cannot be made.
fn forward(
    store: &ElbState,
    listener: &ListenerRecord,
    action: &Action,
    request: &DataPlaneRequest,
    now: SystemTime,
) -> Performed {
    let distribution = super::target_group_selection::distribution(action);
    // Every group of the distribution this store holds that has a target to send the request
    // to, with the weight the action gives it. A group whose targets are none the load
    // balancer may send a request to is not one this request can be distributed to, so it
    // takes no share of the action's requests — and a request the action's other groups can
    // answer is answered rather than lost to the draw.
    let candidates: Vec<(&TargetGroupRecord, &RegisteredTarget, i32)> = distribution
        .groups
        .iter()
        .filter_map(|(arn, weight)| store.target_groups.get(arn).map(|group| (group, *weight)))
        .filter_map(|(group, weight)| {
            group
                .targets
                .values()
                .find(|target| receives_requests(group, target, now))
                .map(|target| (group, target, weight))
        })
        .collect();
    let weights: Vec<i32> = candidates
        .iter()
        .map(|(_, _, weight)| *weight)
        .collect::<Vec<i32>>();
    // The pin the client carried back, where this action's stickiness minted one and the
    // group it names still takes a share of the action's requests.
    let pinned = distribution
        .sticky
        .then(|| {
            let token = super::target_group_selection::pinned_token(
                request.header(super::target_group_selection::COOKIE_HEADER),
            )?;
            candidates.iter().position(|(group, _, weight)| {
                *weight > 0 && super::target_group_selection::group_token(&group.arn) == token
            })
        })
        .flatten();
    // One draw per request, which is what makes the share each group receives converge on
    // the weight the action gives it. Nothing of the draw is written: it is a choice made
    // for this request alone.
    let chosen = pinned.or_else(|| {
        super::target_group_selection::drawn_entry(&weights, ::rand::random::<f64>())
    });
    let Some((group, target, _)) = chosen.and_then(|index| candidates.get(index)) else {
        return Performed::Answered(Served::status(NO_HEALTHY_TARGET_STATUS));
    };
    if group.is_lambda() {
        // "lambda - Register a single Lambda function as a target": the registration holds
        // the function's ARN, and the request is served by invoking it.
        return Performed::Invoke(Box::new(super::lambda_target::Forwarded {
            function_arn: target.id.clone(),
            target_group_arn: group.arn.clone(),
            multi_value_headers: super::lambda_target::multi_value_headers(group),
            protocol: listener.protocol.clone(),
            port: listener.port,
            // The pin that answer hands back, for a client that does not hold it already:
            // "requests from a client should be routed to the same target group" is what the
            // cookie makes true of every later request carrying it.
            set_cookie: (distribution.sticky && pinned.is_none()).then(|| {
                super::target_group_selection::set_cookie(
                    &group.arn,
                    distribution.sticky_duration_seconds,
                )
            }),
        }));
    }
    if group.is_load_balancer_target() {
        // "alb - Register a single Application Load Balancer as a target", whose
        // registration holds that load balancer's ARN (`TargetDescription$Id`: "if the target
        // type is alb, specify the ARN of the Application Load Balancer target") — a load
        // balancer of this very store, which the request is handed to so that what answers is
        // ITS own listener rather than anything invented here. The port is the one the
        // registration says the target receives traffic on, which is the listener of its that
        // takes the connection.
        if store.load_balancers.contains_key(&target.id) {
            return Performed::PassedOn(Box::new(PassedOn {
                load_balancer_arn: target.id.clone(),
                port: target.port.or(group.port),
            }));
        }
        tracing::warn!(
            "the target group's healthy target names a load balancer this service no longer \
             holds, so the connection to it cannot be made"
        );
        return Performed::Answered(Served::status(TARGET_UNREACHABLE_STATUS));
    }
    tracing::warn!(
        "the target group's healthy target is a registration with no server of this \
         emulator's behind it, so the connection to it cannot be made"
    );
    Performed::Answered(Served::status(TARGET_UNREACHABLE_STATUS))
}

/// Whether the load balancer may send this registration a request: its reported health says
/// so.
///
/// A target the health checks found [`HEALTHY_TARGET_STATE`] receives requests, and so does
/// one of a target group whose health checks are OFF ([`HEALTH_CHECKS_DISABLED_STATE`]):
/// nothing is checking it, so nothing marks it unhealthy either. Every other reported state
/// — a registration still coming up, one draining, one of a group no load balancer forwards
/// to — is one the load balancer does not send requests to.
fn receives_requests(
    group: &TargetGroupRecord,
    target: &RegisteredTarget,
    now: SystemTime,
) -> bool {
    matches!(
        group.health_of(target, now).state.as_deref(),
        Some(HEALTHY_TARGET_STATE) | Some(HEALTH_CHECKS_DISABLED_STATE)
    )
}

/// One rendered answer: the status, its headers and its body, as the wire carries them.
fn rendered(served: &Served) -> RtResponse {
    let mut response = RtResponse::new(
        StatusCode::try_from(served.status)
            .unwrap_or_else(|_| StatusCode::try_from(PROVISIONING_STATUS).expect("503 is a status")),
        SdkBody::from(served.body.clone()),
    );
    if let Some(content_type) = &served.content_type {
        if let Err(refused) = response
            .headers_mut()
            .try_insert("content-type", content_type.clone())
        {
            // A content type is a stored value no operation holds to a media type, so one
            // the wire cannot carry is left off the response rather than failing it.
            tracing::warn!(
                error = %refused,
                "the action's content type cannot be sent on the wire and is left off the \
                 response"
            );
        }
    }
    if let Some(location) = &served.location {
        if let Err(refused) = response.headers_mut().try_insert("location", location.clone()) {
            tracing::warn!(
                error = %refused,
                "the redirect's location cannot be sent on the wire and is left off the \
                 response"
            );
        }
    }
    for (name, value) in &served.headers {
        if let Err(refused) = response.headers_mut().try_append(name.clone(), value.clone()) {
            // A header a web ACL's custom response configures, or a `lambda` target's
            // response named, is a stored or returned value, so one the wire cannot carry is
            // left off the response rather than failing it. APPENDED, not inserted: a name
            // the answer carries more than once is sent more than once, which is how several
            // cookies travel.
            tracing::warn!(
                header = %name,
                error = %refused,
                "the header the answer carries cannot be sent on the wire and is left off \
                 the response"
            );
        }
    }
    let _ = response
        .headers_mut()
        .try_insert("content-length", served.body.len().to_string());
    response
}

#[cfg(test)]
mod tests {
    //! The pure rules of this module that a wire test cannot vary cheaply: the wildcard
    //! language a condition's values are written in, and how an addressing is split. What
    //! this plane SERVES is pinned end to end in `tests/all/alb_data_plane.rs`.

    use super::*;

    #[test]
    fn a_condition_value_matches_in_the_documented_wildcard_language() {
        // The rows span what the language generalises over: a literal, each
        // metacharacter alone, both together, a pattern longer than the value, the
        // backtracking case a single left-to-right scan gets wrong, the case flag each
        // field compares under, and the two lengths this plane refuses to walk at all.
        let cases: &[(&str, &str, bool, bool)] = &[
            ("/a", "/a", CASE_SENSITIVE, true),
            ("/a", "/b", CASE_SENSITIVE, false),
            ("/a", "/a/b", CASE_SENSITIVE, false),
            ("/img/*", "/img/cat.png", CASE_SENSITIVE, true),
            ("/img/*", "/img/", CASE_SENSITIVE, true),
            ("/img/*", "/image", CASE_SENSITIVE, false),
            ("/?", "/a", CASE_SENSITIVE, true),
            ("/?", "/ab", CASE_SENSITIVE, false),
            ("/?/b", "/a/b", CASE_SENSITIVE, true),
            ("*", "", CASE_SENSITIVE, true),
            ("*", "/anything/at/all", CASE_SENSITIVE, true),
            ("", "", CASE_SENSITIVE, true),
            ("", "/a", CASE_SENSITIVE, false),
            ("/a*b", "/axxxb", CASE_SENSITIVE, true),
            ("/a*b", "/axxxbc", CASE_SENSITIVE, false),
            ("/a*b*c", "/abbbc", CASE_SENSITIVE, true),
            ("*.example", "foo.example", CASE_SENSITIVE, true),
            ("*.example", "foo.example.org", CASE_SENSITIVE, false),
            // The case flag: a path compares exactly, a host without regard to case.
            ("/A", "/a", CASE_SENSITIVE, false),
            ("SHOP.example.com", "shop.EXAMPLE.com", CASE_INSENSITIVE, true),
            ("*.example.com", "SHOP.EXAMPLE.COM", CASE_INSENSITIVE, true),
        ];
        for (pattern, value, case_sensitive, expected) in cases {
            let mut budget = MAX_MATCH_STEPS;
            let matched = matches_within(pattern, value, *case_sensitive, &mut budget);
            assert_eq!(
                matched, *expected,
                "case ({pattern:?}, {value:?}, case_sensitive {case_sensitive}): expected \
                 {expected}, got {matched}"
            );
            assert!(
                budget < MAX_MATCH_STEPS,
                "case ({pattern:?}, {value:?}): every match is charged to the budget, which \
                 is still {budget}"
            );
        }
    }

    #[test]
    fn one_requests_matching_is_bounded_by_the_budget_and_the_two_lengths() {
        // Text longer than this plane matches, and a condition value longer than AWS
        // publishes, are matched against nothing — whatever wildcard they carry.
        let long_value = "a".repeat(MAX_MATCHED_CHARS + 1);
        let long_pattern = "a".repeat(MAX_CONDITION_VALUE_CHARS + 1);
        let rows: &[(&str, &str, &str)] = &[
            ("request text past the bound", "*", long_value.as_str()),
            ("a condition value past the bound", long_pattern.as_str(), "a"),
        ];
        for (case, pattern, value) in rows {
            let mut budget = MAX_MATCH_STEPS;
            assert!(
                !matches_within(pattern, value, CASE_SENSITIVE, &mut budget),
                "case {case:?}: nothing past the bound is walked"
            );
            assert!(
                budget < MAX_MATCH_STEPS,
                "case {case:?}: a match the bound rejects is charged too, so it cannot be \
                 repeated for free; the budget is still {budget}"
            );
        }

        // The budget is spent, not exceeded: a match whose cost it cannot cover is not
        // walked, and nothing after it is either.
        let mut budget = 4_usize;
        assert!(
            !matches_within("abcde", "abcde", CASE_SENSITIVE, &mut budget),
            "a match costing more than the remaining budget is not walked"
        );
        assert_eq!(budget, 0, "a spent budget is spent, got {budget}");
    }

    #[test]
    fn a_redirect_renders_under_one_budget_and_nothing_past_it() {
        let listener = ListenerRecord {
            protocol: Some("HTTP".to_string()),
            port: Some(80),
            ..ListenerRecord::default()
        };
        let request = DataPlaneRequest {
            method: "GET".to_string(),
            path: "/img/cat.png".to_string(),
            query: "size=large".to_string(),
            host: "shop.example.com".to_string(),
            ..DataPlaneRequest::default()
        };
        // Each placeholder is replaced by what the request carried, and the literal text
        // around it is kept.
        let rows: &[(&str, &str, &str)] = &[
            ("the protocol", PROTOCOL_PLACEHOLDER, "HTTP"),
            ("the host", HOST_PLACEHOLDER, "shop.example.com"),
            ("the port", PORT_PLACEHOLDER, "80"),
            ("the path without its leading slash", PATH_PLACEHOLDER, "img/cat.png"),
            ("the query without its question mark", QUERY_PLACEHOLDER, "size=large"),
            ("a literal", "/moved", "/moved"),
            (
                "a placeholder inside literal text",
                "/new/#{path}?v=2",
                "/new/img/cat.png?v=2",
            ),
            (
                "the same placeholder twice",
                "#{host}.#{host}",
                "shop.example.com.shop.example.com",
            ),
        ];
        for (case, template, expected) in rows {
            let mut budget = MAX_REDIRECT_RENDER_CHARS;
            let rendered = substituted(template, &listener, &request, &mut budget);
            assert_eq!(
                rendered.as_deref(),
                Some(*expected),
                "case {case:?}: expected {expected:?}, got {rendered:?}"
            );
            assert!(
                budget < MAX_REDIRECT_RENDER_CHARS,
                "case {case:?}: every character rendered is charged to the budget, which is \
                 still {budget}"
            );
        }

        // A template holding many placeholders cannot multiply the request's own text into
        // a URL of the caller's choosing: the rendering stops at the bound, and nothing
        // past it is built.
        let many = PATH_PLACEHOLDER.repeat(MAX_REDIRECT_RENDER_CHARS);
        let long_path = format!("/{}", "a".repeat(MAX_MATCHED_CHARS));
        let request = DataPlaneRequest {
            path: long_path,
            ..request
        };
        let mut budget = MAX_REDIRECT_RENDER_CHARS;
        assert!(
            substituted(&many, &listener, &request, &mut budget).is_none(),
            "a rendering past the bound produces nothing"
        );
        assert_eq!(budget, 0, "the bound is spent, got {budget}");
    }

    #[test]
    fn a_prefixed_addressing_names_the_load_balancer_and_the_request_path() {
        let cases: &[(&str, Option<(&str, &str)>)] = &[
            (
                "/_aws/elasticloadbalancing/my-alb/a",
                Some(("my-alb", "/a")),
            ),
            (
                "/_aws/elasticloadbalancing/my-alb-123.us-west-2.elb.amazonaws.com/a/b",
                Some(("my-alb-123.us-west-2.elb.amazonaws.com", "/a/b")),
            ),
            ("/_aws/elasticloadbalancing/my-alb", Some(("my-alb", "/"))),
            ("/_aws/elasticloadbalancing/", None),
            ("/_aws/execute-api/my-alb/a", None),
            ("/_aws/elasticloadbalancingv2/my-alb/a", None),
            ("/a", None),
            ("/", None),
        ];
        for (path, expected) in cases {
            let resolved = prefixed_addressing(path);
            assert_eq!(
                resolved, *expected,
                "case {path:?}: expected {expected:?}, got {resolved:?}"
            );
        }
    }

    #[test]
    fn a_host_is_read_the_way_the_front_door_reads_it() {
        // A host that names no service is the emulator's own endpoint, which is the only
        // address a client on this machine can reach.
        for host in ["127.0.0.1:9297", "localhost:9297", "[::1]:9297", "[::1]"] {
            assert!(
                !names_a_service(host),
                "case {host:?}: a loopback host names no service"
            );
        }
        for host in [
            "elasticloadbalancing.us-west-2.amazonaws.com",
            "elasticloadbalancingv2.us-west-2.amazonaws.com:443",
        ] {
            assert!(
                names_a_service(host),
                "case {host:?}: a service endpoint names its service"
            );
        }
        assert_eq!(
            addressed_port(Some("my-alb.us-west-2.elb.amazonaws.com:8080")),
            Some(8080),
            "the port a host names is the one the client addressed"
        );
        assert_eq!(
            addressed_port(Some("my-alb.us-west-2.elb.amazonaws.com")),
            None,
            "a host naming no port leaves the listener to be chosen by the load balancer"
        );
    }
}
