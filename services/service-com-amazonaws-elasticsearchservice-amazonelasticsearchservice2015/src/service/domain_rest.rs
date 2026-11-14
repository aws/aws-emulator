// SPDX-License-Identifier: CC0-1.0

//! The DOMAIN's own REST data plane: how a request a client addresses to the
//! endpoint an Elasticsearch domain of this service published is recognised, and
//! how it is answered out of the indexes and documents that domain holds.
//!
//! A domain this service creates publishes the host a client sends index and
//! search requests to — `ElasticsearchDomainStatus$Endpoint`, "the Elasticsearch
//! domain endpoint that you use to submit index and search requests" — and the
//! protocol spoken there is the Elasticsearch REST API, which this service's
//! model does not describe: it models the control plane alone. That endpoint's
//! first label is the domain's own and the path it carries names no modelled
//! operation, so both halves of the front door's resolution missed it and every
//! signed request to the one address this service told the caller to use was
//! answered `404 UnknownService`. Nothing served a domain's own traffic at all,
//! so a client that creates a domain and then indexes or searches against the
//! endpoint it was handed could not reach the cluster the control plane said it
//! had provisioned.
//!
//! So this module is the second half of [`crate::service::EmulatedService`]: the
//! claim ([`claims_parts`], read by `Service::can_handle`) and the serving
//! ([`serve`], reached from `Service::override_operation` for a request carrying
//! a SigV4 credential — so the router verifies that credential before anything
//! is served — and from `Service::override_routing_error` for one carrying
//! none).
//!
//! # What a request addresses
//!
//! A client on the machine this emulator runs on resolves a domain's AWS
//! endpoint host nowhere at all, so it reaches the emulator and carries what it
//! was told; two addressings resolve:
//!
//! * PREFIXED PATH-STYLE, the addressing `Endpoint` PUBLISHES: the data plane is
//!   named by the reserved namespace segment
//!   (`service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE`) and
//!   `crate::state::DATA_PLANE_SEGMENT`, then the domain — its `DomainName` or a
//!   host it publishes — then the request path:
//!   `/_aws/es-2015-01-01/{domain}/{path}`. That segment begins with `_`, which
//!   no modelled path segment of any service does, so the prefix collides with
//!   no control-plane route, and the plane segment carries this model's own API
//!   version, so it is disjoint from the modern OpenSearch model's plane under
//!   the shared `es` signing name. This is what a client of a published endpoint
//!   uses, because the published value IS this addressing on the endpoint this
//!   emulator answers on (`crate::state::published_domain_address`);
//! * HOST-STYLE, the addressing a client that resolves AWS's own hosts uses: the
//!   request's host IS the endpoint host some domain of this store publishes
//!   (`crate::state::domain_endpoint`), and the whole path is the request path.
//!
//! Either way the claim is narrowed to a domain this service HOLDS, read from the
//! one shared store, so one request keeps one claimant. A domain of this store
//! is keyed by the Region it was created in as well as its name, so a named
//! domain resolves in the Region the request's own credential scope names first,
//! and in any Region holding that name otherwise.
//!
//! # What it answers, and what decides it
//!
//! Every status, header and byte is computed from what a create or a write
//! persisted: the cluster banner and the cluster health from the domain's own
//! configuration, an index's settings and mappings from the index record a
//! create wrote, a document from the write that stored it, and a count and a
//! search from the documents the index holds. A read writes NOTHING; the writes
//! are the requests that are writes.
//!
//! The REST surface served is the one this emulator can answer out of persisted
//! state: the cluster banner, the cluster health, an index's
//! create/read/head/delete, its settings and mappings, a document's
//! write/read/delete, a refresh, a count and a search over the bounded query
//! vocabulary [`search`] reads. A path outside it is answered the way the engine
//! answers a path no handler is registered for, rather than with an invented
//! body — including a RESERVED path ([`reserved`]), which names an API of the
//! engine and is never read as an index name. A SECTION of a search body or a
//! PARAMETER of a read that this cluster does not evaluate is refused too
//! ([`unevaluated_section`], [`unrecognized_parameter`]): a search that asked for
//! an order, an aggregation or a narrowed `_source` and was answered without them
//! would be answered a different question from the one it asked.
//!
//! A request reaching the serving seams has had its SigV4 signature verified by
//! the router, which is what authenticates a data-plane request here; the
//! domain's own `AccessPolicies` document is stored and reported by the control
//! plane and is not evaluated.

use std::time::SystemTime;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use serde_json::{json, Map, Value};
use smithy_server_runtime::http;

use ::service_common::data_plane_addressing::{first_segment, host_only, EMULATOR_PATH_NAMESPACE};

use crate::state::{
    self, service_state, ComAmazonawsElasticsearchserviceAmazonelasticsearchservice2015State as Store,
    Domain, SearchIndex,
};

/// The clause tree a search selects its documents with.
mod search;

/// The scheme an AWS API call's credential is presented under.
const SIGV4_SCHEME: &str = "AWS4-HMAC-SHA256";

/// How many hits one search may answer with: the engine's own
/// `index.max_result_window` default, which its refusal of a larger window
/// quotes.
const MAX_RESULT_WINDOW: usize = 10_000;

/// How many hits a search that asks for no particular size answers with — the
/// engine's own default `size`.
const DEFAULT_SEARCH_SIZE: usize = 10;

/// The section of a search or count body that states WHICH documents are asked
/// for.
const QUERY_SECTION: &str = "query";

/// The sections of a search body this cluster EVALUATES: which documents are
/// asked for, and the window of them to answer with.
///
/// A body naming any other section — an order, an aggregation, a narrowed
/// `_source`, a highlight — is refused ([`unevaluated_section`]) rather than
/// answered without it: the engine answers such a request by evaluating that
/// section, so answering it while ignoring the section would answer a different
/// question from the one the client asked.
const SEARCH_BODY_SECTIONS: &[&str] = &[QUERY_SECTION, "from", "size"];

/// The parameters a read of this cluster answers WITHOUT evaluating: the three the
/// engine takes on every request and that change no value it reports — how the
/// JSON is laid out, whether units are written for a human, and whether a stack
/// trace is attached.
const DIAGNOSTIC_PARAMETERS: [&str; 3] = ["pretty", "human", "error_trace"];

/// The parameters a `_search` recognises: the window it may state in the URI
/// instead of the body, and the diagnostic three.
const SEARCH_PARAMETERS: &[&str] = &[
    "from",
    "size",
    DIAGNOSTIC_PARAMETERS[0],
    DIAGNOSTIC_PARAMETERS[1],
    DIAGNOSTIC_PARAMETERS[2],
];

/// The parameters a `_count` recognises: a count answers no window, so only the
/// diagnostic three.
const COUNT_PARAMETERS: &[&str] = &DIAGNOSTIC_PARAMETERS;

/// The parameters a `_cluster/health` recognises: the status a request waits for,
/// which this cluster answers because it is already settled, and the diagnostic
/// three.
const HEALTH_PARAMETERS: &[&str] = &[
    WAIT_FOR_STATUS,
    DIAGNOSTIC_PARAMETERS[0],
    DIAGNOSTIC_PARAMETERS[1],
    DIAGNOSTIC_PARAMETERS[2],
];

/// The parameter a health request names the status it waits for in.
const WAIT_FOR_STATUS: &str = "wait_for_status";

/// The parameter a document write names the one op type it may ask for in.
const OP_TYPE: &str = "op_type";

/// The parameter a write names the visibility it waits for in.
///
/// Every write this cluster takes is visible to the next read the moment it lands,
/// which is what each of this parameter's values asks for, so a write naming it is
/// answered with that guarantee already met rather than refused.
const REFRESH: &str = "refresh";

/// The parameters a document write recognises: whether the write must CREATE, the
/// visibility it waits for, and the diagnostic three.
const WRITE_PARAMETERS: &[&str] = &[
    OP_TYPE,
    REFRESH,
    DIAGNOSTIC_PARAMETERS[0],
    DIAGNOSTIC_PARAMETERS[1],
    DIAGNOSTIC_PARAMETERS[2],
];

