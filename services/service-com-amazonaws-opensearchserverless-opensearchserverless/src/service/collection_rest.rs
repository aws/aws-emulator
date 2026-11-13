// SPDX-License-Identifier: CC0-1.0

//! The COLLECTION's own data plane: how a request a client addresses to the endpoint a
//! collection of this service published is recognised, and how it is answered out of the
//! indexes and documents that collection holds.
//!
//! A collection this service creates publishes the host a client sends index, search and
//! data-upload requests to — `CollectionDetail$collectionEndpoint`, the
//! "collection-specific endpoint used to submit index, search, and data upload requests"
//! — and the protocol spoken there is the OpenSearch REST API, which this service's model
//! does not describe: it models the control plane alone. That endpoint host resolved to
//! this service's SigV4 signing name, so every REST request to it reached the `awsJson1_0`
//! front door, which routes on an `x-amz-target` header no REST verb carries, and was
//! answered `400 SerializationException` — so no index could be created in a collection
//! and nothing could be written to or read from one.
//!
//! So this module is the second half of [`crate::service::EmulatedService`]: the claim
//! ([`claims_parts`], read by `Service::can_handle`) and the serving ([`serve`], reached
//! from `Service::override_operation` for a request carrying a SigV4 credential — so the
//! router verifies that credential before anything is served — and from
//! `Service::override_routing_error` for one carrying none). It is also what the
//! collection's OWN published address answers with ([`serve_published`], the one decision
//! behind the listener [`crate::state::collection_plane`] opens): every addressing goes
//! through [`serve_collection`], so none of them can answer a different thing.
//!
//! # What a request addresses
//!
//! What a collection PUBLISHES is a listener of its own
//! ([`crate::state::collection_plane`]), where the address names the collection and the
//! whole path is the client's — the addressing a stock OpenSearch client needs, since it is
//! configured with a host and a port and dials the REST API at the root of them. A request
//! that reaches the emulator's own FRONT DOOR instead carries what it was told, and two
//! addressings resolve there:
//!
//! * HOST-STYLE: the request's host IS a host some collection of this store publishes — its
//!   collection endpoint, the FIPS endpoint beside it, or either Dashboards host (see
//!   [`crate::state::CollectionRecord::publishes_host`]) — and the whole path is the
//!   request path;
//! * PREFIXED PATH-STYLE: the data plane is named by the reserved namespace segment
//!   (`service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE`) and one of
//!   [`DATA_PLANE_SEGMENTS`], then the collection's id, its name or its endpoint host, then
//!   the request path — `/_aws/aoss/{collection-or-host}/{path}`. That segment begins with
//!   `_`, which no modelled path segment of any service does, so the prefix collides with
//!   no control-plane route. It is also what a collection reports while no listener of its
//!   own is open (`crate::state::announced_endpoints`).
//!
//! Either way the claim is narrowed to a collection this service HOLDS, read from the one
//! shared store, and a request carrying an `x-amz-target` header is never claimed: that
//! header is how this service's own RPC front door names an operation, so a control-plane
//! call addressed to a collection's host stays a control-plane call.
//!
//! # What it answers, and what decides it
//!
//! Every status, header and byte is computed from what a create or a write persisted: an
//! index's settings and mappings from the index record the control plane's `CreateIndex`
//! and this plane's `PUT /{index}` both write, a document read from the document that
//! write stored, a search and a count from the documents the index holds. A read writes
//! NOTHING; the writes are the requests that are writes.
//!
//! The REST surface served is the one this emulator can answer out of persisted state: the
//! cluster banner, an index's create/read/head/delete, its mappings and settings, a
//! document's write/read/delete, the bulk write, a refresh, a count and a search over a
//! bounded query vocabulary ([`search`]) including the k-nearest-neighbour query a vector
//! collection exists for. A path outside it is answered the way the engine answers a path
//! no handler is registered for, rather than with an invented body.

use std::time::Instant;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use serde_json::{json, Value};
use smithy_server_runtime::http;

use ::service_common::data_plane_addressing::{
    addressed_host, first_segment, host_only, EMULATOR_PATH_NAMESPACE,
};

use crate::state::{self, CollectionRecord};

mod authorization;
mod documents;
mod indexes;
mod search;

