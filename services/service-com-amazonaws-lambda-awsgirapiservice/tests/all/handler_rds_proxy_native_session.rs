// SPDX-License-Identifier: CC0-1.0

//! The RDS PROXY ENDPOINT dialled by the only client that can reach it: a `python3.x` function
//! attached to the proxy's own VPC, speaking the proxy's engine-family wire on the address the
//! control plane published.
//!
//! RULE: a data-plane address a control plane publishes in a member that carries NO PORT of
//! its own is a HOST the client completes with the port of the resource's own protocol — for a
//! DB proxy, the port of the databases its `EngineFamily` names — so this emulator answers that
//! port at the host it publishes, beside the port that member spells. What answers there is the
//! resource's own protocol handler over its own state: a handler that reads `DBProxy$Endpoint`
//! out of Amazon RDS and opens a connection to it logs in with the credentials the proxy's
//! `Auth` secret holds and runs statements against the database the proxy's target group holds,
//! under the encryption `RequireTLS` demands of connections to the proxy, while an address no
//! plane of this emulator answers at is answered the way a host answers one with nothing
//! listening there.
//!
//! A proxy endpoint is VPC-only, so this is the ONLY vantage point its data plane is observable
//! from: a proxy whose published address no in-process client can open a session on has no
//! observable data plane at all, and every behaviour that lives on the proxy's wire — the
//! credentials it admits, the refusal it words, the database it pools connections to — is
//! unreachable.
//!
//! Three crates have to agree for that to hold — Amazon EC2 owns the VPC, its subnets and its
//! security group, Amazon RDS opens the database's and the proxy's listeners and publishes their
//! addresses, and Lambda runs the handler that dials them — so all three are mounted here on ONE
//! engine, exactly as the assembled router mounts them.
//!
//! The rows differ along what the rule generalises over: WHICH address is dialled (the proxy's
//! published endpoint, a second proxy's, the target database's own endpoint, a port of this
//! machine no plane answers at, and the DNS-form endpoint of a proxy whose engine family this
//! emulator serves no wire for), WHICH port completes it (the engine family's own, and the one
//! the published authority spells), whether the session is in the clear or inside a TLS session
//! negotiated in band, whether the proxy requires that encryption, whether the login names a
//! database, and whether the credential presented is the one the proxy's secret holds or
//! another.
//!
//! The handler is pure standard library — `hashlib`, `socket`, `ssl` and `struct` — and speaks
//! the MySQL protocol itself, so what these rows drive is what a stock driver speaks rather than
//! an in-process shortcut past the protocol.
//!
//! The suite RUNS a handler, so it takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]).

use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;

use crate::function_lifecycle::{create_function, json_call, make_activation_due, Answer, ROLE};
use crate::handler_rds_catalogue_calls::{element, rds};
use crate::handler_secret_relayed_to_queue::secrets;
use crate::invoke_request_id::{is_host_bound, one_invocation_stream_at_a_time};
use crate::node_invoke_run::package;
use crate::vpc_attachment_interfaces::{ec2_client, network, Network};

/// The second octet of the VPC the database, the proxy and the function all sit in.
const PREFIX: u8 = 61;

/// The Availability Zone the second subnet of that VPC lies in, so the subnet group the database
/// and the proxy are placed in covers two.
const SECOND_ZONE: &str = "us-east-1b";

/// The database resources every row acts on, and the credential the database was created with —
/// the same pair the proxy's `Auth` secret holds.
const SUBNET_GROUP: &str = "proxy-session-subnets";
const INSTANCE: &str = "proxy-session-instance";
const MASTER_USER: &str = "appadmin";
const MASTER_PASSWORD: &str = "initial-Password-1";
const DATABASE: &str = "appdb";

/// The secret the proxies' `Auth` entry names as the credential they log in with.
const SECRET_NAME: &str = "proxy-session-credential";

/// The proxies every row dials: one that admits a session in the clear, one that requires TLS of
/// connections to it, and one of an engine family this emulator speaks no wire for.
const PLAIN_PROXY: &str = "proxy-session-plain";
const TLS_PROXY: &str = "proxy-session-tls";
const SQLSERVER_PROXY: &str = "proxy-session-sqlserver";

/// An IAM role ARN of the form `CreateDBProxy$RoleArn` takes: the role a proxy reads its
/// credentials with.
const PROXY_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/rds-proxy";

/// The function that carries the probe.
const FUNCTION: &str = "rds-proxy-session-probe";

/// How long one invocation is given, as the function's own `Timeout`: well past what one MySQL
/// exchange needs on any host, because what is under test is which address the handler reaches
/// and not how fast an interpreter gets through one.
const FUNCTION_TIMEOUT: u64 = 60;

/// The memory the probe is configured for. No row tests a memory limit, so the bound is generous:
/// an interpreter standing up on a loaded host must not decide the outcome. 3008 MiB is one of
/// the sizes a function is ordinarily configured with, inside `MemorySize`'s 128..=10240 range.
const FUNCTION_MEMORY: u64 = 3_008;

/// How long a wait for work a service's own clock owes is given, and how often to look. Every
/// wait is for the CONDITION, and the deadline is generous so a loaded host cannot decide the
/// outcome.
const CLOCK_BUDGET: Duration = Duration::from_secs(120);
/// See [`CLOCK_BUDGET`].
const LOOK_AGAIN: Duration = Duration::from_millis(25);

