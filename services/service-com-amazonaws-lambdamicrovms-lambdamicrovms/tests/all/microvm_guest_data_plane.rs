// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the MicroVM GUEST data plane: what a client reads back from the
//! `endpoint` `RunMicrovm` publishes, and which outbound connection the MicroVM's
//! `egressNetworkConnectors` carry.
//!
//! Each test states a RULE and exercises it over a SET of inputs; the addresses, ports
//! and ids any one row carries are examples, not the specification. The emulator here
//! mounts the owners of everything the decision reads on ONE engine — this service, the
//! object store the guest's code artifact is staged in, Amazon EC2 (the subnet, the
//! destination interface and its security groups) and lambda-core (the connector) — so
//! every answer is computed from records those services really hold.
//!
//! The guest's program is a Node-shaped HTTP server written here and packaged into the
//! .zip a client uploads: it answers `/health` and, for `/connect?host=&port=`, opens ONE
//! outbound connection and reports what it observed of it. Nothing in the assertions
//! below is a value this emulator chooses — each is what that program computed.

use std::io::Write;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::microvm_endpoint::{
    serve_request, AUTH_TOKEN_HEADER,
};
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::microvm_network::connector_state;
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::ComAmazonawsLambdamicrovmsLambdamicrovmsState as MicrovmState;
use smithy_server_runtime::MultiServiceRouter;

use crate::microvm_lifecycle::{call, image_body, run_microvm, REGION};

/// The VPC the connector reaches into, and the subnet its interfaces live in.
const VPC_CIDR: &str = "10.90.0.0/16";

/// The subnet the connector is configured with.
const SUBNET_CIDR: &str = "10.90.1.0/24";

/// The address the destination interface holds in that subnet.
const TARGET_ADDRESS: &str = "10.90.1.4";

/// The port the destination's security group admits.
const ADMITTED_PORT: i32 = 443;

/// The link-local block a guest's own source address comes out of (RFC 3927), which is
/// what the connector-carried path's local end must be inside of.
const LINK_LOCAL_PREFIX: (u8, u8) = (169, 254);

/// The AWS-managed internet-egress connector, in the ARN form an AWS-owned connector of
/// lambda's own namespace carries.
fn internet_egress_arn() -> String {
    format!("arn:aws:lambda:{REGION}:aws:network-connector:aws-network-connector:INTERNET_EGRESS")
}

/// An operator role for the connector: `NetworkConnectorRoleArn`, pattern
/// `^arn:(aws[a-zA-Z-]*)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`.
const OPERATOR_ROLE: &str = "arn:aws:iam::123456789012:role/connector-operator";

/// How long the whole poll for one clock-owed convergence may take before the test gives
/// up: generous, because what is asserted is that the transition HAPPENS, not when.
const SETTLE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// How long one poll of a settling resource waits before looking again.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// The guest program this suite runs: a Node-shaped HTTP server that answers `/health`
/// and reports ONE outbound connection for `/connect?host=&port=`.
const GUEST_SERVER: &str = r#"
const http = require('http');
const net = require('net');
const url = require('url');

http.createServer(function (req, res) {
    const parsed = url.parse(req.url, true);
    const answer = function (status, body) {
        res.writeHead(status, {'content-type': 'application/json'});
        res.end(JSON.stringify(body));
    };
    if (parsed.pathname === '/health') {
        answer(200, {healthy: true});
        return;
    }
    if (parsed.pathname !== '/connect') {
        answer(404, {error: 'no such path'});
        return;
    }
    const host = parsed.query.host;
    const port = Number(parsed.query.port);
    const socket = net.connect({host: host, port: port});
    let reported = false;
    const report = function (outcome) {
        if (reported) { return; }
        reported = true;
        const body = {connected: outcome === 'connected', outcome: outcome, peer: [host, port]};
        if (outcome === 'connected') {
            body.local = [socket.localAddress, socket.localPort];
        }
        answer(200, body);
    };
    socket.setTimeout(5000, function () { socket.destroy(); report('timed_out'); });
    socket.on('connect', function () { report('connected'); socket.destroy(); });
    socket.on('error', function () { report('timed_out'); });
}).listen(8080);
"#;