/// The segments that name THIS plane under
/// [`EMULATOR_PATH_NAMESPACE`](service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE),
/// which are the two host labels a collection's published endpoints carry — this service's
/// own name and its FIPS label: `/_aws/aoss/{collection}/{path}` and
/// `/_aws/aoss-fips/{collection}/{path}`.
///
/// Both resolve to the same collection, because the FIPS endpoint of a collection is the
/// same collection reached at its other published address
/// (`crate::state::announced_endpoints`).
const DATA_PLANE_SEGMENTS: &[&str] = &[state::ARN_NAMESPACE, state::FIPS_ARN_NAMESPACE];

/// The most request-body bytes this plane reads.
///
/// EMULATOR RESOURCE GUARD, not an AWS limit: a body is held in memory while it is parsed
/// and a document it carries is stored, and its size is the caller's to choose. 1 MiB is
/// past any index mapping or document this surface is asked to hold; a body past it is
/// refused with the status the engine refuses an oversized request with, before anything
/// is parsed.
const MAX_REQUEST_BODY_BYTES: usize = 1024 * 1024;

/// The scheme an AWS API call's credential is presented under.
const SIGV4_SCHEME: &str = "AWS4-HMAC-SHA256";

/// The header this service's own RPC protocol names an operation in, whose presence means
/// a request is a CONTROL-plane call however it is addressed.
const RPC_TARGET_HEADER: &str = "x-amz-target";

/// The engine error type for a create of an index the cluster already holds.
const RESOURCE_ALREADY_EXISTS_EXCEPTION: &str = "resource_already_exists_exception";

/// The engine error type for a request naming an index the cluster does not hold.
const INDEX_NOT_FOUND_EXCEPTION: &str = "index_not_found_exception";

/// The engine error type for a request whose argument the cluster cannot act on.
const ILLEGAL_ARGUMENT_EXCEPTION: &str = "illegal_argument_exception";

/// The engine error type for a body that is not the JSON the request takes.
const PARSE_EXCEPTION: &str = "parse_exception";

/// The engine error type for a query body the search parser cannot read.
const PARSING_EXCEPTION: &str = "parsing_exception";

/// The engine error type for a write that may not replace the document it addresses.
const VERSION_CONFLICT_ENGINE_EXCEPTION: &str = "version_conflict_engine_exception";

/// The engine error type for a cluster that is not serving.
const CLUSTER_BLOCK_EXCEPTION: &str = "cluster_block_exception";

/// The engine error type for an index name the cluster will not take.
const INVALID_INDEX_NAME_EXCEPTION: &str = "invalid_index_name_exception";

/// The OpenSearch distribution this surface answers as, which is the engine an OpenSearch
/// Serverless collection runs.
const ENGINE_DISTRIBUTION: &str = "opensearch";

/// The engine version this plane implements the REST surface of. A collection is
/// serverless, so no caller chooses it and nothing in the control plane reports one: it is
/// stated once here, and is what `GET /` announces.
const ENGINE_VERSION: &str = "2.13.0";

/// The tagline every OpenSearch cluster banner carries.
const TAGLINE: &str = "The OpenSearch Project: https://opensearch.org/";

/// What a request ADDRESSES, as the wire carries it: the host it was sent to — absent on a
/// request that names neither a `Host` header nor an absolute URI — its path, its query
/// string, and whether it names an RPC target.
///
/// Built from BOTH shapes the router hands this module, the buffered parts the claim reads
/// and the runtime-API request the serving seam reads, so one rule resolves both and the
/// claim cannot resolve a different collection than the answer does.
#[derive(Debug)]
struct Addressed {
    host: Option<String>,
    path: String,
    query: String,
    /// Whether the request carries [`RPC_TARGET_HEADER`], which makes it a control-plane
    /// call this plane claims nothing for.
    rpc_target: bool,
}

impl Addressed {
    /// The addressing of the buffered request these parts came from.
    fn of_parts(parts: &http::request::Parts) -> Self {
        let uri = parts.uri.to_string();
        let header = parts
            .headers
            .get("host")
            .and_then(|host| host.to_str().ok());
        Self {
            host: addressed_host(&uri, header),
            path: parts.uri.path().to_string(),
            query: parts.uri.query().unwrap_or_default().to_string(),
            rpc_target: parts.headers.contains_key(RPC_TARGET_HEADER),
        }
    }

    /// The addressing of a runtime-API request, whose URI arrives as a string and is
    /// parsed by the `http` crate rather than split by hand.
    fn of_request(req: &RtRequest) -> Self {
        let uri = parsed_uri(req.uri());
        Self {
            host: addressed_host(req.uri(), req.headers().get("host")),
            path: uri.path().to_string(),
            query: uri.query().unwrap_or_default().to_string(),
            rpc_target: req.headers().get(RPC_TARGET_HEADER).is_some(),
        }
    }
}