/// How many times an invocation is re-driven while the answer reports one of this test binary's
/// own host bounds rather than a run.
const HOST_BOUND_ATTEMPTS: usize = 6;

/// A port of this machine no plane of this emulator answers at, which is what the row for an
/// address with nothing behind it dials. The core hands its listeners ephemeral ports, so this
/// one is outside that range.
const UNSERVED_PORT: u64 = 1;

/// The port a client dials a MySQL-family proxy on.
///
/// `DBProxy` carries no port member at all, so a client completes the host `Endpoint` publishes
/// with the port of the databases the proxy's `EngineFamily` names. That port is 3306 for the
/// MySQL family, as the one place those defaults are published states: "Default: … RDS for
/// MariaDB — 3306, … RDS for MySQL — 3306"
/// (`smithy_query(query="doc",shape="CreateDBInstanceMessage",member="Port")`).
const MYSQL_FAMILY_PORT: u64 = 3306;

/// The statement every admitted session runs, and what each of its three values must be: the
/// literal the statement selects, the version the engine reports for itself, and the account the
/// session was admitted as.
const SESSION_STATEMENT: &str = "SELECT 1, VERSION(), CURRENT_USER()";

/// The session statements a stock driver issues ITSELF, before the caller's first one: the
/// character set every answer is decoded as, and the autocommit mode it manages transactions
/// under.
///
/// A driver's connect is not complete until the server has answered both, so a session that
/// refuses one is a session no driver opens — which is why every row sends them where a driver
/// would, rather than only the statement the caller wrote.
const CLIENT_PREAMBLE: &[&str] = &[
    "SET NAMES utf8mb4 COLLATE utf8mb4_general_ci",
    "SET AUTOCOMMIT = 0",
];

/// The probe this suite runs: it dials the address the event names, speaks the MySQL protocol
/// itself — the greeting, the `mysql_native_password` proof, then one `COM_QUERY` per statement —
/// and reports what the server answered, including the refusal it was answered with instead of a
/// session.
/// `pub(crate)` because the suite over a session a client authenticates to the proxy with IAM
/// (`tests/all/handler_rds_proxy_iam_auth_session.rs`) opens its sessions with this same
/// handler rather than carrying a second spelling of the MySQL protocol: what differs there is
/// the CREDENTIAL presented, which that suite mints and hands this handler as the password.
pub(crate) const PROBE_SOURCE: &str = r#"
import hashlib
import socket
import ssl
import struct

# The capability flags this client sets, as a stock driver sets them.
CLIENT_CONNECT_WITH_DB = 0x00000008
CLIENT_PROTOCOL_41 = 0x00000200
CLIENT_SSL = 0x00000800
CLIENT_SECURE_CONNECTION = 0x00008000
CLIENT_PLUGIN_AUTH = 0x00080000
CLIENT_DEPRECATE_EOF = 0x01000000

# How long the handler waits on its own connection. Generous: what is under test is which address
# it reaches, never how fast a host answers, and the invocation's own Timeout bounds every wait.
WAIT = 45

# The largest packet this client announces it can carry.
MAX_PACKET = 16 * 1024 * 1024


class Refused(Exception):
    """An ERR packet the server answered: its error number and its message."""

    def __init__(self, number, message):
        Exception.__init__(self, message)
        self.number = number
        self.message = message


def _read_exactly(conn, size):
    held = b""
    while len(held) < size:
        block = conn.recv(size - len(held))
        if not block:
            raise EOFError("the server closed the connection")
        held += block
    return held


def _read_packet(conn):
    header = _read_exactly(conn, 4)
    length = header[0] | (header[1] << 8) | (header[2] << 16)
    return header[3], _read_exactly(conn, length)


def _write_packet(conn, sequence, payload):
    conn.sendall(struct.pack("<I", len(payload))[:3] + bytes([sequence]) + payload)


def _cstring(payload, at):
    end = payload.find(b"\x00", at)
    if end < 0:
        end = len(payload)
    return payload[at:end].decode("utf-8", "replace"), end + 1


def _length_encoded(payload, at):
    first = payload[at]
    if first < 0xFB:
        return first, at + 1
    if first == 0xFC:
        return payload[at + 1] | (payload[at + 2] << 8), at + 3
    if first == 0xFD:
        return (
            payload[at + 1] | (payload[at + 2] << 8) | (payload[at + 3] << 16),
            at + 4,
        )
    if first == 0xFE:
        return struct.unpack("<Q", payload[at + 1 : at + 9])[0], at + 9
    raise ValueError("not a length-encoded value: %s" % first)


def _length_encoded_string(payload, at):
    length, at = _length_encoded(payload, at)
    return payload[at : at + length].decode("utf-8", "replace"), at + length


def _refusal(payload):
    number = payload[1] | (payload[2] << 8)
    # `#` then the five-character SQLSTATE, then the message.
    return Refused(number, payload[9:].decode("utf-8", "replace"))


def _native_proof(password, nonce):
    """SHA1(password) XOR SHA1(nonce || SHA1(SHA1(password))) — the mysql_native_password proof."""
    first = hashlib.sha1(password.encode("utf-8")).digest()
    stored = hashlib.sha1(first).digest()
    mask = hashlib.sha1(nonce + stored).digest()
    return bytes(left ^ right for left, right in zip(first, mask))


