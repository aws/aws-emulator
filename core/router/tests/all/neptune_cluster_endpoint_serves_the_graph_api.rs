// SPDX-License-Identifier: CC0-1.0

//! A DB cluster's published connection endpoint is an address this emulator ANSWERS that
//! cluster's data plane on.
//!
//! `DBCluster$Endpoint` is "the connection endpoint for the primary instance of the DB
//! cluster": the HOST a client dials, because an Amazon Neptune cluster's data plane — the
//! graph API, restJson1, signing name `neptune-db` — has no regional endpoint of its own and
//! is addressed at the cluster's connection endpoint and nowhere else. This emulator serves
//! that API at its OWN endpoint, so the host a Neptune cluster publishes is this process's;
//! a `*.neptune.amazonaws.com` name resolves nowhere here, and publishing it would leave the
//! whole 43-operation graph API unreachable.
//!
//! `DBCluster$Port` is "the port that the database engine is listening on", which
//! `CreateDBCluster$Port` CONFIGURES ("Default: 8182") — a stored property of the cluster,
//! the same on every read and across restarts, and never the port this process happens to be
//! bound to.
//!
//! So each row below creates a cluster through the control plane, asserts the exact host and
//! the exact port it published — those two assertions are what pin this rule — and then
//! composes `https://<Endpoint>:<Port>` exactly as a client does and DIALS it with a
//! neptunedata request: the published address must reach the graph API's own front door and
//! be answered by it. (The dial goes through the router, which resolves a service from the
//! signing name rather than from the authority's port, so it is the two comparisons above it
//! that pin what was published — do not weaken them on the strength of the dial.)
//!
//! The rows differ along what the rule generalises over: the address the control-plane
//! client is configured with (loopback by number and by name, with a port and without), the
//! port the create asks for (none, the engine default, another value), and an engine whose
//! data plane this emulator serves neither as an API nor as a native wire — which keeps the
//! AWS DNS name and the engine's own default port, because there is no host of this process
//! to publish for it.
//!
//! This suite needs the WHOLE router: the control plane that publishes the address (Amazon
//! RDS, which serves the Neptune control plane's model too) and the data plane that has to
//! answer at it (neptunedata) are two services, so a single-service test cannot see the
//! hand-off. Assembling the router costs stack, so it is built — and the requests driven —
//! on an explicitly sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::element;
use crate::rds_docdb_neptune_signing_name_routing::{authorization, API_VERSION, BOOT_STACK_SIZE};

/// The graph engine of the Amazon Neptune control plane this front door serves.
/// `CreateDBCluster$Engine`: "Valid values: … neptune".
const GRAPH_ENGINE: &str = "neptune";

/// The SigV4 signing name of the Neptune graph API, which is what routes a data-plane
/// request to the neptunedata service.
/// Source: `smithy_query(query="service_info")` on the neptunedata model — `sigv4Name:
/// neptune-db`.
const GRAPH_API_SIGNING_NAME: &str = "neptune-db";

/// The signing name the RDS front door serves the Neptune control plane's model behind: an
/// AWS SDK Neptune client signs `rds`, because that model gives it no other name.
const CONTROL_PLANE_SIGNING_NAME: &str = "rds";

/// The statuses `GetEngineStatusOutput$status` reports: "Set to `healthy` if the instance is
/// not experiencing problems. If the instance is recovering from a crash or from being
/// rebooted and there are active transactions running from the latest server shutdown,
/// status is set to `recovery`".
const ENGINE_STATUSES: [&str; 2] = ["healthy", "recovery"];

/// One row of the published-address table.
struct Row {
    case: &'static str,
    /// The engine the cluster is created with.
    engine: &'static str,
    /// The endpoint the control-plane client is configured with — the address the create
    /// reaches this emulator at.
    endpoint: &'static str,
    /// Request members beyond `DBClusterIdentifier` and `Engine`.
    extra: &'static [(&'static str, &'static str)],
    /// The `host` this emulator is reached at and the `Port` the create configured, which is
    /// what a cluster whose data plane this emulator serves must publish; `None` for one it
    /// serves neither way, which keeps the AWS DNS name of its engine's control plane and
    /// the port the create configured.
    served_at: Option<(&'static str, &'static str)>,
}