/// What a request resolves to: the collection it addresses, by the id that is a
/// collection's identity in this API, and the path WITHIN that collection's own endpoint —
/// which is what the REST surface is read against.
#[derive(Debug)]
struct Resolved {
    collection_id: String,
    request_path: String,
}

/// The request URI of a runtime-API request, parsed by the `http` crate. A URI this parse
/// rejects is one the server would not have accepted, and it resolves to `http::Uri`'s own
/// default (`/`), which names no collection and claims nothing.
fn parsed_uri(uri: &str) -> http::Uri {
    uri.parse::<http::Uri>().unwrap_or_default()
}

/// Whether this data plane serves the request these `parts` came from — read by
/// `Service::can_handle`, so a request addressed to a collection this service holds
/// reaches this service instead of the front door's `UnknownService`.
pub fn claims_parts<Eng: Send + Sync + 'static>(
    engine: &Eng,
    parts: &http::request::Parts,
) -> bool {
    resolve(engine, &Addressed::of_parts(parts)).is_some()
}

/// [`claims_parts`] for the runtime-API request the serving seams are handed, resolved by
/// the same rule.
pub fn claims_request<Eng: Send + Sync + 'static>(engine: &Eng, req: &RtRequest) -> bool {
    resolve(engine, &Addressed::of_request(req)).is_some()
}

/// Whether `req` presents a SigV4 credential, which is what decides whether the router
/// verifies a signature before this plane serves it.
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

/// Which collection `addressed` addresses, and the path within its endpoint, or `None`
/// when this data plane serves the request not at all.
fn resolve<Eng: Send + Sync + 'static>(engine: &Eng, addressed: &Addressed) -> Option<Resolved> {
    // An RPC target names a modelled operation of the control plane, whatever host the
    // request reached: this plane claims none of those.
    if addressed.rpc_target {
        return None;
    }
    // Resolved WITHOUT the store first, so a request this plane cannot be addressed by —
    // which is every control-plane request and every request the router offers this
    // service for another reason — never takes the shared lock: the addressing is either
    // the prefix that names this plane, or a host shaped like one a collection publishes.
    let prefixed = prefixed_addressing(&addressed.path);
    if prefixed.is_none() {
        // A path under the reserved namespace that did NOT name this plane names another
        // one, which serves its own requests.
        if first_segment(&addressed.path)
            .is_some_and(|(first, _)| first == EMULATOR_PATH_NAMESPACE)
        {
            return None;
        }
        if !may_address_a_collection(addressed.host.as_deref()?) {
            return None;
        }
    }
    let state = state::service_state(engine)?;
    let store = state.lock();

    // PREFIXED PATH-STYLE, the one addressing that is explicit about naming this plane,
    // and so the one read whatever the request's host is.
    if let Some((named, path)) = prefixed {
        return Some(Resolved {
            collection_id: addressed_collection(&store, named)?,
            request_path: path.to_string(),
        });
    }
    // HOST-STYLE: the host IS a host some collection of this store publishes. Compared
    // BOTH as the client addressed it and without its port, because a published address
    // may carry a port of its own (a collection's own listener,
    // `crate::state::collection_plane`) or stand for the default one (AWS's host).
    let addressed_host = addressed.host.as_deref()?;
    let collection = store.collections.values().find(|collection| {
        collection.publishes_host(addressed_host)
            || collection.publishes_host(host_only(addressed_host))
    })?;
    Some(Resolved {
        collection_id: collection.id.clone(),
        request_path: addressed.path.clone(),
    })
}

/// Whether `host` CAN be a host a collection publishes, read without the store so a
/// request that cannot be one is dropped before the shared lock is taken.
///
/// Two shapes pass, which are the two a published address takes:
///
/// * the endpoint template AWS's own hosts follow (`state::collection_endpoint` /
///   `state::fips_collection_endpoint`: `<id>.<region>.aoss[-fips].<dns suffix>`), which
///   this service's own API endpoint — carrying no label before the service name — does
///   not;
/// * a host that names no service at all (an address or a loopback name), which is what a
///   collection's own listener and this emulator's own endpoint are addressed by. Which
///   collection such a host names is still the store's to say.
fn may_address_a_collection(host: &str) -> bool {
    addresses_a_collection_host(host)
        || !::service_common::data_plane_addressing::names_a_service(host)
}