/// Which parameters the route `segments` names recognises.
///
/// Every route of this surface answers the diagnostic three, and a route that
/// EVALUATES a parameter of its own names it beside them — the window a search
/// answers, the status a health request waits for, what a write asks of an id it
/// already holds. Everything else a request may name is a parameter the engine
/// would evaluate and this cluster does not, which is refused rather than dropped
/// ([`unrecognized_parameter`]): the optimistic-concurrency preconditions
/// (`if_seq_no`, `if_primary_term`, `version`, `version_type`), the pipeline a
/// write names, a routing value, a narrowed `_source` or a report level.
fn recognized_parameters(segments: &[&str]) -> &'static [&'static str] {
    match segments {
        ["_cluster", "health", ..] => HEALTH_PARAMETERS,
        ["_search"] => SEARCH_PARAMETERS,
        [index, "_search"] if !reserved(index) => SEARCH_PARAMETERS,
        [index, "_doc"] if !reserved(index) => WRITE_PARAMETERS,
        [index, "_doc", _] if !reserved(index) => WRITE_PARAMETERS,
        [index, "_create", _] if !reserved(index) => WRITE_PARAMETERS,
        _ => COUNT_PARAMETERS,
    }
}

/// The engine's error type for an index name no index may carry.
const INVALID_INDEX_NAME_EXCEPTION: &str = "invalid_index_name_exception";

/// The engine's error type for an index the cluster does not hold.
const INDEX_NOT_FOUND_EXCEPTION: &str = "index_not_found_exception";

/// The engine's error type for a create of an index the cluster already holds.
const RESOURCE_ALREADY_EXISTS_EXCEPTION: &str = "resource_already_exists_exception";

/// The engine's error type for an argument it cannot act on.
const ILLEGAL_ARGUMENT_EXCEPTION: &str = "illegal_argument_exception";

/// The engine's error type for a body it cannot read as the JSON it expects.
const PARSE_EXCEPTION: &str = "parse_exception";

/// The engine's error type for a write to an id the index already holds, where
/// the write asked to create.
const VERSION_CONFLICT_ENGINE_EXCEPTION: &str = "version_conflict_engine_exception";

/// The engine's error type for a cluster that is not up.
const CLUSTER_BLOCK_EXCEPTION: &str = "cluster_block_exception";

/// The `index_uuid` the engine reports for an error about an index it holds no
/// cluster state for.
const NO_INDEX_UUID: &str = "_na_";

/// The `number_of_shards` setting an index carries unless its create named one.
const NUMBER_OF_SHARDS: &str = "number_of_shards";

/// The `number_of_replicas` setting an index carries unless its create named
/// one.
const NUMBER_OF_REPLICAS: &str = "number_of_replicas";

/// The engine's own default for both shard settings of a newly created index.
const DEFAULT_SHARD_COUNT: i64 = 1;

/// A ceiling on either shard setting, so an index's own settings — which a
/// caller wrote — cannot multiply out without bound when a shard summary is
/// computed from them.
const MAX_SHARDS_PER_INDEX: i64 = 1_024;

/// What a request ADDRESSES, as the wire carries it: the host it was sent to —
/// absent on a request naming neither a `Host` header nor an absolute URI — its
/// path, and the Region its credential scope names.
///
/// Built from BOTH shapes the router hands this module, the buffered parts the
/// claim reads and the runtime-API request the serving seam reads, so one rule
/// resolves both and the claim cannot resolve a different domain than the answer
/// does.
#[derive(Debug)]
struct Addressed<'a> {
    host: Option<&'a str>,
    path: &'a str,
    region: String,
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
            region: state::request_region(&parts.headers),
        }
    }

    /// The addressing of a runtime-API request, whose URI arrives as a string
    /// and is parsed by the `http` crate rather than split by hand.
    fn of_request(req: &'a RtRequest, uri: &'a http::Uri) -> Self {
        Self {
            host: req
                .headers()
                .get("host")
                .or_else(|| uri.authority().map(http::uri::Authority::as_str)),
            path: uri.path(),
            region: scoped_region(req.headers().get("authorization")),
        }
    }
}

/// The Region a runtime-API request's SigV4 credential scope names.
///
/// The reading is the store's own ([`state::request_region`]), which takes the
/// header map a handler is given; a runtime-API request carries its headers in
/// the runtime's own type, so the ONE header that reading looks at is
/// re-presented to it as a map rather than a second scope reader being written
/// here.
fn scoped_region(authorization: Option<&str>) -> String {
    let mut headers = ::http::HeaderMap::new();
    if let Some(value) = authorization.and_then(|value| ::http::HeaderValue::from_str(value).ok()) {
        headers.insert(::http::header::AUTHORIZATION, value);
    }
    state::request_region(&headers)
}

/// What a request resolves to: the domain it addresses, as the key this store
/// holds it under, and the path WITHIN that domain's own endpoint — which is
/// what the REST surface is read against.
#[derive(Debug)]
struct Resolved {
    key: (String, String),
    request_path: String,
}

/// The request URI of a runtime-API request, parsed by the `http` crate. A URI
/// this parse rejects is one the server would not have accepted, and it resolves
/// to `http::Uri`'s own default (`/`), which names no domain and claims nothing.
fn parsed_uri(uri: &str) -> http::Uri {
    uri.parse::<http::Uri>().unwrap_or_default()
}

/// Whether this data plane serves the request these `parts` came from — read by
/// `Service::can_handle`, so a request addressed to a domain this service holds
/// reaches this service instead of the front door's `UnknownService`.
pub fn claims_parts<Eng: Send + Sync + 'static>(
    engine: &Eng,
    parts: &http::request::Parts,
) -> bool {
    resolve(engine, &Addressed::of_parts(parts)).is_some()
}

/// [`claims_parts`] for the runtime-API request the serving seams are handed,
/// resolved by the same rule.
pub fn claims_request<Eng: Send + Sync + 'static>(engine: &Eng, req: &RtRequest) -> bool {
    let uri = parsed_uri(req.uri());
    resolve(engine, &Addressed::of_request(req, &uri)).is_some()
}

/// Whether `req` presents a SigV4 credential, which is what decides whether the
/// router verifies a signature before this plane serves it.
pub fn carries_sigv4_credential(req: &RtRequest) -> bool {
    req.headers().get("authorization").is_some_and(|value| {
        value
            .get(..SIGV4_SCHEME.len())
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case(SIGV4_SCHEME))
    }) || req
        .uri()
        .split(['?', '&'])
        .any(|parameter| parameter.starts_with("X-Amz-Signature="))
}

/// Which domain `addressed` addresses, and the path within its endpoint, or
/// `None` when this data plane serves the request not at all.
fn resolve<Eng: Send + Sync + 'static>(engine: &Eng, addressed: &Addressed<'_>) -> Option<Resolved> {
    let state = service_state(engine)?;
    let store = state.lock();

    // PREFIXED PATH-STYLE, the one addressing that is explicit about naming this
    // plane, and so the one read whatever the request's host is.
    if let Some((named, path)) = prefixed_addressing(addressed.path) {
        let key = addressed_domain(&store, named, &addressed.region)?;
        return Some(Resolved {
            key,
            request_path: path.to_string(),
        });
    }
    // A path under the reserved namespace that did NOT name this plane names
    // another one, which serves its own requests.
    if first_segment(addressed.path).is_some_and(|(first, _)| first == EMULATOR_PATH_NAMESPACE) {
        return None;
    }
    // A path this model's own routing table serves is a CONTROL-plane request,
    // claimed by the service's modelled path space rather than by this plane,
    // whatever host it reached.
    if first_segment(addressed.path)
        .is_some_and(|(first, _)| crate::ROUTE_PATH_SEGMENTS.contains(&first))
    {
        return None;
    }
    // HOST-STYLE: the host IS the endpoint host some domain of this store
    // publishes.
    let host = host_only(addressed.host?);
    let (key, _) = store
        .domains
        .iter()
        .find(|(_, domain)| publishes_host(domain, host))?;
    Some(Resolved {
        key: key.clone(),
        request_path: addressed.path.to_string(),
    })
}

