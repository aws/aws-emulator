// SPDX-License-Identifier: CC0-1.0

//! The addresses a collection PUBLISHES, and that every one of them is an address this
//! emulator answers the collection's OpenSearch REST surface on.
//!
//! `CollectionDetail$collectionEndpoint` is the "Collection-specific endpoint used to
//! submit index, search, and data upload requests to an OpenSearch Serverless collection",
//! and `dashboardEndpoint` / `fipsEndpoints` are the same address in its other forms: none
//! of them is a label a caller reads back, each is an address a client is CONFIGURED with
//! and then DIALS at its root — which is what AWS's own documented
//! `OpenSearch(hosts=[{'host': <collectionEndpoint host>, 'port': 443}], …)` client does.
//!
//! So the rule under test is that every published endpoint names an address THIS emulator
//! answers on — the collection's own listener, opened through the core — rather than AWS's
//! `<id>.<region>.aoss.amazonaws.com` host, which resolves nowhere on the machine the
//! emulator runs on. Each case asserts it three ways: on the value `BatchGetCollection`
//! reports, by OPENING a connection to the address, and by driving the OpenSearch REST
//! surface the listener serves there (`state::collection_plane` hands every request to
//! `service::collection_rest::serve_published`, which is the one decision behind it).
//!
//! The table spans what the rule generalises over: the collection type, which address the
//! creating client reached this emulator at, and whether this process is served on a port
//! at all — none of which may change that the published address is one this emulator
//! answers on.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_opensearchserverless_opensearchserverless::service::collection_rest::serve_published;
use service_com_amazonaws_opensearchserverless_opensearchserverless::state::service_state;
use service_com_amazonaws_opensearchserverless_opensearchserverless::EmulatedService;
use smithy_server_runtime::{EndpointUrl, MultiServiceRouter};

use crate::collection_lifecycle::text;

/// This service's SigV4 signing name, which is what routes a control-plane request to it
/// and the name a client signs a data-plane request with
/// (`smithy_query(query="service_info")` -> `sigv4Name`).
const SIGNING_NAME: &str = "aoss";

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The `X-Amz-Target` prefix from this crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "OpenSearchServerless";

/// The path AWS's OpenSearch Dashboards endpoint adds to a collection endpoint.
const DASHBOARDS_PATH: &str = "/_dashboards";

/// One emulator carrying only this service, with the engine the handlers write kept so a
/// test can drive the address a collection is published on, and served at `base` when the
/// emulator is served on a port at all (`MultiServiceRouter::set_url_base`, which is what
/// `OperationContext::endpoint` reports).
fn emulator_served_at(base: Option<&str>) -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The suite exercises the published address, not SigV4: a request carries a credential
    // scope so the router can name the target service and the region, but no signature.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    if let Some(base) = base {
        router.set_url_base(
            EndpointUrl::parse(base).unwrap_or_else(|| panic!("the base URL {base} must parse")),
        );
    }
    (router, engine)
}