def _login(conn, host, user, password, database, tls):
    _sequence, greeting = _read_packet(conn)
    if greeting and greeting[0] == 0xFF:
        raise _refusal(greeting)
    if not greeting or greeting[0] != 10:
        raise ValueError("the greeting must announce protocol version 10")
    version, at = _cstring(greeting, 1)
    # The connection id (4), the first 8 nonce bytes, a filler, the lower capability flags, the
    # charset, the status flags, the upper capability flags, the nonce length, 10 reserved bytes,
    # then the remaining nonce bytes.
    nonce = greeting[at + 4 : at + 12] + greeting[at + 31 : at + 43]
    offered = (greeting[at + 13] | (greeting[at + 14] << 8)) | (
        (greeting[at + 19] | (greeting[at + 20] << 8)) << 16
    )
    capabilities = (
        CLIENT_PROTOCOL_41
        | CLIENT_SECURE_CONNECTION
        | CLIENT_PLUGIN_AUTH
        | CLIENT_DEPRECATE_EOF
    )
    if database:
        capabilities = capabilities | CLIENT_CONNECT_WITH_DB
    sequence = 1
    if tls:
        capabilities = capabilities | CLIENT_SSL
        # The 32-byte handshake response that asks for TLS and nothing else.
        _write_packet(
            conn,
            sequence,
            struct.pack("<I", capabilities)
            + struct.pack("<I", MAX_PACKET)
            + bytes([255])
            + bytes(23),
        )
        conn = ssl.create_default_context().wrap_socket(conn, server_hostname=host)
        sequence = 2
    response = (
        struct.pack("<I", capabilities)
        + struct.pack("<I", MAX_PACKET)
        + bytes([255])
        + bytes(23)
        + user.encode("utf-8")
        + b"\x00"
    )
    proof = _native_proof(password, nonce)
    response = response + bytes([len(proof)]) + proof
    if database:
        response = response + database.encode("utf-8") + b"\x00"
    response = response + b"mysql_native_password\x00"
    _write_packet(conn, sequence, response)
    sequence, verdict = _read_packet(conn)
    if verdict and verdict[0] == 0xFE:
        # An AuthSwitchRequest: the cleartext plugin sends the credential as it stands.
        plugin, _at = _cstring(verdict, 1)
        if plugin != "mysql_clear_password":
            raise ValueError("unexpected authentication plugin %s" % plugin)
        _write_packet(conn, sequence + 1, password.encode("utf-8") + b"\x00")
        _sequence, verdict = _read_packet(conn)
    if verdict and verdict[0] == 0xFF:
        raise _refusal(verdict)
    if not verdict or verdict[0] != 0x00:
        raise ValueError("unexpected answer to the handshake response")
    return conn, version, offered


def _query(conn, statement):
    _write_packet(conn, 0, b"\x03" + statement.encode("utf-8"))
    _sequence, first = _read_packet(conn)
    if first[0] == 0x00:
        return {"columns": [], "rows": []}
    if first[0] == 0xFF:
        raise _refusal(first)
    count, _at = _length_encoded(first, 0)
    columns = []
    for _each in range(count):
        _sequence, definition = _read_packet(conn)
        at = 0
        name = ""
        for field in range(6):
            value, at = _length_encoded_string(definition, at)
            if field == 4:
                name = value
        columns.append(name)
    rows = []
    while True:
        _sequence, packet = _read_packet(conn)
        # The packet that ends a result set: an EOF marker, never a row, which always begins with
        # a length-encoded value.
        if packet[0] == 0xFE and len(packet) < 9:
            break
        if packet[0] == 0xFF:
            raise _refusal(packet)
        row = []
        at = 0
        while at < len(packet):
            if packet[at] == 0xFB:
                row.append(None)
                at = at + 1
                continue
            value, at = _length_encoded_string(packet, at)
            row.append(value)
        rows.append(row)
    return {"columns": columns, "rows": rows}


def _answers(conn, statements):
    """What the server answered each statement: its result, or the refusal it carried."""
    answered = []
    for statement in statements:
        try:
            answered.append(_query(conn, statement))
        except Refused as refusal:
            answered.append({"code": refusal.number, "message": refusal.message})
    return answered


def handler(event, context):
    try:
        opened = socket.create_connection((event["host"], event["port"]), WAIT)
    except OSError as refusal:
        return {"ok": False, "stage": "connect", "refused": str(refusal)}
    conn = opened
    try:
        opened.settimeout(WAIT)
        try:
            conn, version, offered = _login(
                opened,
                event["host"],
                event["user"],
                event["password"],
                event.get("database"),
                event.get("tls") or False,
            )
        except Refused as refusal:
            return {
                "ok": False,
                "stage": "login",
                "code": refusal.number,
                "message": refusal.message,
            }
        return {
            "ok": True,
            "server_version": version,
            "offered_tls": bool(offered & CLIENT_SSL),
            "tls_version": conn.version() if event.get("tls") else None,
            "preamble": _answers(conn, event.get("preamble") or []),
            "statements": _answers(conn, event["statements"]),
        }
    finally:
        opened.close()
"#;