/// Whether `domain` publishes `host`: the endpoint host AWS names its data plane
/// by, which is an address a client was told to send index and search requests
/// to.
fn publishes_host(domain: &Domain, host: &str) -> bool {
    domain
        .endpoint
        .as_ref()
        .is_some_and(|endpoint| endpoint == host)
}

/// The domain a prefixed addressing NAMES, as the key this store holds it under:
/// the domain of the request's own Region carrying that `DomainName`, else any
/// domain publishing that endpoint host or carrying that name.
///
/// The Region comes first because a `DomainName` is unique only within one
/// Region (the member's own documentation), so the same name in two Regions is
/// two domains and the request's own credential scope says which of them its
/// client is talking to.
fn addressed_domain(store: &Store, named: &str, region: &str) -> Option<(String, String)> {
    let key = state::domain_key(region, named);
    if store.domains.contains_key(&key) {
        return Some(key);
    }
    store
        .domains
        .iter()
        .find(|((_, name), domain)| publishes_host(domain, named) || name == named)
        .map(|(key, _)| key.clone())
}

/// The domain a prefixed path names and the request path that follows it, or
/// `None` when the path is not of that shape.
fn prefixed_addressing(path: &str) -> Option<(&str, &str)> {
    let (namespace, rest) = first_segment(path)?;
    if namespace != EMULATOR_PATH_NAMESPACE {
        return None;
    }
    let (plane, rest) = first_segment(rest)?;
    if plane != state::DATA_PLANE_SEGMENT {
        return None;
    }
    let (named, request_path) = first_segment(rest)?;
    (!named.is_empty()).then_some((named, request_path))
}

/// One data-plane request, as the surface below reads it.
///
/// `Copy`, so a route that re-reads one request with another body — the
/// auto-create a write into a new index performs — states only what it changes.
#[derive(Debug, Clone, Copy)]
struct Request<'a> {
    method: &'a str,
    path: &'a str,
    query: &'a str,
    body: &'a [u8],
}

impl Request<'_> {
    /// The path's segments, with empty ones dropped so a trailing or doubled `/`
    /// names no segment of its own.
    fn segments(&self) -> Vec<&str> {
        self.path
            .split('/')
            .filter(|part| !part.is_empty())
            .collect()
    }

    /// The value the query string gives `name`, or `None`.
    fn parameter(&self, name: &str) -> Option<&str> {
        self.query.split('&').find_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            (key == name).then_some(value)
        })
    }

    /// The names the query string states, in the order it states them.
    fn parameters(&self) -> impl Iterator<Item = &str> {
        self.query
            .split('&')
            .filter(|pair| !pair.is_empty())
            .map(|pair| pair.split_once('=').map_or(pair, |(key, _)| key))
    }

    /// The request body as the JSON the handler expects, `None` for an empty
    /// body.
    ///
    /// The refusal is BOXED, as every fallible step of this module's reading is:
    /// a rendered response is far larger than what these functions answer with,
    /// and a `Result` whose error dominates its size is paid for on the success
    /// path too.
    fn json(&self) -> Result<Option<Value>, Box<RtResponse>> {
        if self.body.is_empty() {
            return Ok(None);
        }
        ::serde_json::from_slice(self.body)
            .map(Some)
            .map_err(|failed| {
                Box::new(error(
                    400,
                    PARSE_EXCEPTION,
                    &format!("failed to parse: {failed}"),
                ))
            })
    }

    /// The request body as a JSON OBJECT, `None` for an empty body — the shape
    /// every body this surface takes has.
    fn json_object(&self) -> Result<Option<Map<String, Value>>, Box<RtResponse>> {
        match self.json()? {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Object(members)) => Ok(Some(members)),
            Some(_) => Err(Box::new(error(
                400,
                PARSE_EXCEPTION,
                "failed to parse: expected the body to be a JSON object",
            ))),
        }
    }
}

/// Answer a request this plane claimed, out of the records the control plane
/// holds.
pub fn serve<Eng: Send + Sync + 'static>(engine: &Eng, req: &RtRequest) -> RtResponse {
    let uri = parsed_uri(req.uri());
    let Some(resolved) = resolve(engine, &Addressed::of_request(req, &uri)) else {
        // The domain was released by another request between the claim and here,
        // so the host it published answers nothing.
        return cluster_unavailable();
    };
    let method = req.method().to_ascii_uppercase();
    let body = req.body().bytes().unwrap_or_default();
    if body.len() > state::MAX_REQUEST_BODY_BYTES {
        return error(
            413,
            ILLEGAL_ARGUMENT_EXCEPTION,
            &format!(
                "request body is larger than the maximum of {} bytes this cluster takes",
                state::MAX_REQUEST_BODY_BYTES
            ),
        );
    }
    let request = Request {
        method: &method,
        path: &resolved.request_path,
        query: uri.query().unwrap_or_default(),
        body,
    };
    let now = SystemTime::now();
    let Some(state) = service_state(engine) else {
        return cluster_unavailable();
    };
    let mut store = state.lock();
    let Some(domain) = store.domains.get_mut(&resolved.key) else {
        return cluster_unavailable();
    };
    if !domain.cluster_exists(now) {
        // The cluster behind the endpoint is not up: the domain is still being
        // provisioned, or it is being torn down.
        return cluster_unavailable();
    }
    serve_resolved(domain, &request, now)
}

/// Answer the request against the domain it resolved to.
fn serve_resolved(domain: &mut Domain, request: &Request<'_>, now: SystemTime) -> RtResponse {
    let segments = request.segments();
    // What the request ASKS is its path, its body and its parameters: a parameter
    // this cluster does not evaluate is refused here, once, for every route below,
    // rather than dropped — a conditional write answered `created` as if its
    // precondition had held, or a narrowed read answered whole, answers a
    // different question from the one that was asked
    // ([`recognized_parameters`]).
    if let Some(refusal) = unrecognized_parameter(request, recognized_parameters(&segments)) {
        return *refusal;
    }
    match segments.as_slice() {
        [] => match request.method {
            "GET" | "HEAD" => ok(&banner(domain)),
            _ => wrong_method(request, "GET, HEAD"),
        },
        ["_cluster", "health"] => match request.method {
            "GET" => cluster_health(domain, request, None),
            _ => wrong_method(request, "GET"),
        },
        ["_cluster", "health", index] => match request.method {
            "GET" => cluster_health(domain, request, Some(index)),
            _ => wrong_method(request, "GET"),
        },
        // The cluster-wide spellings of the index-scoped routes below. Each comes
        // first, so the reserved name is never read as an index.
        ["_refresh"] => match request.method {
            "GET" | "POST" => refresh(domain, None),
            _ => wrong_method(request, "GET, POST"),
        },
        ["_count"] => match request.method {
            "GET" | "POST" => count(domain, request, None),
            _ => wrong_method(request, "GET, POST"),
        },
        ["_search"] => match request.method {
            "GET" | "POST" => search_request(domain, request, None),
            _ => wrong_method(request, "GET, POST"),
        },
        // The index-level routes. Each is guarded by [`reserved`]: a first
        // segment that begins with the reserved prefix names an API of the
        // engine's own and never an index, so a reserved path this surface does
        // not serve is answered as an unserved path rather than as a request
        // about an index no cluster could hold.
        [index] if !reserved(index) => index_request(domain, request, index, now),
        [index, "_doc"] if !reserved(index) => {
            document_write(domain, request, index, None, Creates::Upsert, now)
        }
        [index, "_doc", id] if !reserved(index) => {
            document_request(domain, request, index, id, requested_op_type(request), now)
        }
        [index, "_create", id] if !reserved(index) => {
            document_request(domain, request, index, id, Creates::Only, now)
        }
        [index, "_settings"] if !reserved(index) => match request.method {
            "GET" => index_settings(domain, index),
            _ => wrong_method(request, "GET"),
        },
        [index, "_mapping"] if !reserved(index) => match request.method {
            "GET" => index_mappings(domain, index),
            _ => wrong_method(request, "GET"),
        },
        [index, "_refresh"] if !reserved(index) => match request.method {
            "GET" | "POST" => refresh(domain, Some(index)),
            _ => wrong_method(request, "GET, POST"),
        },
        [index, "_count"] if !reserved(index) => match request.method {
            "GET" | "POST" => count(domain, request, Some(index)),
            _ => wrong_method(request, "GET, POST"),
        },
        [index, "_search"] if !reserved(index) => match request.method {
            "GET" | "POST" => search_request(domain, request, Some(index)),
            _ => wrong_method(request, "GET, POST"),
        },
        _ => no_handler(request),
    }
}