/// Call `operation` with `body` as a client that addressed `host`; answer its HTTP status
/// and parsed body.
async fn call_at(
    router: &MultiServiceRouter,
    host: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", host)
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// The `CollectionDetail` `BatchGetCollection` reports for `id`, read as a client that
/// addressed `host`.
async fn read_collection(router: &MultiServiceRouter, host: &str, id: &str) -> Value {
    let (status, read) = call_at(router, host, "BatchGetCollection", json!({ "ids": [id] })).await;
    assert_eq!(
        status, 200,
        "case BatchGetCollection({id}) at {host}: expected 200, got {status} with {read}"
    );
    read.get("collectionDetails")
        .and_then(Value::as_array)
        .and_then(|details| details.first())
        .cloned()
        .unwrap_or(Value::Null)
}

/// Create a collection of `kind` as a client that addressed `host`, and poll until it is
/// ACTIVE; answer the `CollectionDetail` that read reports.
///
/// The `CREATING -> ACTIVE` move is owed by the clock, so this looks again until the
/// collection reports ACTIVE or a generous deadline passes; nothing asserts on how long it
/// took.
async fn active_collection_at(
    router: &MultiServiceRouter,
    host: &str,
    name: &str,
    kind: &str,
) -> Value {
    // A collection's endpoints answer only through a `network` security policy that names
    // it — AWS's "Network access for Amazon OpenSearch Serverless": "you must have at least
    // one network policy that applies to a collection in order to access its endpoint" — so
    // the suite admits traffic to every collection of this emulator first.
    admit_network_access(router, host).await;
    let (status, created) = call_at(
        router,
        host,
        "CreateCollection",
        json!({ "name": name, "type": kind, "encryptionConfig": { "aWSOwnedKey": true } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateCollection({name}) at {host}: expected 200, got {status} with {created}"
    );
    let id = text(&created, "createCollectionDetail.id");
    await_active(router, host, &id).await
}

/// Admit traffic to the endpoints of every collection of this emulator. Idempotent, so a
/// suite that creates several collections calls it per collection.
async fn admit_network_access(router: &MultiServiceRouter, host: &str) {
    /// The one network policy this suite reaches its collections through.
    const NETWORK_POLICY_NAME: &str = "net-every-collection";

    let (held, _) = call_at(
        router,
        host,
        "GetSecurityPolicy",
        json!({ "type": "network", "name": NETWORK_POLICY_NAME }),
    )
    .await;
    if held == 200 {
        return;
    }
    let policy = json!({
        "Rules": [{ "ResourceType": "collection", "Resource": ["collection/*"] }],
        "AllowFromPublic": true,
    });
    let (status, answer) = call_at(
        router,
        host,
        "CreateSecurityPolicy",
        json!({
            "type": "network",
            "name": NETWORK_POLICY_NAME,
            "policy": policy.to_string(),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateSecurityPolicy(network/{NETWORK_POLICY_NAME}) at {host}: expected 200, \
         got {status} with {answer}"
    );
}

/// Poll `BatchGetCollection` until `id` is ACTIVE, answering the `CollectionDetail` that
/// read reports.
///
/// The `CREATING -> ACTIVE` move is owed by the clock, so this looks again until the
/// collection reports ACTIVE or a generous deadline passes; nothing asserts on how long it
/// took.
async fn await_active(router: &MultiServiceRouter, host: &str, id: &str) -> Value {
    for _ in 0..200 {
        let detail = read_collection(router, host, id).await;
        if text(&detail, "status") == "ACTIVE" {
            return detail;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("case {id} at {host}: the collection never reached ACTIVE");
}

/// The four endpoints a collection publishes, in the order `CollectionDetail` carries
/// them: the collection endpoint, its Dashboards endpoint, and the FIPS pair.
fn published(detail: &Value) -> Vec<(&'static str, String)> {
    vec![
        ("collectionEndpoint", text(detail, "collectionEndpoint")),
        ("dashboardEndpoint", text(detail, "dashboardEndpoint")),
        (
            "fipsEndpoints.collectionEndpoint",
            text(detail, "fipsEndpoints.collectionEndpoint"),
        ),
        (
            "fipsEndpoints.dashboardEndpoint",
            text(detail, "fipsEndpoints.dashboardEndpoint"),
        ),
    ]
}

/// The `host:port` a published endpoint addresses — the authority a client dials, with any
/// path dropped.
fn authority_of(url: &str) -> String {
    let after_scheme = url.strip_prefix("https://").unwrap_or(url);
    after_scheme
        .split('/')
        .next()
        .unwrap_or(after_scheme)
        .to_string()
}

/// The socket address a published endpoint names, so a test can open a connection to it.
fn socket_addr_of(url: &str) -> std::net::SocketAddr {
    use std::net::ToSocketAddrs;

    let authority = authority_of(url);
    authority
        .to_socket_addrs()
        .unwrap_or_else(|error| panic!("endpoint {url:?} has no socket address: {error}"))
        .next()
        .unwrap_or_else(|| panic!("endpoint {url:?} resolves to no socket address"))
}

/// One OpenSearch REST request a client sends to a published endpoint: `method path` with
/// `body`, answered by the listener that endpoint is published on
/// (`state::collection_plane` hands every request there to this one function).
fn through_endpoint(
    engine: &Arc<engine::InMemoryEngine>,
    collection_id: &str,
    endpoint: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let state = service_state(&**engine).expect("the service store is reachable");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("{endpoint}{path}"))
        .header("host", authority_of(endpoint))
        .header("content-type", "application/json")
        .body(
            body.map(|body| body.to_string().into_bytes())
                .unwrap_or_default(),
        )
        .expect("a well-formed request to a collection endpoint");
    let response = serve_published(&**engine, &state, collection_id, &request);
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// One OpenSearch REST request sent to the emulator's own FRONT DOOR addressed to `host`,
/// which is how a client reaches a collection whose published authority it carries in the
/// `Host` header: no RPC target, because the OpenSearch REST API has none.
async fn rest_at_front_door(
    router: &MultiServiceRouter,
    host: &str,
    method: &str,
    path: &str,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", host)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDDATAPLANE/20260101/{REGION}/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// A `knn_vector` index body of `dimensions` dimensions — the index a Bedrock knowledge/// base requires to exist in the collection, and the body the recorded `indices.create`
/// carries.
fn knn_index_body(dimensions: usize) -> Value {
    json!({
        "settings": { "index": { "knn": true } },
        "mappings": { "properties": {
            "bedrock-knowledge-base-default-vector": {
                "type": "knn_vector",
                "dimension": dimensions,
            },
            "AMAZON_BEDROCK_TEXT_CHUNK": { "type": "text" },
        } },
    })
}

/// RULE: every endpoint a collection publishes names an address this emulator answers the
/// collection's OpenSearch REST surface on, at the ROOT of that address — so a client
/// configured with the published host and port reaches the collection with no prefix it
/// would have to be told about — and no published endpoint names one of AWS's own hosts.
///
/// The rows span the collection type, the address the creating client reached this emulator
/// at, and whether this process is served on a port at all.
#[tokio::test]
async fn every_published_collection_endpoint_is_an_address_this_emulator_serves() {
    /// One case: the collection created, who created it and where the emulator is served.
    struct Row {
        case: &'static str,
        kind: &'static str,
        caller_host: &'static str,
        served_base: Option<&'static str>,
    }

    let aws_host = format!("{SIGNING_NAME}.{REGION}.amazonaws.com");
    let rows: Vec<Row> = vec![
        Row {
            case: "a vector collection created from a client on the emulator's own endpoint",
            kind: "VECTORSEARCH",
            caller_host: "127.0.0.1:9297",
            served_base: Some("https://127.0.0.1:39355"),
        },
        Row {
            case: "a search collection created from a client that resolves AWS's own hosts",
            kind: "SEARCH",
            caller_host: "aws-host",
            served_base: Some("https://127.0.0.1:39355"),
        },
        Row {
            case: "a time-series collection on an emulator served on no port at all",
            kind: "TIMESERIES",
            caller_host: "aws-host",
            served_base: None,
        },
        Row {
            case: "a vector collection created from a loopback name",
            kind: "VECTORSEARCH",
            caller_host: "localhost:9297",
            served_base: None,
        },
    ];

    for Row {
        case,
        kind,
        caller_host,
        served_base,
    } in rows
    {
        let (router, engine) = emulator_served_at(served_base);
        let host = match caller_host {
            "aws-host" => aws_host.clone(),
            named => named.to_string(),
        };
        let detail = active_collection_at(&router, &host, "announced-endpoints", kind).await;
        let id = text(&detail, "id");
        let endpoint = text(&detail, "collectionEndpoint");

        // 1. Every published endpoint is an address this emulator answers on, and none of
        //    them is one of AWS's own hosts.
        for (member, url) in published(&detail) {
            assert!(
                url.starts_with("https://"),
                "case {case:?}: {member} must be an HTTPS address, got {url:?}"
            );
            assert!(
                !url.contains("amazonaws.com"),
                "case {case:?}: {member} must not name an AWS host, got {url:?}"
            );
            let authority = authority_of(&url);
            assert!(
                authority.rsplit(':').next().is_some_and(|port| port
                    .parse::<u16>()
                    .is_ok_and(|port| port != 0)),
                "case {case:?}: {member} must name the port it is served on, got {url:?}"
            );
        }
        // The Dashboards endpoint is the collection endpoint plus the documented path, and
        // the FIPS pair names the one address this emulator serves the collection at.
        let expected: Vec<String> = vec![
            endpoint.clone(),
            format!("{endpoint}{DASHBOARDS_PATH}"),
            endpoint.clone(),
            format!("{endpoint}{DASHBOARDS_PATH}"),
        ];
        let reported: Vec<String> = published(&detail)
            .into_iter()
            .map(|(_member, url)| url)
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the published endpoints {expected:?}, got {reported:?}"
        );

        // 2. The address is OPEN: a client can connect to it. The listener binds the
        //    interface the core chose (loopback unless the operator asked otherwise), so
        //    the connection is made to the loopback address of the published port.
        for (member, url) in published(&detail) {
            let addr = socket_addr_of(&url);
            let opened = match addr.ip() {
                std::net::IpAddr::V4(_) => {
                    std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, addr.port()))
                }
                std::net::IpAddr::V6(_) => {
                    std::net::TcpStream::connect((std::net::Ipv6Addr::LOCALHOST, addr.port()))
                }
            };
            assert!(
                opened.is_ok(),
                "case {case:?}: {member} must be an address a client can open, got {addr} \
                 refusing the connection: {opened:?}"
            );
        }

        // 3. The address SERVES the collection's OpenSearch REST surface at its root: the
        //    vector index a knowledge base requires is created there and read back.
        let index = "bedrock-knowledge-base-index";
        let (status, created) = through_endpoint(
            &engine,
            &id,
            &endpoint,
            "PUT",
            &format!("/{index}"),
            Some(&knn_index_body(1536)),
        );
        assert_eq!(
            (status, created.get("acknowledged")),
            (200, Some(&json!(true))),
            "case {case:?}: PUT {endpoint}/{index} expected 200 acknowledged, got {status} with \
             {created}"
        );
        let (status, read) = through_endpoint(
            &engine,
            &id,
            &endpoint,
            "GET",
            &format!("/{index}"),
            None,
        );
        assert_eq!(
            status, 200,
            "case {case:?}: GET {endpoint}/{index} expected 200, got {status} with {read}"
        );
        assert!(
            read.get(index).is_some(),
            "case {case:?}: GET {endpoint}/{index} must report the index, got {read}"
        );

        // 4. The emulator's own FRONT DOOR answers the same request addressed to the
        //    collection's published authority, port and all — the addressing a client that
        //    was handed the endpoint earlier, or that reaches this process on one socket,
        //    still sends.
        let (status, read) = rest_at_front_door(
            &router,
            &authority_of(&endpoint),
            "GET",
            &format!("/{index}"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GET /{index} at the front door for host {:?} expected 200, got \
             {status} with {read}",
            authority_of(&endpoint)
        );
        assert!(
            read.get(index).is_some(),
            "case {case:?}: the front door must report the index of the collection the \
             published authority names, got {read}"
        );

        // 5. The control plane keeps reporting the same addresses on a later read, so a
        //    client that stored the endpoint keeps dialling the one this emulator serves.
        let again = read_collection(&router, &host, &id).await;
        assert_eq!(
            published(&again),
            published(&detail),
            "case {case:?}: a later read must report the same addresses, got {again}"
        );
    }
}

/// RULE: a published endpoint addresses ONE collection, so two collections of the same
/// emulator publish two different addresses and each serves only its own indexes.
#[tokio::test]
async fn each_collection_publishes_an_address_of_its_own() {
    let (router, engine) = emulator_served_at(Some("https://127.0.0.1:39355"));
    let host = format!("{SIGNING_NAME}.{REGION}.amazonaws.com");
    let first = active_collection_at(&router, &host, "published-first", "VECTORSEARCH").await;
    let second = active_collection_at(&router, &host, "published-second", "VECTORSEARCH").await;
    let first_endpoint = text(&first, "collectionEndpoint");
    let second_endpoint = text(&second, "collectionEndpoint");
    assert_ne!(
        first_endpoint, second_endpoint,
        "case two collections: each must publish an address of its own, got \
         {first_endpoint:?} twice"
    );

    let index = "only-in-first";
    let (status, created) = through_endpoint(
        &engine,
        &text(&first, "id"),
        &first_endpoint,
        "PUT",
        &format!("/{index}"),
        Some(&knn_index_body(8)),
    );
    assert_eq!(
        status, 200,
        "case PUT {first_endpoint}/{index}: expected 200, got {status} with {created}"
    );

    // (case, the collection read, the endpoint it publishes, the status the read answers)
    let rows: Vec<(&str, &Value, &String, u16)> = vec![
        (
            "the collection that holds the index",
            &first,
            &first_endpoint,
            200,
        ),
        (
            "the collection that does not",
            &second,
            &second_endpoint,
            404,
        ),
    ];
    for (case, collection, endpoint, expected) in rows {
        let (status, read) = through_endpoint(
            &engine,
            &text(collection, "id"),
            endpoint,
            "GET",
            &format!("/{index}"),
            None,
        );
        assert_eq!(
            status, expected,
            "case {case}: GET {endpoint}/{index} expected {expected}, got {status} with {read}"
        );
    }
}

/// RULE: an address is published only while the collection behind it exists — once a
/// deleted collection is purged, the address this service announced for it serves nothing.
#[tokio::test]
async fn a_purged_collections_address_stops_serving() {
    let (router, engine) = emulator_served_at(Some("https://127.0.0.1:39355"));
    let host = format!("{SIGNING_NAME}.{REGION}.amazonaws.com");
    let collection = active_collection_at(&router, &host, "published-deleted", "SEARCH").await;
    let id = text(&collection, "id");
    let endpoint = text(&collection, "collectionEndpoint");
    let index = "before-the-delete";
    let (status, created) = through_endpoint(
        &engine,
        &id,
        &endpoint,
        "PUT",
        &format!("/{index}"),
        Some(&knn_index_body(4)),
    );
    assert_eq!(
        status, 200,
        "case PUT {endpoint}/{index}: expected 200, got {status} with {created}"
    );

    let (status, deleted) = call_at(&router, &host, "DeleteCollection", json!({ "id": &id })).await;
    assert_eq!(
        status, 200,
        "case DeleteCollection({id}): expected 200, got {status} with {deleted}"
    );

    // The purge is owed by the clock: look again until the collection is gone, then the
    // address it was published on serves nothing.
    for _ in 0..200 {
        if read_collection(&router, &host, &id).await.is_null() {
            let (status, answer) = through_endpoint(
                &engine,
                &id,
                &endpoint,
                "GET",
                &format!("/{index}"),
                None,
            );
            assert_eq!(
                status, 503,
                "case GET {endpoint}/{index} after the purge: expected 503, got {status} with \
                 {answer}"
            );
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("case DeleteCollection({id}): the collection was never purged");
}

/// RULE: a collection keeps the address it was published at, so a client that was handed an
/// endpoint keeps reaching the collection there — a retried create (a replayed
/// `clientToken`) answers the collection the first call created and moves nothing.
///
/// The rows span what a retry can carry beside the token: the same request again, and one
/// that names the collection's other members too.
#[tokio::test]
async fn a_retried_create_keeps_the_address_the_first_call_published() {
    let (router, engine) = emulator_served_at(Some("https://127.0.0.1:39355"));
    let host = format!("{SIGNING_NAME}.{REGION}.amazonaws.com");
    let name = "published-retried";
    let token = "published-retried-token";
    // As for every collection this suite addresses: its endpoints answer only through a
    // network policy that names it.
    admit_network_access(&router, &host).await;
    let request = json!({
        "name": name,
        "type": "VECTORSEARCH",
        "encryptionConfig": { "aWSOwnedKey": true },
        "clientToken": token,
    });
    let (status, created) = call_at(&router, &host, "CreateCollection", request.clone()).await;
    assert_eq!(
        status, 200,
        "case CreateCollection({name}): expected 200, got {status} with {created}"
    );
    let id = text(&created, "createCollectionDetail.id");
    let first = await_active(&router, &host, &id).await;
    let endpoint = text(&first, "collectionEndpoint");
    let index = "survives-the-retry";
    let (status, answer) = through_endpoint(
        &engine,
        &id,
        &endpoint,
        "PUT",
        &format!("/{index}"),
        Some(&knn_index_body(16)),
    );
    assert_eq!(
        status, 200,
        "case PUT {endpoint}/{index}: expected 200, got {status} with {answer}"
    );

    // (case, the retried request)
    let retries: Vec<(&str, Value)> = vec![
        ("the same request again", request.clone()),
        (
            "a retry naming the other members too",
            json!({
                "name": name,
                "type": "VECTORSEARCH",
                "encryptionConfig": { "aWSOwnedKey": true },
                "standbyReplicas": "DISABLED",
                "clientToken": token,
            }),
        ),
    ];
    for (case, retried) in retries {
        let (status, replayed) = call_at(&router, &host, "CreateCollection", retried).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {replayed}"
        );
        assert_eq!(
            text(&replayed, "createCollectionDetail.id"),
            id,
            "case {case}: the retry must answer the collection the first call created, got \
             {replayed}"
        );
        let read = read_collection(&router, &host, &id).await;
        assert_eq!(
            published(&read),
            published(&first),
            "case {case}: the published addresses must not move, got {read}"
        );
        // The address still serves what was written through it, which is what a client
        // dialling the endpoint it was handed sees.
        let (status, answer) = through_endpoint(
            &engine,
            &id,
            &endpoint,
            "GET",
            &format!("/{index}"),
            None,
        );
        assert_eq!(
            status, 200,
            "case {case}: GET {endpoint}/{index} expected 200, got {status} with {answer}"
        );
        let addr = socket_addr_of(&endpoint);
        let opened = match addr.ip() {
            std::net::IpAddr::V4(_) => {
                std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, addr.port()))
            }
            std::net::IpAddr::V6(_) => {
                std::net::TcpStream::connect((std::net::Ipv6Addr::LOCALHOST, addr.port()))
            }
        };
        assert!(
            opened.is_ok(),
            "case {case}: the published port must still be open, got {addr} refusing the \
             connection: {opened:?}"
        );
    }
}