const ROWS: &[Row] = &[
    Row {
        case: "a Neptune cluster created through this emulator's default data port",
        engine: GRAPH_ENGINE,
        endpoint: "https://127.0.0.1:9297/",
        extra: &[],
        // `CreateDBClusterMessage$Port` of the Neptune model: "Default: 8182".
        served_at: Some(("127.0.0.1", "8182")),
    },
    Row {
        case: "a Neptune cluster whose create asked for the Neptune engine port",
        engine: GRAPH_ENGINE,
        endpoint: "https://127.0.0.1:44903/",
        extra: &[("Port", "8182")],
        served_at: Some(("127.0.0.1", "8182")),
    },
    Row {
        case: "a Neptune cluster reached by the loopback NAME, asking for another port",
        engine: GRAPH_ENGINE,
        endpoint: "https://localhost:41509/",
        extra: &[("Port", "9999")],
        served_at: Some(("localhost", "9999")),
    },
    Row {
        case: "a Neptune cluster reached at an endpoint that names no port",
        engine: GRAPH_ENGINE,
        endpoint: "https://127.0.0.1/",
        // The floor of the range an endpoint port may fall in ("Valid Values: 1150-65535").
        extra: &[("Port", "1150")],
        served_at: Some(("127.0.0.1", "1150")),
    },
    Row {
        case: "a DocumentDB cluster, whose data plane this emulator serves neither way",
        engine: "docdb",
        endpoint: "https://127.0.0.1:9297/",
        extra: &[],
        served_at: None,
    },
];

/// `CreateDBCluster` for `row`, addressed to the endpoint the row's client is configured
/// with: the status and the XML answer.
async fn create_cluster(
    router: &MultiServiceRouter,
    row: &Row,
    identifier: &str,
) -> (u16, String) {
    let mut body = format!(
        "Action=CreateDBCluster&Version={API_VERSION}&DBClusterIdentifier={identifier}\
         &Engine={}",
        row.engine
    );
    for (name, value) in row.extra {
        body.push_str(&format!("&{name}={value}"));
    }
    let request = ::http::Request::builder()
        .method("POST")
        .uri(row.endpoint)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(CONTROL_PLANE_SIGNING_NAME))
        .body(bytes::Bytes::from(body))
        .expect("a well-formed control-plane request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let rendered = String::from_utf8_lossy(response.body()).to_string();
    (status, rendered)
}

/// One neptunedata request DIALLED at `authority` — the `host:port` the cluster published —
/// exactly as a client configured with that endpoint sends it: the status and the JSON body.
async fn dial_graph_api(
    router: &MultiServiceRouter,
    authority: &str,
    method: &str,
    path: &str,
    payload: Option<Value>,
) -> (u16, Value) {
    let body = payload
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(format!("https://{authority}{path}"))
        .header("host", authority)
        .header("authorization", authorization(GRAPH_API_SIGNING_NAME));
    if payload.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(bytes::Bytes::from(body))
        .expect("a well-formed data-plane request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let answered = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, answered)
}

#[test]
fn a_neptune_clusters_published_endpoint_answers_the_graph_api() {
    std::thread::Builder::new()
        .name("neptune-endpoint".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                // IO and timers are on because a create reaches a service that opens a core
                // listener for the engines whose native wire this emulator speaks.
                .enable_all()
                .build()
                .expect("a current-thread runtime");

            for (index, row) in ROWS.iter().enumerate() {
                let case = row.case;
                let identifier = format!("emu-graph-endpoint-{index}");
                let (status, created) =
                    runtime.block_on(create_cluster(&router, row, &identifier));
                assert_eq!(
                    status, 200,
                    "case {case:?}: the create must be answered, got {status} with {created}"
                );
                let endpoint = element(&created, "Endpoint");
                let reader = element(&created, "ReaderEndpoint");
                let port = element(&created, "Port");

                let Some((expected_host, expected_port)) = row.served_at else {
                    // The boundary of the rule: no data plane of this cluster is served
                    // here, so there is no address of this process to publish and the
                    // cluster keeps the published DNS form of its own control plane, with
                    // the port its engine was configured with.
                    let suffix = ".us-east-1.docdb.amazonaws.com";
                    assert!(
                        endpoint.ends_with(suffix),
                        "case {case:?}: expected an <Endpoint> ending {suffix}, got \
                         {endpoint:?}"
                    );
                    // The port Amazon DocumentDB publishes as its default.
                    assert_eq!(
                        port, "27017",
                        "case {case:?}: expected <Port> \"27017\", got {port:?}"
                    );
                    continue;
                };

                // The published pair: a port-less host beside the port, so the client's own
                // `<Endpoint>:<Port>` composition names exactly one address.
                assert_eq!(
                    endpoint, expected_host,
                    "case {case:?}: expected <Endpoint> {expected_host:?} — the host this \
                     emulator answers the graph API at — got {endpoint:?} in {created}"
                );
                assert_eq!(
                    reader, expected_host,
                    "case {case:?}: one address serves the whole graph API, so \
                     <ReaderEndpoint> must be {expected_host:?}, got {reader:?}"
                );
                assert_eq!(
                    port, expected_port,
                    "case {case:?}: expected <Port> {expected_port:?} — the port the create \
                     configured this cluster's engine with — got {port:?} in {created}"
                );
                let composed = format!("https://{endpoint}:{port}/")
                    .parse::<::http::Uri>()
                    .unwrap_or_else(|error| {
                        panic!("case {case:?}: https://{endpoint}:{port}/ must parse: {error}")
                    });
                assert_eq!(
                    composed.port_u16().map(|named| named.to_string()).as_deref(),
                    Some(expected_port),
                    "case {case:?}: expected the dialled URL to name port \
                     {expected_port:?}, got {:?}",
                    composed.port_u16()
                );

                // And the address really ANSWERS: a neptunedata request dialled at the
                // published pair reaches the graph API's own front door.
                let authority = format!("{endpoint}:{port}");
                let (status, answered) =
                    runtime.block_on(dial_graph_api(&router, &authority, "GET", "/status", None));
                assert_eq!(
                    status, 200,
                    "case {case:?}: GET https://{authority}/status must be answered by the \
                     graph API, got {status} with {answered}"
                );
                let reported = answered["status"].as_str().unwrap_or_default();
                assert!(
                    ENGINE_STATUSES.contains(&reported),
                    "case {case:?}: expected a status in {ENGINE_STATUSES:?}, got \
                     {reported:?} in {answered}"
                );
            }
        })
        .expect("spawn the endpoint thread")
        .join()
        .expect("a Neptune cluster's published endpoint must answer the graph API");
}