/// Lambda, Amazon EC2, Amazon RDS and Secrets Manager over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the proxy the handler dials is the one this suite
/// created, and the connection it opens reaches that proxy's own protocol handler.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    ec2: aws_sdk_ec2::Client,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Pinned off so the suite does not depend on how the emulator process was started; what is
    // under test is the session the handler opens, not the emulator's SigV4 check.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_rds_amazonrdsv19::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    let callback = EmbeddedEmulator::from_router(router.clone());
    Emulator {
        ec2: ec2_client(&callback),
        engine,
        router,
    }
}

/// One Amazon RDS call that must be served, as its XML body.
async fn rds_ok(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let (status, body) = rds(router, action, params).await;
    assert_eq!(
        status, 200,
        "setup {case:?}: expected {action} to be served, got {status} with {body}"
    );
    body
}

/// Read one Amazon RDS answer again until `holds`, which is how a change the CLOCK owes is waited
/// for: the condition, under a generous deadline, never a fixed span.
async fn rds_until(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(&str, &str)],
    holds: impl Fn(&str) -> bool,
) -> String {
    let deadline = Instant::now() + CLOCK_BUDGET;
    loop {
        let body = rds_ok(router, case, action, params).await;
        if holds(&body) {
            return body;
        }
        assert!(
            Instant::now() < deadline,
            "setup {case:?}: {action} never reported the condition, last answer {body}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// One more subnet of the VPC, in another Availability Zone.
///
/// A database and a proxy are placed in a subnet group that "doesn't meet Availability Zone (AZ)
/// coverage requirement" with one zone alone, and the shared network helper mints every subnet it
/// is asked for in the Region's first zone, so the second zone is asked for here.
async fn second_zone_subnet(ec2: &aws_sdk_ec2::Client, case: &str, vpc_id: &str) -> String {
    let created = ec2
        .create_subnet()
        .vpc_id(vpc_id)
        .cidr_block(format!("10.{PREFIX}.9.0/24"))
        .availability_zone(SECOND_ZONE)
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("case {case:?}: EC2 must create the second subnet: {error:?}")
        });
    created
        .subnet()
        .and_then(aws_sdk_ec2::types::Subnet::subnet_id)
        .unwrap_or_else(|| panic!("case {case:?}: CreateSubnet must report the subnet it minted"))
        .to_string()
}

/// The DB instance the proxies pool connections to, in the VPC the function is attached to, and
/// the `host:port` Amazon RDS published for its own endpoint.
async fn plant_the_database(router: &MultiServiceRouter, network: &Network) -> (String, u16) {
    let mut subnets: Vec<(&str, &str)> = vec![
        ("DBSubnetGroupName", SUBNET_GROUP),
        ("DBSubnetGroupDescription", "the database's own subnets"),
    ];
    let subnet_members: Vec<(String, &str)> = network
        .subnet_ids
        .iter()
        .enumerate()
        .map(|(at, subnet)| {
            (
                format!("SubnetIds.SubnetIdentifier.{}", at + 1),
                subnet.as_str(),
            )
        })
        .collect();
    for (name, value) in &subnet_members {
        subnets.push((name.as_str(), value));
    }
    rds_ok(router, "the subnet group", "CreateDBSubnetGroup", &subnets).await;
    rds_ok(
        router,
        "the database",
        "CreateDBInstance",
        &[
            ("DBInstanceIdentifier", INSTANCE),
            ("Engine", "mysql"),
            ("DBInstanceClass", "db.t3.micro"),
            ("AllocatedStorage", "20"),
            ("MasterUsername", MASTER_USER),
            ("MasterUserPassword", MASTER_PASSWORD),
            ("DBName", DATABASE),
            ("DBSubnetGroupName", SUBNET_GROUP),
            ("PubliclyAccessible", "false"),
            (
                "VpcSecurityGroupIds.VpcSecurityGroupId.1",
                network.group_id.as_str(),
            ),
        ],
    )
    .await;
    let described = rds_until(
        router,
        "the database",
        "DescribeDBInstances",
        &[("DBInstanceIdentifier", INSTANCE)],
        |body| element(body, "DBInstanceStatus") == "available",
    )
    .await;
    let address = element(&described, "Address");
    let port: u16 = element(&described, "Port")
        .parse()
        .unwrap_or_else(|_| panic!("setup: the instance must publish a numeric port: {described}"));
    assert!(
        !address.is_empty(),
        "setup: the instance must publish an endpoint address, got {described}"
    );
    (address, port)
}

/// The secret the proxies' `Auth` entry names, holding the database's own credential — the pair a
/// proxy "uses the credentials in the secret" to log in with.
async fn plant_the_secret(router: &MultiServiceRouter) -> String {
    let created = secrets(
        router,
        "the proxy's credential",
        "CreateSecret",
        &json!({
            "Name": SECRET_NAME,
            // "You must provide a ClientRequestToken value. We recommend a UUID-type value" —
            // the idempotency token Secrets Manager requires of a create made without one
            // generated by an SDK.
            "ClientRequestToken": "0f4b8a0e-3d2c-4f1a-9b6e-7c5d2a1e8f30",
            "SecretString": json!({
                "username": MASTER_USER,
                "password": MASTER_PASSWORD,
            })
            .to_string(),
        }),
    )
    .await;
    created
        .get("ARN")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("setup: CreateSecret must report the ARN it minted: {created}"))
        .to_string()
}

