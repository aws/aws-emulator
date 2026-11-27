// SPDX-License-Identifier: CC0-1.0

//! A cluster MEMBER's published address answers the Neptune graph API too, and a later READ
//! reports the same pair the create published.
//!
//! The rule this suite pins is the one `neptune_cluster_endpoint_serves_the_graph_api.rs`
//! states for the cluster envelope — `Endpoint` is the HOST this emulator serves the plane
//! at, and `Port` is the port the create configured — carried to the two places that suite
//! leaves open:
//!
//! * a MEMBER instance of the cluster, whose own `Endpoint$Address` is a second address onto
//!   the same data plane and whose `Endpoint$Port` is its CLUSTER's, the port
//!   `CreateDBInstance$Port` says is "managed by the DB cluster"; and
//! * the READ back, because the address is persisted on the record rather than recomputed
//!   per response: `DescribeDBClusters` and `DescribeDBInstances` report the pair the
//!   creates published, and the graph API still answers there.
//!
//! This suite needs the WHOLE router for the same reason that one does: the control plane
//! that publishes the address (Amazon RDS, which serves the Neptune control plane's model
//! too) and the data plane that has to answer at it (neptunedata) are two services.
//! Assembling the router costs stack, so it is built — and the requests driven — on an
//! explicitly sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::element;
use crate::rds_docdb_neptune_signing_name_routing::{authorization, API_VERSION, BOOT_STACK_SIZE};

/// The graph engine of the Amazon Neptune control plane the RDS front door serves.
const GRAPH_ENGINE: &str = "neptune";

/// The signing name an AWS SDK Neptune control-plane client signs with: the Neptune model
/// gives it no other name.
const CONTROL_PLANE_SIGNING_NAME: &str = "rds";

/// The SigV4 signing name of the Neptune graph API, which is what routes a data-plane
/// request to the neptunedata service.
const GRAPH_API_SIGNING_NAME: &str = "neptune-db";

/// The instance class the orderable-options report offers for a member of a Neptune cluster.
const MEMBER_CLASS: &str = "db.t4g.medium";

/// The endpoint the control-plane client below is configured with — the address every
/// create and read reaches this emulator at, and so the address the cluster and its member
/// publish.
const CLIENT_ENDPOINT: &str = "https://127.0.0.1:9297/";

/// The host that endpoint names, which is the host the published members must carry, beside
/// the port the cluster's create configured — here the engine default `CreateDBCluster$Port`
/// publishes ("Default: 8182"), since the create below names no `Port`.
const SERVED_AT: (&str, &str) = ("127.0.0.1", "8182");

/// The port the member's create asks for: `CreateDBInstance$Port` is "Not applicable. The
/// port is managed by the DB cluster", so it is not the port the member is reached at.
const REQUESTED_ENGINE_PORT: &str = "7777";