/// A router carrying this service and every owner of what a guest's connection is decided
/// against, over ONE shared engine.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Verifying signatures is the emulator's concern, not this service's; these tests
    // sign nothing, so the check is off and the scope header is read for routing only.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambdamicrovms_lambdamicrovms::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_lambdacore_lambdacoreapiservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    (router, engine)
}

/// The in-process handle every SDK client below reaches this emulator over.
fn callback(router: &MultiServiceRouter) -> smithy_server_runtime::EmbeddedEmulator {
    smithy_server_runtime::EmbeddedEmulator::from_router(router.clone())
}

/// An Amazon S3 client over this emulator's own in-process callback.
fn s3(router: &MultiServiceRouter) -> aws_sdk_s3::Client {
    let callback = callback(router);
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
        .region(aws_sdk_s3::config::Region::new(REGION))
        .endpoint_url(format!("https://s3.{REGION}.amazonaws.com"))
        .force_path_style(true)
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    aws_sdk_s3::Client::from_conf(config)
}

/// An Amazon EC2 client over the same callback.
fn ec2(router: &MultiServiceRouter) -> aws_sdk_ec2::Client {
    let callback = callback(router);
    let config = aws_sdk_ec2::Config::builder()
        .behavior_version(aws_sdk_ec2::config::BehaviorVersion::latest())
        .region(aws_sdk_ec2::config::Region::new(REGION))
        .endpoint_url(format!("https://ec2.{REGION}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(aws_sdk_ec2::config::retry::RetryConfig::disabled())
        .timeout_config(aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
        .identity_cache(aws_sdk_ec2::config::IdentityCache::no_cache())
        .stalled_stream_protection(aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled())
        .build();
    aws_sdk_ec2::Client::from_conf(config)
}

/// The .zip a client uploads as a MicroVM image's code artifact: the container
/// specification that declares which module starts the guest, and the guest itself.
fn guest_artifact(server: &str) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        for (name, source) in [
            (
                "Dockerfile",
                "FROM node:24-alpine\nCOPY . /app\nWORKDIR /app\nCMD [\"node\", \"server.js\"]\n",
            ),
            ("server.js", server),
        ] {
            writer.start_file(name, options).expect("the entry starts");
            writer
                .write_all(source.as_bytes())
                .expect("the entry's bytes");
        }
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// Stage `bytes` as the object `key` of `bucket`, through Amazon S3's own operations.
async fn stage_artifact(router: &MultiServiceRouter, bucket: &str, key: &str, bytes: Vec<u8>) {
    let client = s3(router);
    let created = client
        .create_bucket()
        .bucket(bucket)
        .create_bucket_configuration(
            aws_sdk_s3::types::CreateBucketConfiguration::builder()
                .location_constraint(aws_sdk_s3::types::BucketLocationConstraint::from(REGION))
                .build(),
        )
        .send()
        .await;
    assert!(
        created.is_ok(),
        "staging {bucket}: the bucket must be created, got {created:?}"
    );
    let put = client
        .put_object()
        .bucket(bucket)
        .key(key)
        .body(aws_sdk_s3::primitives::ByteStream::from(bytes))
        .send()
        .await;
    assert!(
        put.is_ok(),
        "staging s3://{bucket}/{key}: the object must be stored, got {put:?}"
    );
}

/// The network the connector reaches into: a VPC, one subnet, the connector's own
/// security group, and a destination interface in that subnet carrying a group of its
/// own.
struct Network {
    subnet_id: String,
    connector_group: String,
    target_group: String,
    target_address: String,
}

/// Create that network through Amazon EC2's own operations.
async fn network(router: &MultiServiceRouter) -> Network {
    let client = ec2(router);
    let vpc = client
        .create_vpc()
        .cidr_block(VPC_CIDR)
        .send()
        .await
        .expect("the VPC is created");
    let vpc_id = vpc
        .vpc()
        .and_then(|vpc| vpc.vpc_id())
        .expect("the VPC carries an id")
        .to_string();
    let subnet = client
        .create_subnet()
        .vpc_id(&vpc_id)
        .cidr_block(SUBNET_CIDR)
        .send()
        .await
        .expect("the subnet is created");
    let subnet_id = subnet
        .subnet()
        .and_then(|subnet| subnet.subnet_id())
        .expect("the subnet carries an id")
        .to_string();

    let mut groups = Vec::new();
    for name in ["microvm-connector", "private-endpoint"] {
        let group = client
            .create_security_group()
            .group_name(name)
            .description(format!("{name} group"))
            .vpc_id(&vpc_id)
            .send()
            .await
            .unwrap_or_else(|error| panic!("the security group {name} is created: {error:?}"));
        groups.push(
            group
                .group_id()
                .expect("the security group carries an id")
                .to_string(),
        );
    }
    let (connector_group, target_group) = (groups[0].clone(), groups[1].clone());

    let interface = client
        .create_network_interface()
        .subnet_id(&subnet_id)
        .groups(&target_group)
        .private_ip_address(TARGET_ADDRESS)
        .description("the private destination this suite connects to")
        .send()
        .await
        .expect("the destination interface is created");
    let target_address = interface
        .network_interface()
        .and_then(|interface| interface.private_ip_address())
        .expect("the interface carries a private address")
        .to_string();

    Network {
        subnet_id,
        connector_group,
        target_group,
        target_address,
    }
}

/// Authorize, or revoke, TCP `port` on `group` from the VPC's own range — the ingress
/// rule that admits the connector's path at the destination.
async fn ingress(router: &MultiServiceRouter, group: &str, port: i32, admit: bool) {
    let client = ec2(router);
    let rule = aws_sdk_ec2::types::IpPermission::builder()
        .ip_protocol("tcp")
        .from_port(port)
        .to_port(port)
        .ip_ranges(
            aws_sdk_ec2::types::IpRange::builder()
                .cidr_ip(VPC_CIDR)
                .build(),
        )
        .build();
    if admit {
        let authorized = client
            .authorize_security_group_ingress()
            .group_id(group)
            .ip_permissions(rule)
            .send()
            .await;
        assert!(
            authorized.is_ok(),
            "admitting tcp/{port} on {group}: got {authorized:?}"
        );
        return;
    }
    let revoked = client
        .revoke_security_group_ingress()
        .group_id(group)
        .ip_permissions(rule)
        .send()
        .await;
    assert!(
        revoked.is_ok(),
        "revoking tcp/{port} on {group}: got {revoked:?}"
    );
}

/// Create a VPC-egress network connector over lambda-core's own operation and poll it
/// until it reports `ACTIVE`, then answer its ARN.
async fn active_connector(router: &MultiServiceRouter, network: &Network) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/2026-04-04/network-connectors",
        Some(json!({
            "Name": "microvm-vpc-egress",
            "OperatorRole": OPERATOR_ROLE,
            "Configuration": {
                "VpcEgressConfiguration": {
                    "SubnetIds": [network.subnet_id],
                    "SecurityGroupIds": [network.connector_group],
                    "NetworkProtocol": "IPv4",
                    "AssociatedComputeResourceTypes": ["MicroVm"],
                }
            },
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "create the network connector: got {status} {created}"
    );
    let arn = created["Arn"]
        .as_str()
        .expect("the connector carries an ARN")
        .to_string();

    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, read) = call(
            router,
            "GET",
            &format!("/2026-04-04/network-connectors/{}", connector_id(&arn)),
            None,
        )
        .await;
        assert_eq!(status, 200, "read the connector: got {status} {read}");
        if read["State"].as_str() == Some("ACTIVE") {
            return arn;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the connector never reached ACTIVE: last read {read}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The connector id an ARN names — its last segment.
fn connector_id(arn: &str) -> &str {
    arn.rsplit(':').next().unwrap_or(arn)
}

/// Create the image whose code artifact is the staged guest, and poll it until its build
/// has settled with an active version.
async fn guest_image(router: &MultiServiceRouter, name: &str, uri: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/2025-09-09/microvm-images",
        Some(image_body(name, json!({"codeArtifact": {"uri": uri}}))),
    )
    .await;
    assert_eq!(status, 201, "create the image {name}: got {status} {created}");
    let image_arn = created["imageArn"]
        .as_str()
        .expect("the image carries an ARN")
        .to_string();

    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, read) = call(
            router,
            "GET",
            &format!("/2025-09-09/microvm-images/{name}"),
            None,
        )
        .await;
        assert_eq!(status, 200, "read the image {name}: got {status} {read}");
        if read["latestActiveImageVersion"].is_string() {
            return image_arn;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the image {name} never settled with an active version: last read {read}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Run a MicroVM from `image` attached to `egress`, poll it until it reports `RUNNING`,
/// and answer its id and published endpoint.
async fn running_microvm(
    router: &MultiServiceRouter,
    image: &str,
    egress: &[String],
) -> (String, String) {
    let (status, run) = run_microvm(
        router,
        json!({
            "imageIdentifier": image,
            "egressNetworkConnectors": egress,
        }),
    )
    .await;
    assert_eq!(status, 200, "run a MicroVM: got {status} {run}");
    let microvm_id = run["microvmId"]
        .as_str()
        .expect("the MicroVM carries an id")
        .to_string();
    let endpoint = run["endpoint"]
        .as_str()
        .expect("the MicroVM carries an endpoint")
        .to_string();

    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, read) = call(
            router,
            "GET",
            &format!("/2025-09-09/microvms/{microvm_id}"),
            None,
        )
        .await;
        assert_eq!(status, 200, "read the MicroVM: got {status} {read}");
        if read["state"].as_str() == Some("RUNNING") {
            return (microvm_id, endpoint);
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the MicroVM {microvm_id} never reached RUNNING: last read {read}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Mint an authentication token for every port of `microvm_id`.
async fn auth_token(router: &MultiServiceRouter, microvm_id: &str) -> String {
    let (status, minted) = call(
        router,
        "POST",
        &format!("/2025-09-09/microvms/{microvm_id}/auth-token"),
        Some(json!({"expirationInMinutes": 15, "allowedPorts": [{"allPorts": {}}]})),
    )
    .await;
    assert_eq!(status, 200, "mint a token: got {status} {minted}");
    minted["authToken"][AUTH_TOKEN_HEADER]
        .as_str()
        .expect("the token carries the proxy header's value")
        .to_string()
}

/// One authenticated request to a MicroVM's published endpoint: the status and the body
/// the GUEST answered.
async fn through_endpoint(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    endpoint: &str,
    path: &str,
    token: &str,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("GET")
        .uri(format!("{endpoint}{path}"))
        .header(AUTH_TOKEN_HEADER, token)
        .body(Vec::new())
        .expect("a well-formed request to a MicroVM endpoint");
    let state = engine.state::<MicrovmState>();
    let connectors = connector_state(&**engine);
    let response =
        serve_request(&state, connectors.as_ref(), &callback(router), &request).await;
    let status = response.status().as_u16();
    let body = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, body)
}

/// Everything the suites below stand up once: the network, the connector, the staged
/// guest and the two MicroVMs it runs.
struct Standing {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    network: Network,
    /// The MicroVM attached to the ACTIVE VPC-egress connector.
    connected: (String, String),
    /// The MicroVM attached only to the AWS-managed internet-egress connector.
    internet_only: (String, String),
}

/// Stand that up.
async fn standing() -> Standing {
    let (router, engine) = emulator();
    let network = network(&router).await;
    ingress(&router, &network.target_group, ADMITTED_PORT, true).await;
    let connector = active_connector(&router, &network).await;
    stage_artifact(
        &router,
        "mvm-guest-artifacts",
        "guest.zip",
        guest_artifact(GUEST_SERVER),
    )
    .await;
    let image = guest_image(
        &router,
        "guest-serving",
        "s3://mvm-guest-artifacts/guest.zip",
    )
    .await;
    let connected = running_microvm(&router, &image, &[connector]).await;
    let internet_only = running_microvm(&router, &image, &[internet_egress_arn()]).await;
    Standing {
        router,
        engine,
        network,
        connected,
        internet_only,
    }
}

/// The body a `/connect` of `address`:`port` reported, through `microvm`.
async fn guest_connect(
    standing: &Standing,
    microvm: &(String, String),
    address: &str,
    port: i32,
) -> Value {
    let token = auth_token(&standing.router, &microvm.0).await;
    let (status, body) = through_endpoint(
        &standing.router,
        &standing.engine,
        &microvm.1,
        &format!("/connect?host={address}&port={port}"),
        &token,
    )
    .await;
    assert_eq!(
        status, 200,
        "GET /connect?host={address}&port={port} on {}: expected the guest's own answer, \
         got {status} {body}",
        microvm.0
    );
    body
}

/// One row of the guest-answer table: the case, the MicroVM asked, the guest path, and
/// the status and body the guest's own program must answer with.
type AnswerRow<'a> = (&'a str, &'a (String, String), &'a str, u16, Value);

/// RULE: an authenticated request to the published `endpoint` of a RUNNING MicroVM is
/// answered with the GUEST's own HTTP response — the status, the content type and the
/// body its program produced — for every path the guest serves and for every MicroVM
/// running that image.
#[tokio::test(flavor = "multi_thread")]
async fn a_published_endpoint_answers_with_the_guests_own_response() {
    let standing = standing().await;

    // The rows differ in WHICH MicroVM is asked and which of the guest's own paths is
    // asked for, including one the guest answers with a status of its own.
    let rows: Vec<AnswerRow> = vec![
        (
            "the health path of the connector-attached MicroVM",
            &standing.connected,
            "/health",
            200,
            json!({"healthy": true}),
        ),
        (
            "the health path of the internet-egress MicroVM",
            &standing.internet_only,
            "/health",
            200,
            json!({"healthy": true}),
        ),
        (
            "a path the guest answers with its own 404",
            &standing.connected,
            "/nothing-here",
            404,
            json!({"error": "no such path"}),
        ),
        (
            "the health path with a query the guest ignores",
            &standing.connected,
            "/health?probe=1",
            200,
            json!({"healthy": true}),
        ),
    ];

    for (case, microvm, path, status, expected) in rows {
        let token = auth_token(&standing.router, &microvm.0).await;
        let (actual_status, actual) = through_endpoint(
            &standing.router,
            &standing.engine,
            &microvm.1,
            path,
            &token,
        )
        .await;
        assert_eq!(
            actual_status, status,
            "case {case:?}: expected the guest's status {status}, got {actual_status} {actual}"
        );
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the guest's body {expected}, got {actual}"
        );
    }
}

/// One row of the reachability table: the case, the MicroVM asked, the destination, the
/// port, and whether the connection is carried.
type ConnectRow<'a> = (&'a str, &'a (String, String), String, i32, bool);

/// RULE: a MicroVM's outbound connection is carried only by an attached ACTIVE VPC-egress
/// connector whose VPC holds the destination and whose path the destination's security
/// groups admit on that port; every other destination is unreached. The guest observes
/// the carried one as established, with a LINK-LOCAL local address rather than one inside
/// the connector's subnet.
#[tokio::test(flavor = "multi_thread")]
async fn only_an_attached_connector_carries_a_guests_connection() {
    let standing = standing().await;
    let target = standing.network.target_address.clone();

    // (case, the MicroVM asked, the destination, the port, whether it connects)
    let rows: Vec<ConnectRow> = vec![
        (
            "the connector's own VPC, on the admitted port",
            &standing.connected,
            target.clone(),
            ADMITTED_PORT,
            true,
        ),
        (
            "the same destination on a port nothing admits",
            &standing.connected,
            target.clone(),
            9_999,
            false,
        ),
        (
            "an address of the connector's subnet nothing holds",
            &standing.connected,
            "10.90.1.200".to_string(),
            ADMITTED_PORT,
            false,
        ),
        (
            "an address outside the connector's VPC",
            &standing.connected,
            "10.99.1.4".to_string(),
            ADMITTED_PORT,
            false,
        ),
        (
            "a public address this account holds no resource at",
            &standing.connected,
            "203.0.113.10".to_string(),
            ADMITTED_PORT,
            false,
        ),
        (
            "a name rather than an address",
            &standing.connected,
            "endpoint.example.internal".to_string(),
            ADMITTED_PORT,
            false,
        ),
        (
            "the same destination from a MicroVM attached only to internet egress",
            &standing.internet_only,
            target.clone(),
            ADMITTED_PORT,
            false,
        ),
    ];

    for (case, microvm, address, port, connects) in rows {
        let body = guest_connect(&standing, microvm, &address, port).await;
        assert_eq!(
            body["connected"].as_bool(),
            Some(connects),
            "case {case:?}: expected connected={connects}, got {body}"
        );
        let expected_outcome = if connects { "connected" } else { "timed_out" };
        assert_eq!(
            body["outcome"].as_str(),
            Some(expected_outcome),
            "case {case:?}: expected outcome {expected_outcome:?}, got {body}"
        );
        assert_eq!(
            body["peer"],
            json!([address, port]),
            "case {case:?}: the guest reports the peer it was asked for, got {body}"
        );
        if !connects {
            assert!(
                body["local"].is_null(),
                "case {case:?}: an unreached connection has no local end, got {body}"
            );
            continue;
        }
        let local = body["local"][0]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a carried connection has a local end: {body}"))
            .to_string();
        let octets: Vec<u8> = local
            .split('.')
            .filter_map(|octet| octet.parse::<u8>().ok())
            .collect();
        assert_eq!(
            (octets.first().copied(), octets.get(1).copied()),
            (Some(LINK_LOCAL_PREFIX.0), Some(LINK_LOCAL_PREFIX.1)),
            "case {case:?}: expected a link-local local address, got {local:?}"
        );
        assert!(
            !local.starts_with("10.90."),
            "case {case:?}: the local end is never an address of the connector's subnet, \
             got {local:?}"
        );
        let port = body["local"][1].as_i64().unwrap_or_default();
        assert!(
            (49_152..=65_535).contains(&port),
            "case {case:?}: expected an ephemeral local port, got {port}"
        );
    }
}

/// RULE: the destination's security group GATES the connector's path, so the same
/// request turns from unreached to established and back as the ingress rule admitting
/// the port is authorized and revoked.
#[tokio::test(flavor = "multi_thread")]
async fn the_destinations_security_group_gates_the_connectors_path() {
    let standing = standing().await;
    let target = standing.network.target_address.clone();
    let group = standing.network.target_group.clone();

    // The rows are the sequence the rule is read over: the control, the revocation, the
    // re-authorization — each asserted on the SAME request.
    let rows: Vec<(&str, Option<bool>, bool)> = vec![
        ("the rule the suite admitted", None, true),
        ("the rule revoked", Some(false), false),
        ("the rule authorized again", Some(true), true),
    ];

    for (case, change, connects) in rows {
        if let Some(admit) = change {
            ingress(&standing.router, &group, ADMITTED_PORT, admit).await;
        }
        let body = guest_connect(&standing, &standing.connected, &target, ADMITTED_PORT).await;
        assert_eq!(
            body["connected"].as_bool(),
            Some(connects),
            "case {case:?}: expected connected={connects}, got {body}"
        );
    }
}