/// Whether a write may REPLACE a document the index already holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Creates {
    /// `_doc` without an `op_type`: a write to an id the index holds replaces it.
    Upsert,
    /// `_create`, and `_doc` with `op_type=create`: a write to an id the index
    /// already holds is REFUSED rather than replacing the document.
    Only,
}

/// What `op_type` asks of a `_doc` write: `create` is the one value that forbids
/// a replacement, and every other (including none) leaves the write an upsert.
fn requested_op_type(request: &Request<'_>) -> Creates {
    match request.parameter(OP_TYPE) {
        Some("create") => Creates::Only,
        _ => Creates::Upsert,
    }
}

/// Whether `segment` is a RESERVED first path segment of the engine's REST
/// surface rather than an index name.
///
/// An index name may not begin with the reserved prefix
/// ([`state::RESERVED_SEGMENT_PREFIX`]), so a first segment that does names an
/// API — served above, or served by nothing. Every index-level route is guarded
/// by this, which is what keeps a reserved endpoint this surface does not serve
/// from being answered as a request about an index: the refusal then states that
/// the path has no handler instead of listing the methods some other route
/// takes.
fn reserved(segment: &str) -> bool {
    // `_all` is the one reserved name that IS an index selector — it names every
    // index of the cluster — so it is not an API name and the index-level routes
    // take it.
    segment.starts_with(state::RESERVED_SEGMENT_PREFIX) && segment != state::ALL_INDEXES
}

/// The cluster banner a `GET /` answers: what this cluster IS, from the domain's
/// own record.
fn banner(domain: &Domain) -> Value {
    let (distribution, number) = engine_release(domain);
    let mut version = Map::new();
    version.insert("number".to_string(), json!(number));
    if let Some(distribution) = &distribution {
        version.insert("distribution".to_string(), json!(distribution));
    }
    json!({
        "name": domain.domain_name,
        "cluster_name": cluster_name(domain),
        "cluster_uuid": stable_uuid(&domain.arn),
        "version": Value::Object(version),
        "tagline": tagline(distribution.is_some()),
    })
}

/// The tagline a cluster answers a `GET /` with: the Elasticsearch one for an
/// Elasticsearch domain, and the OpenSearch project's for a domain running the
/// engine this service's version strings also name.
fn tagline(opensearch: bool) -> &'static str {
    if opensearch {
        "The OpenSearch Project: https://opensearch.org/"
    } else {
        "You Know, for Search"
    }
}

/// A domain's cluster name, which the service forms from the account that owns
/// it and the domain's own name.
fn cluster_name(domain: &Domain) -> String {
    format!("{}:{}", ::engine::emulator_account_id(), domain.domain_name)
}

/// The distribution and the version NUMBER the domain's stored
/// `ElasticsearchVersion` names: `None` for an Elasticsearch domain, whose
/// version is the bare `X.Y`, and the engine's own name for a domain whose
/// version names the OpenSearch lineage (`OpenSearch_X.Y`, `OS_X.Y`). The patch
/// level the stored string omits is reported as 0.
fn engine_release(domain: &Domain) -> (Option<String>, String) {
    let stored = domain
        .config
        .elasticsearch_version
        .as_ref()
        .map_or("", |record| record.value.as_str());
    let version = stored.split_once('_').map_or(stored, |(_, rest)| rest);
    let number = if version.is_empty() {
        String::new()
    } else if version.split('.').count() < 3 {
        format!("{version}.0")
    } else {
        version.to_string()
    };
    let distribution = (state::engine_type_of(Some(stored)) != "Elasticsearch")
        .then(|| "opensearch".to_string());
    (distribution, number)
}

/// An id that is a function of what it names, so the same resource reports the
/// same id on every read and across restarts without anything being remembered.
///
/// A 128-bit SHA-256 prefix over the name, written in the hyphenated form a
/// cluster and index id is reported in.
fn stable_uuid(seed: &str) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(seed.as_bytes());
    let hex = hex::encode(digest);
    let group = |from: usize, to: usize| hex.get(from..to).unwrap_or_default().to_string();
    format!(
        "{}-{}-{}-{}-{}",
        group(0, 8),
        group(8, 12),
        group(12, 16),
        group(16, 20),
        group(20, 32)
    )
}

/// The cluster health a `GET /_cluster/health` answers, computed from the
/// domain's own cluster configuration and the indexes the request is about:
/// every index the domain holds, or the ONE the path names.
///
/// A shard copy needs a node of its own, so an index asking for more replica
/// copies than the cluster has nodes to put them on leaves copies unassigned and
/// the cluster is `yellow`; with every copy assigned it is `green`.
///
/// `wait_for_status` is the one wait this emulator can answer without waiting:
/// the cluster is already settled, so the status it has is the status it will
/// have — a request asking for a healthier one than it reached is answered
/// `timed_out: true` with the status it really is, which is what the engine
/// answers when the wait expires.
fn cluster_health(
    domain: &Domain,
    request: &Request<'_>,
    index_name: Option<&str>,
) -> RtResponse {
    let named = match selected_indexes(domain, index_name) {
        Ok(named) => named,
        Err(refusal) => return *refusal,
    };
    let data_nodes = domain.data_nodes();
    let master_nodes = if domain.config.cluster_config.value.dedicated_master_enabled == Some(true) {
        domain
            .config
            .cluster_config
            .value
            .dedicated_master_count
            .unwrap_or(0)
    } else {
        0
    };
    let mut primaries = 0_i64;
    let mut replicas = 0_i64;
    let mut assignable = 0_i64;
    for index in &named {
        let (shards, copies) = shard_layout(index);
        primaries = primaries.saturating_add(shards);
        replicas = replicas.saturating_add(shards.saturating_mul(copies));
        assignable = assignable.saturating_add(
            shards.saturating_mul(state::assigned_replicas(copies, data_nodes)),
        );
    }
    let unassigned = replicas.saturating_sub(assignable);
    let active = primaries.saturating_add(assignable);
    let total = primaries.saturating_add(replicas);
    let status = if unassigned > 0 { "yellow" } else { "green" };
    let timed_out = request
        .parameter(WAIT_FOR_STATUS)
        .and_then(health_rank)
        .is_some_and(|wanted| health_rank(status).is_some_and(|reached| reached > wanted));
    let percent = if total == 0 {
        100.0
    } else {
        #[allow(clippy::cast_precision_loss)]
        {
            (active as f64 / total as f64) * 100.0
        }
    };
    ok(&json!({
        "cluster_name": cluster_name(domain),
        "status": status,
        "timed_out": timed_out,
        "number_of_nodes": i64::from(data_nodes) + i64::from(master_nodes),
        "number_of_data_nodes": i64::from(data_nodes),
        "active_primary_shards": primaries,
        "active_shards": active,
        "relocating_shards": 0,
        "initializing_shards": 0,
        "unassigned_shards": unassigned,
        "delayed_unassigned_shards": 0,
        "number_of_pending_tasks": 0,
        "number_of_in_flight_fetch": 0,
        "task_max_waiting_in_queue_millis": 0,
        "active_shards_percent_as_number": percent,
    }))
}