/// Whether `host` is SHAPED like a host a collection publishes, read without the store so
/// a request that cannot be one is dropped before the shared lock is taken.
///
/// The shape is the endpoint template the control plane builds those hosts from
/// (`state::collection_endpoint` / `state::fips_collection_endpoint`:
/// `<id>.<region>.aoss[-fips].<dns suffix>`), so every published host passes it and this
/// service's own API endpoint — which carries no label before the service name — does not.
fn addresses_a_collection_host(host: &str) -> bool {
    let host = host_only(host);
    [state::ARN_NAMESPACE, state::FIPS_ARN_NAMESPACE]
        .into_iter()
        .any(|label| {
            host.strip_suffix(&format!(".{label}.{}", state::DNS_SUFFIX))
                .is_some_and(|before| before.contains('.'))
        })
}

/// The collection a prefixed addressing NAMES: its `CollectionId`, its `CollectionName`,
/// or a host it publishes — the three values a client has been handed for a collection.
fn addressed_collection(
    store: &state::ComAmazonawsOpensearchserverlessOpensearchserverlessState,
    named: &str,
) -> Option<String> {
    if store.collections.contains_key(named) {
        return Some(named.to_string());
    }
    if let Some(id) = store.collection_name_index.get(named) {
        return Some(id.clone());
    }
    store
        .collections
        .values()
        .find(|collection| collection.publishes_host(named))
        .map(|collection| collection.id.clone())
}

/// The collection a prefixed path names and the request path that follows it, or `None`
/// when the path is not of that shape.
fn prefixed_addressing(path: &str) -> Option<(&str, &str)> {
    let (namespace, rest) = first_segment(path)?;
    if namespace != EMULATOR_PATH_NAMESPACE {
        return None;
    }
    let (plane, rest) = first_segment(rest)?;
    if !DATA_PLANE_SEGMENTS.contains(&plane) {
        return None;
    }
    let (named, request_path) = first_segment(rest)?;
    (!named.is_empty()).then_some((named, request_path))
}

/// Answer a request this plane claimed at the FRONT DOOR, out of the records the control
/// plane holds — the host-style and prefixed path-style addressings
/// [`resolve`] reads.
pub fn serve<Eng: Send + Sync + 'static>(engine: &Eng, req: &RtRequest) -> RtResponse {
    let received = Instant::now();
    let addressed = Addressed::of_request(req);
    let Some(resolved) = resolve(engine, &addressed) else {
        // The collection was purged by another request between the claim and here, so the
        // host it published answers nothing.
        return unavailable();
    };
    let Some(state) = state::service_state(engine) else {
        return unavailable();
    };
    // Who the request speaks for, resolved against the account's identity records BEFORE
    // this service's own store is locked, so no two service stores are ever held at once.
    let caller = authorization::caller(engine, req.headers().get("authorization"));
    // The method of a runtime-API request arrives as a string; the router hands this seam a
    // request whose body it has already buffered (it reads the body itself to resolve a
    // routing key), so an absent one is an empty body rather than a stream this plane
    // declined to read.
    let method = req.method().to_ascii_uppercase();
    let request = Request {
        method: &method,
        path: &resolved.request_path,
        query: &addressed.query,
        body: req.body().bytes().unwrap_or_default(),
        received,
    };
    serve_collection(&state, &resolved.collection_id, &request, &caller)
}

/// Answer a request that arrived on the LISTENER one collection is published on
/// ([`crate::state::collection_plane`]), where the address names the collection and the
/// whole path is the request path.
///
/// The same function serves it as serves the front door's path-style addressing
/// ([`serve_collection`]), so the two addressings of one collection cannot diverge in what
/// they answer.
pub fn serve_published<Eng: Send + Sync + 'static>(
    engine: &Eng,
    state: &::engine::ServiceState<
        state::ComAmazonawsOpensearchserverlessOpensearchserverlessState,
    >,
    collection_id: &str,
    req: &::http::Request<Vec<u8>>,
) -> ::http::Response<Vec<u8>> {
    let received = Instant::now();
    // As at the front door: resolved BEFORE this service's store is locked.
    let caller = authorization::caller(
        engine,
        req.headers()
            .get("authorization")
            .and_then(|value| value.to_str().ok()),
    );
    let method = req.method().as_str().to_ascii_uppercase();
    let request = Request {
        method: &method,
        path: req.uri().path(),
        query: req.uri().query().unwrap_or_default(),
        body: req.body(),
        received,
    };
    as_http(serve_collection(state, collection_id, &request, &caller))
}