/// The credential the secret holds, read back through Secrets Manager's own operation: the pair a
/// session through the proxy is admitted with comes from the secret the proxy names, never from
/// what this suite wrote.
async fn credential_in_the_secret(router: &MultiServiceRouter) -> (String, String) {
    let read = secrets(
        router,
        "the proxy's credential",
        "GetSecretValue",
        &json!({ "SecretId": SECRET_NAME }),
    )
    .await;
    let held: Value = read
        .get("SecretString")
        .and_then(Value::as_str)
        .and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or_else(|| panic!("setup: the secret must hold its credential: {read}"));
    (
        held["username"].as_str().unwrap_or_default().to_string(),
        held["password"].as_str().unwrap_or_default().to_string(),
    )
}

/// `CreateDBProxy` of `engine_family` in the VPC, with the `Auth` entry naming `secret_arn`, and
/// the wait for the status at which "the proxy is ready to handle requests". Answers the
/// `Endpoint` it published.
async fn plant_the_proxy(
    router: &MultiServiceRouter,
    network: &Network,
    name: &str,
    engine_family: &str,
    require_tls: bool,
    secret_arn: &str,
) -> String {
    let subnet_members: Vec<(String, &str)> = network
        .subnet_ids
        .iter()
        .enumerate()
        .map(|(at, subnet)| (format!("VpcSubnetIds.member.{}", at + 1), subnet.as_str()))
        .collect();
    let mut params: Vec<(&str, &str)> = vec![
        ("DBProxyName", name),
        ("EngineFamily", engine_family),
        ("RoleArn", PROXY_ROLE_ARN),
        ("RequireTLS", if require_tls { "true" } else { "false" }),
        ("VpcSecurityGroupIds.member.1", network.group_id.as_str()),
        ("Auth.member.1.AuthScheme", "SECRETS"),
        ("Auth.member.1.SecretArn", secret_arn),
        ("Auth.member.1.IAMAuth", "DISABLED"),
    ];
    for (member, value) in &subnet_members {
        params.push((member.as_str(), value));
    }
    let created = rds_ok(router, name, "CreateDBProxy", &params).await;
    let published = element(&created, "Endpoint");
    assert!(
        !published.is_empty(),
        "setup {name:?}: CreateDBProxy must publish an endpoint, got {created}"
    );
    let described = rds_until(
        router,
        name,
        "DescribeDBProxies",
        &[("DBProxyName", name)],
        |body| element(body, "Status") == "available",
    )
    .await;
    assert_eq!(
        element(&described, "Endpoint"),
        published,
        "setup {name:?}: the endpoint the create published must be the one the describe reports, \
         got {described}"
    );
    published
}

/// `RegisterDBProxyTargets` for the database on `proxy`, and the wait for the health at which the
/// proxy pools connections to it.
async fn register_the_target(router: &MultiServiceRouter, proxy: &str) {
    rds_ok(
        router,
        proxy,
        "RegisterDBProxyTargets",
        &[
            ("DBProxyName", proxy),
            ("DBInstanceIdentifiers.member.1", INSTANCE),
        ],
    )
    .await;
    rds_until(
        router,
        proxy,
        "DescribeDBProxyTargets",
        &[("DBProxyName", proxy)],
        |body| element(body, "State") == "AVAILABLE",
    )
    .await;
}

/// The `host` and `port` of a published `host:port` authority, as a client reads them off the
/// string the control plane handed it.
fn dialable(case: &str, published: &str) -> (String, u16) {
    let (host, port) = published.rsplit_once(':').unwrap_or_else(|| {
        panic!("case {case:?}: expected a published host:port authority, got {published:?}")
    });
    (
        host.to_string(),
        port.parse().unwrap_or_else(|_| {
            panic!("case {case:?}: expected a numeric port in {published:?}")
        }),
    )
}

/// The body of an answer as text, for a message that says what went wrong.
fn text(answer: &Answer) -> String {
    String::from_utf8_lossy(&answer.body).to_string()
}

/// Create the probe function, attached to the VPC the database and the proxies sit in, and make
/// the activation the create owes due.
async fn plant_the_function(emulator: &Emulator, network: &Network) {
    let created = create_function(
        &emulator.router,
        &json!({
            "FunctionName": FUNCTION,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Timeout": FUNCTION_TIMEOUT,
            "MemorySize": FUNCTION_MEMORY,
            "VpcConfig": {
                "SubnetIds": network.subnet_ids,
                "SecurityGroupIds": [network.group_id],
            },
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(&[("index.py", PROBE_SOURCE)])),
            },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "setup: CreateFunction must answer 201, got {} {}",
        created.status,
        text(&created)
    );
    // The activation the create owes is the CLOCK's, so it is brought forward rather than waited
    // out.
    make_activation_due(&emulator.engine);
}

/// Invoke the probe with one row's event and answer the report the handler returned.
async fn probe(router: &MultiServiceRouter, case: &str, event: &Value) -> Value {
    let uri = format!("/2015-03-31/functions/{FUNCTION}/invocations");
    let mut answer = json_call(router, "POST", &uri, event).await;
    for _ in 1..HOST_BOUND_ATTEMPTS {
        if !is_host_bound(answer.status, &text(&answer)) {
            break;
        }
        answer = json_call(router, "POST", &uri, event).await;
    }
    assert!(
        !is_host_bound(answer.status, &text(&answer)),
        "case {case:?}: every attempt reported a bound this whole test binary shares, so nothing \
         of the rule was read: {}",
        text(&answer)
    );
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: the invocation must be served, got {} {}",
        answer.status,
        text(&answer)
    );
    let raised = answer.header("x-amz-function-error");
    assert_eq!(
        raised, None,
        "case {case:?}: the handler must run to completion, got {raised:?} with {}",
        text(&answer)
    );
    answer.json()
}