/// How the cluster health statuses order, worst last — what `wait_for_status`
/// compares against. `None` for a value that names none of them.
fn health_rank(status: &str) -> Option<u8> {
    match status {
        "green" => Some(0),
        "yellow" => Some(1),
        "red" => Some(2),
        _ => None,
    }
}

/// The indexes a request naming `index_name` is about: the ONE index of that
/// name, every index of the cluster for the reserved all-indexes selector or for
/// a request naming none, and the refusal for a name the cluster holds no index
/// of.
fn selected_indexes<'d>(
    domain: &'d Domain,
    index_name: Option<&str>,
) -> Result<Vec<&'d SearchIndex>, Box<RtResponse>> {
    match index_name {
        None | Some(state::ALL_INDEXES) => Ok(domain.indexes.values().collect()),
        Some(name) => match domain.indexes.get(name) {
            Some(index) => Ok(vec![index]),
            None => Err(Box::new(index_not_found(name))),
        },
    }
}

/// One index's shard layout, as its own settings state it: how many primary
/// shards it has and how many replica copies of each it asks for.
///
/// Both come from settings a caller wrote, so both are clamped before anything
/// multiplies them out.
fn shard_layout(index: &SearchIndex) -> (i64, i64) {
    let shards = index
        .setting_number(NUMBER_OF_SHARDS)
        .unwrap_or(DEFAULT_SHARD_COUNT)
        .clamp(1, MAX_SHARDS_PER_INDEX);
    let copies = index
        .setting_number(NUMBER_OF_REPLICAS)
        .unwrap_or(DEFAULT_SHARD_COUNT)
        .clamp(0, MAX_SHARDS_PER_INDEX);
    (shards, copies)
}

/// A `PUT`, `GET`, `HEAD` or `DELETE` on one index.
fn index_request(
    domain: &mut Domain,
    request: &Request<'_>,
    index_name: &str,
    now: SystemTime,
) -> RtResponse {
    match request.method {
        "PUT" => create_index(domain, request, index_name, now),
        "GET" => read_index(domain, index_name),
        "HEAD" => {
            if domain.indexes.contains_key(index_name) {
                status_only(200)
            } else {
                status_only(404)
            }
        }
        "DELETE" => match domain.indexes.remove(index_name) {
            Some(_) => ok(&json!({ "acknowledged": true })),
            None => index_not_found(index_name),
        },
        _ => wrong_method(request, "PUT, GET, HEAD, DELETE"),
    }
}

/// Create the index the request names, carrying the sections its body gives.
fn create_index(
    domain: &mut Domain,
    request: &Request<'_>,
    index_name: &str,
    now: SystemTime,
) -> RtResponse {
    if let Some(refusal) = index_name_refused(index_name) {
        return *refusal;
    }
    if domain.indexes.contains_key(index_name) {
        return indexed_error(
            400,
            RESOURCE_ALREADY_EXISTS_EXCEPTION,
            &format!(
                "index [{index_name}/{}] already exists",
                stable_uuid(&format!("{}/{index_name}", domain.arn))
            ),
            index_name,
        );
    }
    if domain.indexes.len() >= state::INDEX_QUOTA {
        return error(
            400,
            ILLEGAL_ARGUMENT_EXCEPTION,
            &format!(
                "domain [{}] already holds the maximum of {} indexes, so index [{index_name}] \
                 could not be created",
                domain.domain_name,
                state::INDEX_QUOTA
            ),
        );
    }
    let requested = match request.json_object() {
        Ok(requested) => requested.unwrap_or_default(),
        Err(refusal) => return *refusal,
    };
    let mut settings = Map::new();
    let mut mappings = Map::new();
    for (section, value) in &requested {
        match section.as_str() {
            "settings" => settings = index_level_settings(value),
            "mappings" => {
                if let Value::Object(members) = value {
                    mappings = members.clone();
                }
            }
            // Every other section of a create is one this cluster does not hold:
            // it is refused rather than accepted and dropped, so nothing is
            // reported created that the index does not carry.
            other => {
                return error(
                    400,
                    ILLEGAL_ARGUMENT_EXCEPTION,
                    &format!("unknown key [{other}] for create index"),
                )
            }
        }
    }
    domain.indexes.insert(
        index_name.to_string(),
        SearchIndex::create(index_name, settings, mappings, now),
    );
    ok(&json!({
        "acknowledged": true,
        "shards_acknowledged": true,
        "index": index_name,
    }))
}

/// The INDEX-LEVEL settings a create's `settings` section states: the members of
/// its `index` sub-object, and the members it states flat, each with the
/// `index.` prefix the engine also takes removed — so one index's settings are
/// held under one spelling however the create wrote them.
fn index_level_settings(section: &Value) -> Map<String, Value> {
    let Value::Object(members) = section else {
        return Map::new();
    };
    let mut settings = Map::new();
    for (name, value) in members {
        match (name.as_str(), value) {
            ("index", Value::Object(nested)) => {
                for (name, value) in nested {
                    settings.insert(name.clone(), value.clone());
                }
            }
            (name, value) => {
                let name = name.strip_prefix("index.").unwrap_or(name);
                settings.insert(name.to_string(), value.clone());
            }
        }
    }
    settings
}

/// An index's own settings and mappings, as a `GET /{index}` reports them.
fn read_index(domain: &Domain, index_name: &str) -> RtResponse {
    let Some(index) = domain.indexes.get(index_name) else {
        return index_not_found(index_name);
    };
    ok(&json!({
        index_name: {
            "mappings": Value::Object(index.mappings.clone()),
            "settings": { "index": settings_view(domain, index) },
        }
    }))
}

/// An index's settings as every reader of them reports them: what its create
/// stated, under the settings the index HAS whether or not the create named them
/// — the two shard counts, which default to the engine's own, and the members
/// the service itself fills in.
///
/// Reported as the engine reports a setting, as text: a client that reads a
/// setting back and writes it again sends what it was given.
fn settings_view(domain: &Domain, index: &SearchIndex) -> Value {
    let (shards, copies) = shard_layout(index);
    let mut reported = Map::new();
    reported.insert(
        "creation_date".to_string(),
        json!(millis_since_epoch(index.created_at).to_string()),
    );
    reported.insert(NUMBER_OF_SHARDS.to_string(), json!(shards.to_string()));
    reported.insert(NUMBER_OF_REPLICAS.to_string(), json!(copies.to_string()));
    reported.insert(
        "uuid".to_string(),
        json!(stable_uuid(&format!("{}/{}", domain.arn, index.index_name))),
    );
    reported.insert("provided_name".to_string(), json!(index.index_name));
    // What the create stated wins over the defaults above: those are the values
    // the index has when its create named none.
    for (name, value) in &index.settings {
        reported.insert(name.clone(), text_setting(value));
    }
    Value::Object(reported)
}

/// One setting as the engine reports it: a scalar as the text it is written
/// with, and a structured value as it was stated.
fn text_setting(value: &Value) -> Value {
    match value {
        Value::String(_) | Value::Object(_) | Value::Array(_) | Value::Null => value.clone(),
        scalar => json!(scalar.to_string()),
    }
}

/// Milliseconds since the epoch, which is how the engine states an instant in a
/// setting.
fn millis_since_epoch(at: SystemTime) -> u128 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

/// An index's settings alone, as a `GET /{index}/_settings` reports them.
fn index_settings(domain: &Domain, index_name: &str) -> RtResponse {
    let Some(index) = domain.indexes.get(index_name) else {
        return index_not_found(index_name);
    };
    ok(&json!({
        index_name: { "settings": { "index": settings_view(domain, index) } }
    }))
}