/// One awsQuery call against the RDS front door, addressed to [`CLIENT_ENDPOINT`].
async fn control_plane_call(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={API_VERSION}");
    for (name, value) in params {
        body.push_str(&format!("&{name}={value}"));
    }
    let request = ::http::Request::builder()
        .method("POST")
        .uri(CLIENT_ENDPOINT)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(CONTROL_PLANE_SIGNING_NAME))
        .body(bytes::Bytes::from(body))
        .expect("a well-formed control-plane request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// The `<Endpoint>` SUBTREE of an instance envelope, which is a structure of `<Address>` and
/// `<Port>` rather than the host string a cluster's own `<Endpoint>` carries.
fn instance_endpoint(answer: &str) -> String {
    let (_, rest) = answer
        .split_once("<Endpoint>")
        .unwrap_or_else(|| panic!("the answer must carry an <Endpoint>, got {answer}"));
    let (subtree, _) = rest
        .split_once("</Endpoint>")
        .unwrap_or_else(|| panic!("the <Endpoint> must be closed, got {answer}"));
    subtree.to_string()
}

/// `GetEngineStatus` — `GET /status` of the graph API — dialled at `authority`, the pair the
/// control plane published.
async fn dial_graph_api(router: &MultiServiceRouter, authority: &str) -> (u16, String) {
    let request = ::http::Request::builder()
        .method("GET")
        .uri(format!("https://{authority}/status"))
        .header("host", authority)
        .header("authorization", authorization(GRAPH_API_SIGNING_NAME))
        .body(bytes::Bytes::new())
        .expect("a well-formed data-plane request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

#[test]
fn a_cluster_member_publishes_the_same_dialable_address_and_reads_report_it() {
    std::thread::Builder::new()
        .name("neptune-member-endpoint".to_string())
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

            let (expected_host, expected_port) = SERVED_AT;
            let cluster = "emu-graph-member-cluster";
            let member = "emu-graph-member";

            let (status, created) = runtime.block_on(control_plane_call(
                &router,
                "CreateDBCluster",
                &[("DBClusterIdentifier", cluster), ("Engine", GRAPH_ENGINE)],
            ));
            assert_eq!(
                status, 200,
                "the cluster create must be answered, got {status} with {created}"
            );
            assert_eq!(
                (
                    element(&created, "Endpoint").as_str(),
                    element(&created, "Port").as_str()
                ),
                (expected_host, expected_port),
                "the cluster must publish the host this emulator answers its graph API at \
                 beside the port its create configured, got {created}"
            );

            // A member of that cluster is a SECOND address onto the same data plane, and the
            // port it is reached at is the one its cluster manages.
            let (status, member_created) = runtime.block_on(control_plane_call(
                &router,
                "CreateDBInstance",
                &[
                    ("DBInstanceIdentifier", member),
                    ("Engine", GRAPH_ENGINE),
                    ("DBInstanceClass", MEMBER_CLASS),
                    ("DBClusterIdentifier", cluster),
                    ("Port", REQUESTED_ENGINE_PORT),
                ],
            ));
            assert_eq!(
                status, 200,
                "the member create must be answered, got {status} with {member_created}"
            );
            let endpoint = instance_endpoint(&member_created);
            assert_eq!(
                (
                    element(&endpoint, "Address").as_str(),
                    element(&endpoint, "Port").as_str()
                ),
                (expected_host, expected_port),
                "the member must publish the same host and its cluster's port, got \
                 {endpoint}"
            );

            // ROUND TRIP: the pair is persisted on both records, so the reads report it.
            let (status, described) = runtime.block_on(control_plane_call(
                &router,
                "DescribeDBClusters",
                &[("DBClusterIdentifier", cluster)],
            ));
            assert_eq!(
                status, 200,
                "the cluster read must be answered, got {status} with {described}"
            );
            assert_eq!(
                (
                    element(&described, "Endpoint").as_str(),
                    element(&described, "ReaderEndpoint").as_str(),
                    element(&described, "Port").as_str()
                ),
                (expected_host, expected_host, expected_port),
                "DescribeDBClusters must report the address the create published, got \
                 {described}"
            );

            let (status, described_member) = runtime.block_on(control_plane_call(
                &router,
                "DescribeDBInstances",
                &[("DBInstanceIdentifier", member)],
            ));
            assert_eq!(
                status, 200,
                "the member read must be answered, got {status} with {described_member}"
            );
            let read_endpoint = instance_endpoint(&described_member);
            assert_eq!(
                (
                    element(&read_endpoint, "Address").as_str(),
                    element(&read_endpoint, "Port").as_str()
                ),
                (expected_host, expected_port),
                "DescribeDBInstances must report the address the create published, got \
                 {read_endpoint}"
            );

            // And the address a client composes from what the READ reported answers the
            // graph API, which is the whole point of publishing the pair.
            let authority = format!(
                "{}:{}",
                element(&read_endpoint, "Address"),
                element(&read_endpoint, "Port")
            );
            let (status, answered) = runtime.block_on(dial_graph_api(&router, &authority));
            assert_eq!(
                status, 200,
                "GET https://{authority}/status must be answered by the graph API, got \
                 {status} with {answered}"
            );
            assert!(
                answered.contains("\"status\""),
                "the graph API must report the engine's status at {authority}, got {answered}"
            );
        })
        .expect("spawn the member-endpoint thread")
        .join()
        .expect("a cluster member must publish the same dialable address as its cluster");
}