/// One row of the session table: the address dialled, the credential presented, and what the
/// server must answer.
struct Row {
    /// What the case is called, in the event and in every assertion.
    case: &'static str,
    /// Which published address the row dials.
    at: Dialled,
    /// Whether the row dials the port the engine FAMILY is dialled on — the port a client
    /// completes a connection string with, because the member it read carries none — rather
    /// than the one the published authority spells.
    at_family_port: bool,
    /// Whether the client negotiates TLS in band before it presents its credential.
    tls: bool,
    /// Whether the login names the proxy's database.
    with_database: bool,
    /// The password presented in place of the one the secret holds, where the row presents
    /// another; `None` presents the secret's own.
    password: Option<&'static str>,
    /// The user presented in place of the secret's own, where the row presents another.
    user: Option<&'static str>,
    /// What the row expects of the session.
    expect: Expected,
}

/// Which published address a row dials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dialled {
    /// The endpoint of the proxy that admits a session in the clear.
    PlainProxy,
    /// The endpoint of the proxy that requires TLS of connections to it.
    TlsProxy,
    /// The target database's OWN endpoint, which the proxy pools connections to.
    Instance,
    /// A port of this machine no plane of this emulator answers at.
    Unserved,
}

/// What a row expects of the session it opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The session is admitted and the statement answers its three values.
    Admitted,
    /// The login is refused with this MySQL error number, and a message naming `user`.
    AccessDenied,
    /// The login is refused by the PROXY, with the sentence the proxy words.
    ProxyTlsRequired,
    /// Nothing answers at the address, so no connection is opened at all.
    NothingAnswers,
}

/// MySQL's error number for a refused login: `ER_ACCESS_DENIED_ERROR`.
///
/// Source: the number the engines' own refusal carries
/// (`service_com_amazonaws_rds_amazonrdsv19::state::data_plane::wire_session`'s
/// `LoginRefusal::AuthenticationFailed`), which is the server-side code a client reads for a
/// credential the server does not admit.
const ACCESS_DENIED: u64 = 1045;

/// The rows: every combination of the address dialled, the encryption asked for and the
/// credential presented that the rule generalises over.
const ROWS: &[Row] = &[
    Row {
        case: "the proxy's published host, on the port its engine family is dialled on",
        at: Dialled::PlainProxy,
        at_family_port: true,
        tls: false,
        with_database: false,
        password: None,
        user: None,
        expect: Expected::Admitted,
    },
    Row {
        case: "the secret's user with another password, on the family port",
        at: Dialled::PlainProxy,
        at_family_port: true,
        tls: false,
        with_database: false,
        password: Some("another-Password-2"),
        user: None,
        expect: Expected::AccessDenied,
    },
    Row {
        case: "the proxy's published endpoint, in the clear, as the secret's user",
        at: Dialled::PlainProxy,
        at_family_port: false,
        tls: false,
        with_database: false,
        password: None,
        user: None,
        expect: Expected::Admitted,
    },
    Row {
        case: "the same session, naming the database the proxy's target holds",
        at: Dialled::PlainProxy,
        at_family_port: false,
        tls: false,
        with_database: true,
        password: None,
        user: None,
        expect: Expected::Admitted,
    },
    Row {
        case: "the same endpoint under a TLS session negotiated in band",
        at: Dialled::PlainProxy,
        at_family_port: false,
        tls: true,
        with_database: false,
        password: None,
        user: None,
        expect: Expected::Admitted,
    },
    Row {
        case: "the secret's user with another password",
        at: Dialled::PlainProxy,
        at_family_port: false,
        tls: false,
        with_database: false,
        password: Some("another-Password-2"),
        user: None,
        expect: Expected::AccessDenied,
    },
    Row {
        case: "a user the database behind the proxy does not hold",
        at: Dialled::PlainProxy,
        at_family_port: false,
        tls: false,
        with_database: false,
        password: None,
        user: Some("nobody"),
        expect: Expected::AccessDenied,
    },
    Row {
        case: "a proxy that requires TLS, dialled in the clear",
        at: Dialled::TlsProxy,
        at_family_port: false,
        tls: false,
        with_database: false,
        password: None,
        user: None,
        expect: Expected::ProxyTlsRequired,
    },
    Row {
        case: "the same proxy under a TLS session negotiated in band",
        at: Dialled::TlsProxy,
        at_family_port: false,
        tls: true,
        with_database: false,
        password: None,
        user: None,
        expect: Expected::Admitted,
    },
    Row {
        case: "the target database's own endpoint, which the proxy pools connections to",
        at: Dialled::Instance,
        at_family_port: false,
        tls: false,
        with_database: true,
        password: None,
        user: None,
        expect: Expected::Admitted,
    },
    Row {
        case: "a port of this machine no plane of this emulator answers at",
        at: Dialled::Unserved,
        at_family_port: false,
        tls: false,
        with_database: false,
        password: None,
        user: None,
        expect: Expected::NothingAnswers,
    },
];