/// An index's mappings alone, as a `GET /{index}/_mapping` reports them.
fn index_mappings(domain: &Domain, index_name: &str) -> RtResponse {
    let Some(index) = domain.indexes.get(index_name) else {
        return index_not_found(index_name);
    };
    ok(&json!({
        index_name: { "mappings": Value::Object(index.mappings.clone()) }
    }))
}

/// A `GET`, `PUT`, `POST` or `DELETE` on one document addressed by its id.
fn document_request(
    domain: &mut Domain,
    request: &Request<'_>,
    index_name: &str,
    id: &str,
    creates: Creates,
    now: SystemTime,
) -> RtResponse {
    match request.method {
        "PUT" | "POST" => document_write(domain, request, index_name, Some(id), creates, now),
        "GET" => read_document(domain, index_name, id),
        "DELETE" => delete_document(domain, index_name, id),
        _ => wrong_method(request, "GET, PUT, POST, DELETE"),
    }
}

/// One document, as a `GET /{index}/_doc/{id}` reports it.
fn read_document(domain: &Domain, index_name: &str, id: &str) -> RtResponse {
    let Some(index) = domain.indexes.get(index_name) else {
        return index_not_found(index_name);
    };
    match index.documents.get(id) {
        Some(document) => ok(&json!({
            "_index": index_name,
            "_id": id,
            "_version": document.version,
            "_seq_no": document.seq_no,
            "_primary_term": document.primary_term,
            "found": true,
            "_source": document.source,
        })),
        None => rendered(
            404,
            &json!({ "_index": index_name, "_id": id, "found": false }),
        ),
    }
}

/// One document removed, as a `DELETE /{index}/_doc/{id}` reports it.
fn delete_document(domain: &mut Domain, index_name: &str, id: &str) -> RtResponse {
    let shards = write_shards(domain, index_name);
    let Some(index) = domain.indexes.get_mut(index_name) else {
        return index_not_found(index_name);
    };
    // A delete takes a sequence number of its own, which is what lets a client
    // tell a delete it made from one someone else made.
    let seq_no = index.take_seq_no();
    let (status, result, version) = match index.documents.remove(id) {
        Some(document) => (200, "deleted", document.version.saturating_add(1)),
        None => (404, "not_found", 1),
    };
    rendered(
        status,
        &json!({
            "_index": index_name,
            "_id": id,
            "_version": version,
            "result": result,
            "_shards": shards,
            "_seq_no": seq_no,
            "_primary_term": state::FIRST_PRIMARY_TERM,
        }),
    )
}

/// Write one document into the index the request names, creating that index when
/// the cluster holds none of that name — which is what the engine's own
/// auto-create does for a write to a new index.
fn document_write(
    domain: &mut Domain,
    request: &Request<'_>,
    index_name: &str,
    id: Option<&str>,
    creates: Creates,
    now: SystemTime,
) -> RtResponse {
    if !matches!(request.method, "POST" | "PUT") {
        return wrong_method(request, "POST, PUT");
    }
    let source = match request.json_object() {
        Ok(Some(source)) => Value::Object(source),
        Ok(None) => {
            return error(
                400,
                PARSE_EXCEPTION,
                "failed to parse: request body is required",
            )
        }
        Err(refusal) => return *refusal,
    };
    if !domain.indexes.contains_key(index_name) {
        // The write creates the index it names, so a name no index may carry is
        // refused here — with the clause the engine states for it — and nothing
        // is stored under it.
        let created = create_index(domain, &Request { body: &[], ..*request }, index_name, now);
        if !created.status().is_success() {
            return created;
        }
    }
    let id = match id.filter(|id| !id.is_empty()) {
        Some(id) => id.to_string(),
        // A write that names no id is answered one the cluster mints for it,
        // derived from the index it lands in and the sequence number this write
        // takes, so two writes never land under one id.
        None => minted_document_id(domain, index_name),
    };
    let shards = write_shards(domain, index_name);
    let Some(index) = domain.indexes.get_mut(index_name) else {
        return cluster_unavailable();
    };
    if creates == Creates::Only {
        if let Some(held) = index.documents.get(&id) {
            return rendered(
                409,
                &json!({
                    "error": {
                        "root_cause": [{
                            "type": VERSION_CONFLICT_ENGINE_EXCEPTION,
                            "reason": format!(
                                "[{id}]: version conflict, document already exists (current \
                                 version [{}])",
                                held.version
                            ),
                            "index": index_name,
                            "index_uuid": NO_INDEX_UUID,
                            "shard": "0",
                        }],
                        "type": VERSION_CONFLICT_ENGINE_EXCEPTION,
                        "reason": format!(
                            "[{id}]: version conflict, document already exists (current version \
                             [{}])",
                            held.version
                        ),
                        "index": index_name,
                        "index_uuid": NO_INDEX_UUID,
                        "shard": "0",
                    },
                    "status": 409,
                }),
            );
        }
    }
    if index.documents.len() >= state::INDEX_DOCUMENT_QUOTA && !index.documents.contains_key(&id) {
        return error(
            429,
            ILLEGAL_ARGUMENT_EXCEPTION,
            &format!(
                "index [{index_name}] already holds the maximum of {} documents this cluster \
                 takes",
                state::INDEX_DOCUMENT_QUOTA
            ),
        );
    }
    let written = index.write_document(&id, source);
    let created = written.version == 1;
    rendered(
        if created { 201 } else { 200 },
        &json!({
            "_index": index_name,
            "_id": id,
            "_version": written.version,
            "result": if created { "created" } else { "updated" },
            "_shards": shards,
            "_seq_no": written.seq_no,
            "_primary_term": written.primary_term,
        }),
    )
}

/// The id the cluster mints for a write that named none: a function of the index
/// it lands in and the sequence number this write will take, so it is unique
/// within the index and stable for one write.
fn minted_document_id(domain: &Domain, index_name: &str) -> String {
    let seq_no = domain
        .indexes
        .get(index_name)
        .map_or(0, |index| index.next_seq_no);
    let seed = format!("{}/{index_name}/{seq_no}", domain.arn);
    use sha2::Digest;
    let digest = sha2::Sha256::digest(seed.as_bytes());
    hex::encode(digest).get(..20).unwrap_or_default().to_string()
}

/// The `_shards` a write reports: the copies of the shard it landed on, and how
/// many of them the cluster's topology could assign.
fn write_shards(domain: &Domain, index_name: &str) -> Value {
    let copies = domain
        .indexes
        .get(index_name)
        .map_or(DEFAULT_SHARD_COUNT, |index| shard_layout(index).1);
    let assigned = state::assigned_replicas(copies, domain.data_nodes());
    json!({
        "total": copies.saturating_add(1),
        "successful": assigned.saturating_add(1),
        "failed": 0,
    })
}

/// What a `_refresh` answers: the shards the refresh ran on, over the indexes it
/// was asked about — every index the domain holds, or the ONE the path names.
///
/// A refresh makes recent operations available for search: it flushes what is
/// already written, so it changes nothing about a document and reports none.
/// Every write this emulator takes is visible to the next read the moment it
/// lands, so what is left to answer is the shard summary, computed from each
/// index's own settings and the nodes the domain has.
fn refresh(domain: &Domain, index_name: Option<&str>) -> RtResponse {
    let named = match selected_indexes(domain, index_name) {
        Ok(named) => named,
        Err(refusal) => return *refusal,
    };
    let data_nodes = domain.data_nodes();
    let mut total = 0_i64;
    let mut successful = 0_i64;
    for index in &named {
        let (shards, copies) = shard_layout(index);
        let assigned = state::assigned_replicas(copies, data_nodes);
        total = total.saturating_add(shards.saturating_mul(copies.saturating_add(1)));
        successful =
            successful.saturating_add(shards.saturating_mul(assigned.saturating_add(1)));
    }
    ok(&json!({
        "_shards": { "total": total, "successful": successful, "failed": 0 },
    }))
}