/// Answer one request out of the collection `collection_id` names — the one reading of this
/// surface, reached from the front door ([`serve`]) and from the collection's own published
/// listener ([`serve_published`]) alike.
///
/// The whole answer is computed inside ONE short critical section. What a request can ask
/// for is bounded BEFORE any scan begins — the body by [`MAX_REQUEST_BODY_BYTES`], a bulk
/// body's actions by [`documents::MAX_BULK_ACTIONS`], and a search's work (the product of
/// the documents scanned and the cost of the query) by [`search::MAX_SEARCH_WORK`] — and
/// nothing inside it awaits, sleeps or copies a document per clause, so the store is held
/// for a bounded number of scalar comparisons rather than for work a caller chose.
fn serve_collection(
    state: &::engine::ServiceState<
        state::ComAmazonawsOpensearchserverlessOpensearchserverlessState,
    >,
    collection_id: &str,
    request: &Request<'_>,
    caller: &authorization::Caller,
) -> RtResponse {
    if request.body.len() > MAX_REQUEST_BODY_BYTES {
        return error(
            413,
            ILLEGAL_ARGUMENT_EXCEPTION,
            &format!(
                "entity content is too long [{}] for the configured buffer limit \
                 [{MAX_REQUEST_BODY_BYTES}]",
                request.body.len()
            ),
        );
    }
    let mut store = state.lock();
    // The read materialises the transitions the clock has already made due, which this
    // request neither starts nor changes: a collection whose create has settled is ACTIVE
    // here, and one whose accepted deletion has been purged is gone.
    store.settle_due_transitions();
    let Some(collection) = store.collections.get(collection_id) else {
        return unavailable();
    };
    // A collection serves its data plane once it is "ready to use"
    // (`CollectionStatus$ACTIVE`'s own documentation): one still being created, updated or
    // deleted has no cluster behind its endpoint yet.
    if collection.status != state::COLLECTION_ACTIVE {
        return unavailable();
    }
    // What the collection's OWN policies say about this request: whether its endpoint is
    // reachable at all, and whether the principal it is signed by may perform what it asks
    // ([`authorization`]). Decided before anything is read or written, and from the
    // policies as they read at this instant.
    let collection_name = collection.name.clone();
    if let Some(refused) = authorization::refusal(&store, &collection_name, caller, request) {
        return refused;
    }
    let Some(collection) = store.collections.get_mut(collection_id) else {
        return unavailable();
    };
    serve_resolved(collection, request)
}

/// One answer of this surface as a plain HTTP response, which is what a core HTTP listener
/// writes — the same status, content type and bytes the front door sends.
fn as_http(response: RtResponse) -> ::http::Response<Vec<u8>> {
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .map(ToString::to_string);
    let body = response.body().bytes().unwrap_or_default().to_vec();
    let mut builder = ::http::Response::builder().status(status);
    if let Some(content_type) = content_type {
        builder = builder.header("content-type", content_type);
    }
    builder.body(body).unwrap_or_else(|_| {
        // Every part came off a response this module built, so the builder cannot fail;
        // answer a bare 500 rather than panicking if it ever does.
        let mut fallback = ::http::Response::new(Vec::new());
        *fallback.status_mut() = ::http::StatusCode::INTERNAL_SERVER_ERROR;
        fallback
    })
}