#[test]
fn the_published_endpoint_serves_the_graph_itself_not_only_its_status() {
    std::thread::Builder::new()
        .name("neptune-graph".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a current-thread runtime");

            let row = Row {
                case: "an openCypher write and read back through the published endpoint",
                engine: GRAPH_ENGINE,
                endpoint: "https://127.0.0.1:9297/",
                extra: &[],
                served_at: Some(("127.0.0.1", "9297")),
            };
            let case = row.case;
            let (status, created) =
                runtime.block_on(create_cluster(&router, &row, "emu-graph-roundtrip"));
            assert_eq!(
                status, 200,
                "case {case:?}: the create must be answered, got {status} with {created}"
            );
            let authority = format!(
                "{}:{}",
                element(&created, "Endpoint"),
                element(&created, "Port")
            );

            // The rows a write leaves are the rows a later read answers, and both travel
            // through the address the control plane published — which is the whole point:
            // the published endpoint is where the graph lives, not merely where a health
            // report is served. `ExecuteOpenCypherQuery` is `POST /opencypher`, with the
            // query in the body member `query`
            // (`smithy_query(query="operation",operation="ExecuteOpenCypherQuery")`).
            let label = "EmuPublishedEndpointProbe";
            for (step, query) in [
                ("the write", format!("CREATE (n:{label} {{k: 'v'}})")),
                (
                    "the read back",
                    format!("MATCH (n:{label}) RETURN n.k AS k"),
                ),
            ] {
                let (status, answered) = runtime.block_on(dial_graph_api(
                    &router,
                    &authority,
                    "POST",
                    "/opencypher",
                    Some(serde_json::json!({ "query": query })),
                ));
                assert_eq!(
                    status, 200,
                    "case {case:?} ({step}): POST https://{authority}/opencypher must be \
                     answered, got {status} with {answered}"
                );
                if step == "the read back" {
                    let rows = answered["results"].as_array().cloned().unwrap_or_default();
                    let read: Vec<&str> = rows
                        .iter()
                        .map(|row| row["k"].as_str().unwrap_or_default())
                        .collect();
                    assert_eq!(
                        read,
                        vec!["v"],
                        "case {case:?}: the read through the published endpoint must answer \
                         the row the write left, got {answered}"
                    );
                }
            }
        })
        .expect("spawn the graph thread")
        .join()
        .expect("the published endpoint must serve the graph itself");
}