/// The count a `_count` answers, over the documents the selected indexes hold.
fn count(domain: &Domain, request: &Request<'_>, index_name: Option<&str>) -> RtResponse {
    // No hit is reported by a count, so none is materialised for it.
    let selected = match selected_documents(domain, request, index_name, None) {
        Ok(selected) => selected,
        Err(refusal) => return *refusal,
    };
    ok(&json!({
        "count": selected.total,
        "_shards": searched_shards(selected.shards),
    }))
}

/// The `_shards` a read over `shards` primary shards reports: every primary the read
/// asked was searched, none skipped and none failed — there is one copy of each
/// primary here and it is always available, which is what makes a read of this
/// cluster never partial.
fn searched_shards(shards: i64) -> Value {
    json!({ "total": shards, "successful": shards, "skipped": 0, "failed": 0 })
}

/// One hit of a search: the index that holds the document, its id and its source.
struct Hit {
    index_name: String,
    id: String,
    source: Value,
}

/// What a query selected: how many documents it HOLDS of, the hits inside the
/// window the request asked for, and how many primary shards the read was asked
/// over — the indexes' own `number_of_shards`, which is what a read reports it
/// searched.
struct Selection {
    total: usize,
    hits: Vec<Hit>,
    shards: i64,
}

/// The documents the request's query selects, over the indexes it is about — in
/// index order and then id order, so two searches of one cluster answer the same
/// hits in the same order.
///
/// `window` is the `(from, size)` the request asked for, and it bounds what this
/// MATERIALISES: a document outside it is counted and not copied, so a small
/// request cannot make the cluster hand back everything it holds. A count asks
/// for no window at all and materialises nothing.
fn selected_documents(
    domain: &Domain,
    request: &Request<'_>,
    index_name: Option<&str>,
    window: Option<(usize, usize)>,
) -> Result<Selection, Box<RtResponse>> {
    let named = selected_indexes(domain, index_name)?;
    let body = request.json_object()?.unwrap_or_default();
    // A count's body states the query and nothing else; a search's may also state
    // the window ([`SEARCH_BODY_SECTIONS`]). Any other section is refused rather
    // than dropped, so a request this cluster cannot answer is never answered as
    // if it had asked something else.
    let sections: &[&str] = match window {
        None => &[QUERY_SECTION],
        Some(_) => SEARCH_BODY_SECTIONS,
    };
    if let Some(refusal) = unevaluated_section(&body, sections) {
        return Err(refusal);
    }
    let selector = search::Selector::of(body.get(QUERY_SECTION)).map_err(|unreadable| {
        Box::new(error(400, unreadable.error_type, &unreadable.reason))
    })?;
    let (from, size) = window.unwrap_or((0, 0));
    let mut selected = Selection {
        total: 0,
        hits: Vec::new(),
        shards: 0,
    };
    for index in named {
        selected.shards = selected.shards.saturating_add(shard_layout(index).0);
        for (id, document) in &index.documents {
            if !selector.selects(id, &document.source) {
                continue;
            }
            let reached = selected.total;
            selected.total = selected.total.saturating_add(1);
            if reached >= from && selected.hits.len() < size {
                selected.hits.push(Hit {
                    index_name: index.index_name.clone(),
                    id: id.clone(),
                    source: document.source.clone(),
                });
            }
        }
    }
    Ok(selected)
}

/// The hits a `_search` answers, over the documents the selected indexes hold.
fn search_request(
    domain: &Domain,
    request: &Request<'_>,
    index_name: Option<&str>,
) -> RtResponse {
    let body = match request.json_object() {
        Ok(body) => body.unwrap_or_default(),
        Err(refusal) => return *refusal,
    };
    // Before anything is read out of the body: a section this cluster does not
    // evaluate is refused, not ignored. A search that asked for an ORDER, an
    // AGGREGATION or a narrowed `_source` and was answered 200 with the cluster's
    // own order, no aggregations and whole documents would be answered a
    // different question from the one it asked.
    if let Some(refusal) = unevaluated_section(&body, SEARCH_BODY_SECTIONS) {
        return *refusal;
    }
    let from = match window_member(&body, request, "from", 0) {
        Ok(from) => from,
        Err(refusal) => return *refusal,
    };
    let size = match window_member(&body, request, "size", DEFAULT_SEARCH_SIZE) {
        Ok(size) => size,
        Err(refusal) => return *refusal,
    };
    // The engine's own bound on how much one search may answer with, which is
    // also what bounds the hits this request materialises: past it the search is
    // refused rather than served.
    if from.saturating_add(size) > MAX_RESULT_WINDOW {
        return error(
            400,
            ILLEGAL_ARGUMENT_EXCEPTION,
            &format!(
                "Result window is too large, from + size must be less than or equal to: \
                 [{MAX_RESULT_WINDOW}] but was [{}].",
                from.saturating_add(size)
            ),
        );
    }
    let selected = match selected_documents(domain, request, index_name, Some((from, size))) {
        Ok(selected) => selected,
        Err(refusal) => return *refusal,
    };
    let total = selected.total;
    let hits: Vec<Value> = selected
        .hits
        .into_iter()
        .map(|hit| {
            json!({
                "_index": hit.index_name,
                "_id": hit.id,
                "_score": search::MATCH_SCORE,
                "_source": hit.source,
            })
        })
        .collect();
    ok(&json!({
        "timed_out": false,
        "_shards": searched_shards(selected.shards),
        "hits": {
            "total": { "value": total, "relation": "eq" },
            "max_score": if total == 0 { Value::Null } else { json!(search::MATCH_SCORE) },
            "hits": hits,
        },
    }))
}

/// One member of the result window a search asks for (`from`, `size`): the body's
/// value, else the query parameter of the same name, else the engine's default.
///
/// A value that is not a whole number at all is refused rather than read as the
/// default, since a client that asked for a window got none of it.
fn window_member(
    body: &Map<String, Value>,
    request: &Request<'_>,
    name: &str,
    default: usize,
) -> Result<usize, Box<RtResponse>> {
    let stated = match body.get(name) {
        Some(Value::Number(number)) => Some(number.as_i64()),
        Some(_) => Some(None),
        None => request
            .parameter(name)
            .map(|value| value.parse::<i64>().ok()),
    };
    match stated {
        None => Ok(default),
        Some(Some(stated)) if stated >= 0 => Ok(usize::try_from(stated).unwrap_or(default)),
        Some(_) => Err(Box::new(error(
            400,
            ILLEGAL_ARGUMENT_EXCEPTION,
            &format!("Failed to parse int parameter [{name}] with value [{}]", {
                body.get(name).map_or_else(
                    || request.parameter(name).unwrap_or_default().to_string(),
                    ToString::to_string,
                )
            }),
        ))),
    }
}

/// The refusal for the FIRST section of `body` that is not one of `evaluated`, or
/// `None` for a body naming none — stated as the engine states a key it does not
/// read in that position, which names the section and the kind of value it
/// carried.
fn unevaluated_section(
    body: &Map<String, Value>,
    evaluated: &[&str],
) -> Option<Box<RtResponse>> {
    let (section, value) = body
        .iter()
        .find(|(section, _)| !evaluated.contains(&section.as_str()))?;
    Some(Box::new(error(
        400,
        search::PARSING_EXCEPTION,
        &format!(
            "Unknown key for a {token} in [{section}].",
            token = json_token(value)
        ),
    )))
}

/// The name the engine's own JSON reader gives the token a value arrives as, which
/// its refusal of an unread key quotes.
fn json_token(value: &Value) -> &'static str {
    match value {
        Value::Object(_) => "START_OBJECT",
        Value::Array(_) => "START_ARRAY",
        Value::String(_) => "VALUE_STRING",
        Value::Number(_) => "VALUE_NUMBER",
        Value::Bool(_) => "VALUE_BOOLEAN",
        Value::Null => "VALUE_NULL",
    }
}