/// Answer `request` out of the `collection` it addresses.
///
/// A path segment where an INDEX belongs is held to what an `IndexName` may be — it cannot
/// begin with `_` ([`state::validate_index_name`]) — so a reserved endpoint this surface
/// does not serve (`/_nodes`, `/_cat/indices`) is answered as a path the engine has no
/// handler for rather than read as an index of that name. The two names the engine reads as
/// every index ([`search::EVERY_INDEX`]) are the exception, and only where a read is over
/// indexes.
fn serve_resolved(collection: &mut CollectionRecord, request: &Request<'_>) -> RtResponse {
    /// Whether `segment` addresses an INDEX rather than a reserved endpoint of the
    /// engine's own: an `IndexName` can never begin with `_`
    /// ([`state::validate_index_name`]), so a segment that does is `/_nodes`, `/_cat` or
    /// another endpoint this surface serves nothing for — never an index of that name.
    /// A name outside the rule in every OTHER way (an uppercase letter, a leading hyphen)
    /// still addresses an index, and is refused as the invalid index name it is.
    fn addresses_an_index(segment: &str) -> bool {
        !segment.starts_with('_')
    }

    /// Whether `segment` names the indexes a READ is over: one index, or one of the two
    /// names the engine reads as every index. A WRITE is addressed to one named index, so
    /// it takes [`addresses_an_index`] alone.
    fn addresses_indexes(segment: &str) -> bool {
        addresses_an_index(segment) || search::EVERY_INDEX.contains(&segment)
    }

    let segments = request.segments();
    match segments.as_slice() {
        [] => match request.method {
            "GET" => ok(&banner(collection)),
            "HEAD" => status_only(200),
            _ => wrong_method(request, "GET, HEAD"),
        },
        ["_bulk"] => documents::bulk(collection, request, None),
        ["_refresh"] => search::refresh(collection, request, None),
        ["_search"] => search::search(collection, request, None),
        ["_count"] => search::count(collection, request, None),
        [index] if addresses_an_index(index) => {
            indexes::index_request(collection, request, index)
        }
        [index, "_doc"] if addresses_an_index(index) => {
            documents::write(collection, request, index, None, Creates::Upsert)
        }
        [index, "_doc", id] if addresses_an_index(index) => {
            documents::document_request(collection, request, index, id)
        }
        [index, "_create", id] if addresses_an_index(index) => {
            match request.method {
                "PUT" | "POST" => {
                    documents::write(collection, request, index, Some(id), Creates::Only)
                }
                _ => wrong_method(request, "POST, PUT"),
            }
        }
        [index, "_mapping" | "_mappings"] if addresses_an_index(index) => {
            indexes::mapping_request(collection, request, index)
        }
        [index, "_settings"] if addresses_an_index(index) => {
            indexes::settings_request(collection, request, index)
        }
        [index, "_count"] if addresses_indexes(index) => {
            search::count(collection, request, Some(index))
        }
        [index, "_search"] if addresses_indexes(index) => {
            search::search(collection, request, Some(index))
        }
        [index, "_refresh"] if addresses_indexes(index) => {
            search::refresh(collection, request, Some(index))
        }
        [index, "_bulk"] if addresses_an_index(index) => {
            documents::bulk(collection, request, Some(index))
        }
        _ => no_handler(request),
    }
}

/// Whether a write may REPLACE a document the index already holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Creates {
    /// `_doc` without an `op_type`: a write to an id the index holds replaces it.
    Upsert,
    /// `_create`, and `_doc` with `op_type=create`: a write to an id the index already
    /// holds is REFUSED rather than replacing the document.
    Only,
}

/// One data-plane request, as the surface below reads it.
struct Request<'a> {
    method: &'a str,
    path: &'a str,
    query: &'a str,
    body: &'a [u8],
    /// When the request reached this plane, which is what `took` is measured from.
    received: Instant,
}