/// RULE, end to end: the address Amazon RDS publishes for a DB proxy is one a function in that
/// proxy's VPC opens a MySQL session on, served against the database the proxy's target group
/// holds.
#[tokio::test]
async fn an_in_vpc_function_runs_statements_through_the_published_proxy_endpoint() {
    let _stream = one_invocation_stream_at_a_time().await;
    let emulator = emulator();
    let case = "the network the database, the proxy and the function share";
    let mut network = network(&emulator.ec2, case, PREFIX, 1).await;
    let second = second_zone_subnet(&emulator.ec2, case, &network.vpc_id).await;
    network.subnet_ids.push(second);
    let (instance_host, instance_port) = plant_the_database(&emulator.router, &network).await;
    let secret_arn = plant_the_secret(&emulator.router).await;
    // The proxy that requires TLS is created FIRST and the one that admits a session in the
    // clear SECOND, because the port an engine family is dialled on is one port of the one
    // interface this emulator is reached on: it names no proxy in particular, so the proxy that
    // published its endpoint last is the one answering there, and the rows that dial that port
    // are the rows about that proxy.
    let requires_tls = plant_the_proxy(
        &emulator.router,
        &network,
        TLS_PROXY,
        "MYSQL",
        true,
        &secret_arn,
    )
    .await;
    let plain = plant_the_proxy(
        &emulator.router,
        &network,
        PLAIN_PROXY,
        "MYSQL",
        false,
        &secret_arn,
    )
    .await;
    register_the_target(&emulator.router, TLS_PROXY).await;
    register_the_target(&emulator.router, PLAIN_PROXY).await;
    plant_the_function(&emulator, &network).await;
    let (user, password) = credential_in_the_secret(&emulator.router).await;
    assert_eq!(
        (user.as_str(), password.as_str()),
        (MASTER_USER, MASTER_PASSWORD),
        "setup: the secret must hold the database's own credential"
    );

    let (plain_host, plain_port) = dialable(PLAIN_PROXY, &plain);
    let (tls_host, tls_port) = dialable(TLS_PROXY, &requires_tls);
    assert_ne!(
        (plain_host.as_str(), plain_port),
        (instance_host.as_str(), instance_port),
        "setup: a proxy's endpoint is its own address, not the database's"
    );
    // Each proxy publishes an authority of its own: the port the core assigned its listener is
    // what tells two proxies of one engine family apart there, and neither of them is the
    // family's own port, which a client completes the published host with.
    assert_ne!(
        (plain_host.as_str(), plain_port),
        (tls_host.as_str(), tls_port),
        "setup: two proxies must publish different authorities, got {plain:?} and \
         {requires_tls:?}"
    );
    for (proxy, port) in [(PLAIN_PROXY, plain_port), (TLS_PROXY, tls_port)] {
        assert_ne!(
            u64::from(port),
            MYSQL_FAMILY_PORT,
            "setup: {proxy:?} publishes the port the core assigned its listener, not the \
             family's own"
        );
    }

    for row in ROWS {
        // WHICH pair the row dials: the published host, with the port the published authority
        // spells or the one the engine family is dialled on — the two ways a client completes
        // a connection string from a member that carries no port of its own.
        let (host, published) = match row.at {
            Dialled::PlainProxy => (plain_host.clone(), u64::from(plain_port)),
            Dialled::TlsProxy => (tls_host.clone(), u64::from(tls_port)),
            Dialled::Instance => (instance_host.clone(), u64::from(instance_port)),
            Dialled::Unserved => (plain_host.clone(), UNSERVED_PORT),
        };
        let port = if row.at_family_port {
            MYSQL_FAMILY_PORT
        } else {
            published
        };
        let case = row.case;
        let report = probe(
            &emulator.router,
            case,
            &json!({
                "host": host,
                "port": port,
                "user": row.user.unwrap_or(user.as_str()),
                "password": row.password.unwrap_or(password.as_str()),
                "database": if row.with_database { Some(DATABASE) } else { None },
                "tls": row.tls,
                "preamble": CLIENT_PREAMBLE,
                "statements": [SESSION_STATEMENT],
            }),
        )
        .await;
        match row.expect {
            Expected::Admitted => {
                assert_eq!(
                    report["ok"], json!(true),
                    "case {case:?}: expected the session to be admitted, got {report}"
                );
                let preamble = report["preamble"].as_array().cloned().unwrap_or_default();
                assert_eq!(
                    preamble.len(),
                    CLIENT_PREAMBLE.len(),
                    "case {case:?}: expected an answer to each statement a driver issues on\
                     connect, got {report}"
                );
                for (statement, answered) in CLIENT_PREAMBLE.iter().zip(&preamble) {
                    assert_eq!(
                        answered.get("code"),
                        None,
                        "case {case:?}: the session must answer {statement:?}, which a driver \
                         issues before the caller's first statement, got {answered}"
                    );
                }
                let rows = report
                    .pointer("/statements/0/rows")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                assert_eq!(
                    rows.len(),
                    1,
                    "case {case:?}: expected one row from {SESSION_STATEMENT}, got {report}"
                );
                let values = rows[0].as_array().cloned().unwrap_or_default();
                assert_eq!(
                    values.first().cloned().unwrap_or(Value::Null),
                    json!("1"),
                    "case {case:?}: expected the literal the statement selects, got {report}"
                );
                let version = values
                    .get(1)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                assert_eq!(
                    version,
                    report["server_version"].as_str().unwrap_or_default(),
                    "case {case:?}: the version the engine answers must be the one its greeting \
                     named, got {report}"
                );
                assert!(
                    !version.is_empty(),
                    "case {case:?}: expected the engine's own version string, got {report}"
                );
                let account = values
                    .get(2)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                assert!(
                    account.starts_with(&format!("{user}@")),
                    "case {case:?}: expected the session to be the secret's user {user:?}, got \
                     {account:?} in {report}"
                );
                if row.tls {
                    let negotiated = report["tls_version"].as_str().unwrap_or_default();
                    assert!(
                        negotiated.starts_with("TLS"),
                        "case {case:?}: expected a negotiated TLS session, got {report}"
                    );
                }
            }
            Expected::AccessDenied => {
                assert_eq!(
                    (report["ok"].clone(), report["stage"].clone()),
                    (json!(false), json!("login")),
                    "case {case:?}: expected the login to be refused by the server, got {report}"
                );
                assert_eq!(
                    report["code"], json!(ACCESS_DENIED),
                    "case {case:?}: expected MySQL's access-denied number, got {report}"
                );
                let named = row.user.unwrap_or(user.as_str());
                let message = report["message"].as_str().unwrap_or_default();
                assert!(
                    message.contains(named),
                    "case {case:?}: the refusal must name the user it refused ({named:?}), got \
                     {report}"
                );
            }
            Expected::ProxyTlsRequired => {
                assert_eq!(
                    (report["ok"].clone(), report["stage"].clone()),
                    (json!(false), json!("login")),
                    "case {case:?}: expected the proxy to refuse the session, got {report}"
                );
                let message = report["message"].as_str().unwrap_or_default();
                assert_eq!(
                    message,
                    service_com_amazonaws_rds_amazonrdsv19::state::PROXY_REQUIRES_TLS_MESSAGE,
                    "case {case:?}: expected the sentence the proxy words a plaintext session \
                     with, got {report}"
                );
            }
            Expected::NothingAnswers => {
                assert_eq!(
                    (report["ok"].clone(), report["stage"].clone()),
                    (json!(false), json!("connect")),
                    "case {case:?}: expected no connection to be opened at all, got {report}"
                );
            }
        }
    }

    // What a session through the proxy WRITES is the target's own storage: the row one session
    // inserts through the proxy's endpoint is read back through the database's own endpoint, so
    // the proxy pooled its connection to the registered target rather than to storage of its own.
    let case = "a write through the proxy, read back through the database's own endpoint";
    let written = probe(
        &emulator.router,
        case,
        &json!({
            "host": plain_host,
            "port": u64::from(plain_port),
            "user": user,
            "password": password,
            "database": DATABASE,
            "tls": false,
            "preamble": CLIENT_PREAMBLE,
            "statements": [
                "CREATE TABLE through_the_proxy (id INT PRIMARY KEY, note VARCHAR(32))",
                "INSERT INTO through_the_proxy (id, note) VALUES (1, 'pooled')",
                "COMMIT",
                "SELECT note FROM through_the_proxy WHERE id = 1",
            ],
        }),
    )
    .await;
    assert_eq!(
        written["ok"], json!(true),
        "case {case:?}: expected the writing session to be admitted, got {written}"
    );
    assert_eq!(
        written.pointer("/statements/3/rows"),
        Some(&json!([["pooled"]])),
        "case {case:?}: expected the proxy's session to read its own write back, got {written}"
    );
    let read_back = probe(
        &emulator.router,
        case,
        &json!({
            "host": instance_host,
            "port": u64::from(instance_port),
            "user": user,
            "password": password,
            "database": DATABASE,
            "tls": false,
            "preamble": CLIENT_PREAMBLE,
            "statements": ["SELECT note FROM through_the_proxy WHERE id = 1"],
        }),
    )
    .await;
    assert_eq!(
        read_back.pointer("/statements/0/rows"),
        Some(&json!([["pooled"]])),
        "case {case:?}: expected the database's own endpoint to hold the row the proxy's session \
         wrote, got {read_back}"
    );

    // A proxy of an engine family this emulator speaks no wire for publishes the DNS-form endpoint
    // instead, and a port that answered nothing would be worse than a name: nothing of this
    // emulator answers there, which is what the function reads.
    let case = "the DNS-form endpoint of a proxy whose engine family this emulator serves no wire \
                for";
    let published = plant_the_proxy(
        &emulator.router,
        &network,
        SQLSERVER_PROXY,
        "SQLSERVER",
        false,
        &secret_arn,
    )
    .await;
    assert!(
        published.ends_with(".rds.amazonaws.com") && !published.contains(':'),
        "case {case:?}: expected the DNS-form endpoint, got {published:?}"
    );
    let report = probe(
        &emulator.router,
        case,
        &json!({
            "host": published,
            "port": 3306,
            "user": user,
            "password": password,
            "database": Value::Null,
            "tls": false,
            "preamble": CLIENT_PREAMBLE,
            "statements": [SESSION_STATEMENT],
        }),
    )
    .await;
    assert_eq!(
        (report["ok"].clone(), report["stage"].clone()),
        (json!(false), json!("connect")),
        "case {case:?}: expected no connection to be opened at all, got {report}"
    );
}