/// The refusal for the FIRST parameter of `request` that is not one of
/// `recognized`, or `None` for a request naming none — stated as the engine states
/// a parameter it does not read, which names the path and the parameter.
///
/// A parameter this cluster does not evaluate is refused for the same reason a body
/// section is: a search narrowed, ordered or routed by a parameter and then
/// answered as if it had named none is answered a different question.
fn unrecognized_parameter(
    request: &Request<'_>,
    recognized: &[&str],
) -> Option<Box<RtResponse>> {
    let named = request.parameters().find(|name| {
        // A parameter with no value at all names nothing to evaluate.
        !name.is_empty() && !recognized.contains(name)
    })?;
    Some(Box::new(error(
        400,
        ILLEGAL_ARGUMENT_EXCEPTION,
        &format!(
            "request [{path}] contains unrecognized parameter: [{named}]",
            path = request.path
        ),
    )))
}

/// The refusal for an index name no index may carry, or `None` for one that may.///
/// The rule and the clause are the engine's ([`state::index_name_refusal`]), so
/// a create and a write that auto-creates refuse one name the same way.
fn index_name_refused(index_name: &str) -> Option<Box<RtResponse>> {
    let clause = state::index_name_refusal(index_name)?;
    Some(Box::new(invalid_index_name(index_name, &clause)))
}

/// The refusal the engine answers an index name outside its rule: 400, naming
/// the name and the clause it broke.
fn invalid_index_name(index_name: &str, clause: &str) -> RtResponse {
    let reason = format!("Invalid index name [{index_name}], {clause}");
    let cause = json!({
        "type": INVALID_INDEX_NAME_EXCEPTION,
        "reason": reason,
        "index": index_name,
        "index_uuid": NO_INDEX_UUID,
    });
    rendered(
        400,
        &json!({
            "error": {
                "root_cause": [cause.clone()],
                "type": INVALID_INDEX_NAME_EXCEPTION,
                "reason": reason,
                "index": index_name,
                "index_uuid": NO_INDEX_UUID,
            },
            "status": 400,
        }),
    )
}

/// The refusal for an index the cluster does not hold.
fn index_not_found(index_name: &str) -> RtResponse {
    indexed_error(
        404,
        INDEX_NOT_FOUND_EXCEPTION,
        &format!("no such index [{index_name}]"),
        index_name,
    )
}

/// An error naming the index it is about, which is what the engine's index
/// errors carry beside the type and the reason.
fn indexed_error(status: u16, error_type: &str, reason: &str, index_name: &str) -> RtResponse {
    let cause = json!({
        "type": error_type,
        "reason": reason,
        "index": index_name,
        "index_uuid": NO_INDEX_UUID,
        "resource.type": "index_or_alias",
        "resource.id": index_name,
    });
    rendered(
        status,
        &json!({
            "error": {
                "root_cause": [cause.clone()],
                "type": error_type,
                "reason": reason,
                "index": index_name,
                "index_uuid": NO_INDEX_UUID,
                "resource.type": "index_or_alias",
                "resource.id": index_name,
            },
            "status": status,
        }),
    )
}

/// The answer for a domain whose cluster is not up: still being provisioned,
/// being torn down, or released between the claim and the serving.
fn cluster_unavailable() -> RtResponse {
    error(
        503,
        CLUSTER_BLOCK_EXCEPTION,
        "blocked by: [SERVICE_UNAVAILABLE/1/state not recovered / initialized]",
    )
}

/// The refusal the engine answers a path it has no handler registered for.
fn no_handler(request: &Request<'_>) -> RtResponse {
    rendered(
        400,
        &json!({
            "error": format!(
                "no handler found for uri [{}] and method [{}]",
                request.path, request.method
            )
        }),
    )
}

/// The refusal the engine answers a path it DOES serve, reached with a method
/// that path does not take: a different class of failure from a path it serves
/// nothing for, and worded as the engine words it, naming the methods that are
/// allowed.
fn wrong_method(request: &Request<'_>, allowed: &str) -> RtResponse {
    rendered(
        405,
        &json!({
            "error": format!(
                "Incorrect HTTP method for uri [{}] and method [{}], allowed: [{allowed}]",
                request.path, request.method
            )
        }),
    )
}

/// The engine's error envelope: the type and the reason, with the status
/// repeated in the body as every one of its errors carries it.
fn error(status: u16, error_type: &str, reason: &str) -> RtResponse {
    rendered(
        status,
        &json!({
            "error": {
                "root_cause": [{ "type": error_type, "reason": reason }],
                "type": error_type,
                "reason": reason,
            },
            "status": status,
        }),
    )
}

/// A 200 answer carrying `body`.
fn ok(body: &Value) -> RtResponse {
    rendered(200, body)
}

/// A status with no body at all — what a `HEAD` answers.
fn status_only(status: u16) -> RtResponse {
    RtResponse::new(
        StatusCode::try_from(status).unwrap_or_else(|_| fallback_status()),
        SdkBody::empty(),
    )
}

/// The status a status this emulator could not build falls back to.
fn fallback_status() -> StatusCode {
    StatusCode::try_from(500).expect("500 is a status")
}

/// One rendered JSON answer, as the wire carries it.
fn rendered(status: u16, body: &Value) -> RtResponse {
    let payload = ::serde_json::to_vec(body).unwrap_or_default();
    let mut response = RtResponse::new(
        StatusCode::try_from(status).unwrap_or_else(|_| fallback_status()),
        SdkBody::from(payload),
    );
    if let Err(refused) = response
        .headers_mut()
        .try_insert("content-type", "application/json")
    {
        ::tracing::warn!(
            error = %refused,
            "the data plane's content type cannot be sent on the wire and is left off"
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row: the path, and the domain it names with the request path within
    /// that domain's endpoint — `None` for a path that names this plane not at
    /// all.
    struct AddressingRow {
        case: &'static str,
        path: &'static str,
        names: Option<(&'static str, &'static str)>,
    }

    /// RULE: the data plane is addressed under the reserved path namespace naming
    /// this plane and the domain; nothing else names it.
    #[test]
    fn a_prefixed_path_names_this_plane_the_domain_and_the_request_path() {
        let cases: &[AddressingRow] = &[
            AddressingRow {
                case: "the plane, a domain and a document path",
                path: "/_aws/es-2015-01-01/probe/index/_doc/1",
                names: Some(("probe", "/index/_doc/1")),
            },
            AddressingRow {
                case: "the plane and a domain alone",
                path: "/_aws/es-2015-01-01/probe",
                names: Some(("probe", "/")),
            },
            AddressingRow {
                case: "another plane of the same emulator",
                path: "/_aws/es/probe/index/_doc/1",
                names: None,
            },
            AddressingRow {
                case: "this model's own control plane",
                path: "/2015-01-01/es/domain/probe",
                names: None,
            },
            AddressingRow {
                case: "no namespace at all",
                path: "/probe/index/_doc/1",
                names: None,
            },
            AddressingRow {
                case: "the plane naming no domain",
                path: "/_aws/es-2015-01-01/",
                names: None,
            },
        ];
        for row in cases {
            let case = row.case;
            let named = prefixed_addressing(row.path);
            assert_eq!(
                named, row.names,
                "case {case:?}: expected {:?}, got {named:?}",
                row.names
            );
        }
    }

    /// RULE: a first path segment beginning with the engine's reserved prefix
    /// names an API of the engine and never an index — except the one reserved
    /// name that selects every index.
    #[test]
    fn a_reserved_segment_names_an_api_and_not_an_index() {
        let cases: &[(&str, bool)] = &[
            ("_search", true),
            ("_bulk", true),
            ("_cluster", true),
            ("_all", false),
            ("index", false),
            ("my-index-000001", false),
        ];
        for (segment, expected) in cases {
            let reserved = reserved(segment);
            assert_eq!(
                reserved, *expected,
                "case {segment:?}: expected reserved {expected}, got {reserved}"
            );
        }
    }
}