impl Request<'_> {
    /// The path's segments, with empty ones dropped so a trailing or doubled `/` names no
    /// segment of its own.
    fn segments(&self) -> Vec<&str> {
        self.path
            .split('/')
            .filter(|part| !part.is_empty())
            .collect()
    }

    /// The value the query string gives `name`, or `None`.
    ///
    /// The parameters this surface reads are single tokens — a count, a document version,
    /// an `op_type`, a refresh mode — so what the caller wrote is what the escapes would
    /// stand for and no decoding is needed to read them.
    fn parameter(&self, name: &str) -> Option<&str> {
        self.query.split('&').find_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            (key == name).then_some(value)
        })
    }

    /// The request body as the JSON value the handler expects, `None` for an empty body.
    ///
    /// The refusal is BOXED, as every fallible step of this module's reading is: a
    /// rendered response is far larger than what these functions answer with, and a
    /// `Result` whose error dominates its size is paid for on the success path too.
    fn json(&self) -> Result<Option<Value>, Box<RtResponse>> {
        if self.body.iter().all(u8::is_ascii_whitespace) {
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

    /// How long this plane has been answering the request, as `took` reports it.
    fn took(&self) -> u64 {
        u64::try_from(self.received.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

/// The cluster banner a `GET /` answers: what this cluster IS, from the collection's own
/// record.
fn banner(collection: &CollectionRecord) -> Value {
    json!({
        "name": collection.id,
        "cluster_name": collection.name,
        "cluster_uuid": stable_uuid(&collection.arn),
        "version": {
            "distribution": ENGINE_DISTRIBUTION,
            "number": ENGINE_VERSION,
        },
        "tagline": TAGLINE,
    })
}

/// The `uuid` an index of `collection` carries: a stable function of the collection's own
/// ARN and the index name, so every read of one index reports one uuid, two indexes never
/// share one, and nothing has to be remembered for it.
fn index_uuid(collection: &CollectionRecord, index_name: &str) -> String {
    stable_uuid(&format!("{}/index/{index_name}", collection.arn))
}

/// A hyphenated UUID derived from `seed`: a version-5 (name-based) UUID in the OID
/// namespace, so the id of a thing is a function of the thing — stable across reads,
/// distinct per seed, and written in the form a UUID is written in, without anything
/// having to be remembered for it.
fn stable_uuid(seed: &str) -> String {
    ::uuid::Uuid::new_v5(&::uuid::Uuid::NAMESPACE_OID, seed.as_bytes()).to_string()
}

/// The `_shards` summary a write, a refresh, a count and a search carry.
///
/// A collection is serverless, so no caller chooses a shard count and the control plane
/// reports none: what the engine reports is that the write or the read reached the one
/// primary it addressed and nothing failed.
fn shards() -> Value {
    json!({ "total": 1, "successful": 1, "failed": 0 })
}

/// The JSON value a stored document is — the shape every answer renders and every query
/// is evaluated over.
///
/// A stored `Document` is within [`state::MAX_DOCUMENT_DEPTH`], which is what bounds this
/// walk's recursion: nothing deeper than that was ever stored
/// ([`state::document_from_json`] is the only way one is built).
fn json_from_document(document: &::aws_smithy_types::Document) -> Value {
    use ::aws_smithy_types::{Document, Number};

    match document {
        Document::Null => Value::Null,
        Document::Bool(held) => Value::Bool(*held),
        Document::Number(Number::PosInt(held)) => json!(held),
        Document::Number(Number::NegInt(held)) => json!(held),
        Document::Number(Number::Float(held)) => ::serde_json::Number::from_f64(*held)
            .map_or(Value::Null, Value::Number),
        Document::String(held) => Value::String(held.clone()),
        Document::Array(items) => Value::Array(items.iter().map(json_from_document).collect()),
        Document::Object(members) => Value::Object(
            members
                .iter()
                .map(|(name, member)| (name.clone(), json_from_document(member)))
                .collect(),
        ),
    }
}

/// The stored `Document` a caller-supplied JSON value is, refused when it is past the
/// bounds this emulator stores one under ([`state::MAX_DOCUMENT_DEPTH`] and `max_nodes`).
fn document_within(value: &Value, max_nodes: usize, what: &str) -> Result<::aws_smithy_types::Document, Box<RtResponse>> {
    let refused = || {
        Box::new(error(
            400,
            ILLEGAL_ARGUMENT_EXCEPTION,
            &format!(
                "{what} must not nest deeper than {} levels or carry more than {max_nodes} values",
                state::MAX_DOCUMENT_DEPTH
            ),
        ))
    };
    let document = state::document_from_json(value).ok_or_else(refused)?;
    if !state::document_within_bounds(&document, max_nodes) {
        return Err(refused());
    }
    Ok(document)
}

/// The answer for a collection whose endpoint is published but whose cluster is not
/// serving: it is still being created, it is being deleted, or it has been purged.
fn unavailable() -> RtResponse {
    error(
        503,
        CLUSTER_BLOCK_EXCEPTION,
        "blocked by: [SERVICE_UNAVAILABLE]",
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

/// The refusal the engine answers a path it DOES serve, reached with a method that path
/// does not take, worded as the engine words it: naming the methods that are allowed.
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

/// The refusal for an index the collection does not hold.
fn index_not_found(index_name: &str) -> RtResponse {
    indexed_error(
        404,
        INDEX_NOT_FOUND_EXCEPTION,
        &format!("no such index [{index_name}]"),
        index_name,
        None,
    )
}

/// An error naming the index it is about, which is what the engine's index errors carry
/// beside the type and the reason — with the index's own uuid where the error is about an
/// index that exists.
fn indexed_error(
    status: u16,
    error_type: &str,
    reason: &str,
    index_name: &str,
    index_uuid: Option<&str>,
) -> RtResponse {
    let mut cause = json!({
        "type": error_type,
        "reason": reason,
        "index": index_name,
        "resource.type": "index_or_alias",
        "resource.id": index_name,
    });
    if let (Some(uuid), Value::Object(members)) = (index_uuid, &mut cause) {
        members.insert("index_uuid".to_string(), Value::String(uuid.to_string()));
    }
    rendered(
        status,
        &json!({
            "error": {
                "root_cause": [cause.clone()],
                "type": error_type,
                "reason": reason,
                "index": index_name,
                "index_uuid": index_uuid,
                "resource.type": "index_or_alias",
                "resource.id": index_name,
            },
            "status": status,
        }),
    )
}

/// The engine's error envelope: the type and the reason, with the status repeated in the
/// body as every OpenSearch error carries it.
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
    StatusCode::try_from(500).unwrap_or_else(|_| StatusCode::from(::http::StatusCode::INTERNAL_SERVER_ERROR))
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

    /// RULE: the data plane is addressed either at a host the collection publishes or
    /// under the reserved path namespace naming this plane and the collection; nothing
    /// else names it.
    #[test]
    fn a_prefixed_path_names_this_plane_the_collection_and_the_request_path() {
        /// One case: the path, and the collection and request path it names.
        struct Row {
            case: &'static str,
            path: &'static str,
            names: Option<(&'static str, &'static str)>,
        }
        let rows: &[Row] = &[
            Row {
                case: "a collection id and an index path",
                path: "/_aws/aoss/abc123/my-index",
                names: Some(("abc123", "/my-index")),
            },
            Row {
                case: "a collection id and the root path",
                path: "/_aws/aoss/abc123",
                names: Some(("abc123", "/")),
            },
            Row {
                case: "a collection id and a search path",
                path: "/_aws/aoss/abc123/my-index/_search",
                names: Some(("abc123", "/my-index/_search")),
            },
            Row {
                case: "the FIPS plane segment and an index path",
                path: "/_aws/aoss-fips/abc123/my-index",
                names: Some(("abc123", "/my-index")),
            },
            Row {
                case: "another plane under the same namespace",
                path: "/_aws/es/search-target/_cluster/health",
                names: None,
            },
            Row {
                case: "no namespace at all",
                path: "/my-index",
                names: None,
            },
            Row {
                case: "the namespace with no plane",
                path: "/_aws",
                names: None,
            },
            Row {
                case: "the plane with no collection",
                path: "/_aws/aoss",
                names: None,
            },
        ];
        for Row { case, path, names } in rows {
            let named = prefixed_addressing(path);
            assert_eq!(
                named, *names,
                "case {case}: expected {names:?}, got {named:?}"
            );
        }
    }

    /// RULE: a derived uuid is a function of the thing it names — stable across reads,
    /// distinct per thing, and written in the 8-4-4-4-12 grouping.
    #[test]
    fn a_derived_uuid_is_stable_distinct_and_well_formed() {
        let one = stable_uuid("arn:aws:aoss:us-east-1:000000000000:collection/abc123");
        let again = stable_uuid("arn:aws:aoss:us-east-1:000000000000:collection/abc123");
        let other = stable_uuid("arn:aws:aoss:us-east-1:000000000000:collection/def456");
        assert_eq!(one, again, "case read twice: expected {one:?}, got {again:?}");
        assert_ne!(
            one, other,
            "case two collections: expected different uuids, got {one:?} twice"
        );
        let widths: Vec<usize> = one.split('-').map(str::len).collect();
        assert_eq!(
            widths,
            vec![8, 4, 4, 4, 12],
            "case the grouping of {one:?}: expected 8-4-4-4-12, got {widths:?}"
        );
    }

    /// RULE: a stored document reads back as the JSON it was written as, whatever JSON
    /// shape it carries.
    #[test]
    fn a_stored_document_reads_back_as_the_json_it_was_written_as() {
        let rows: &[(&str, Value)] = &[
            ("an object of scalars", json!({ "a": 1, "b": "two", "c": true })),
            ("a nested object", json!({ "a": { "b": { "c": [1, 2, 3] } } })),
            ("a vector", json!({ "v": [0.5, -1.5, 2.0] })),
            ("an empty object", json!({})),
            ("a null member", json!({ "a": Value::Null })),
            ("a list of objects", json!([{ "a": 1 }, { "b": 2 }])),
        ];
        for (case, value) in rows {
            let stored = state::document_from_json(value).expect("the value is within bounds");
            let read = json_from_document(&stored);
            assert_eq!(
                &read, value,
                "case {case}: expected {value}, got {read}"
            );
        }
    }
}
