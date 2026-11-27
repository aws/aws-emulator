// SPDX-License-Identifier: CC0-1.0

//! The INSTANCE ENDPOINT Amazon DocumentDB publishes, dialled by the only client that can
//! reach it: a Lambda function attached to the cluster's own VPC.
//!
//! RULE: a data-plane address this emulator publishes is one an in-process client of this
//! emulator reaches the resource at — never a name in a service's public DNS zone — so the
//! `Endpoint{Address,Port}` `DescribeDBInstances` reports for a member of a document cluster is
//! an address of this emulator's own, and what answers there is that cluster's own document
//! server over that cluster's own storage. A `python3.x` handler that reads the pair out of the
//! control plane, opens a socket to it, completes the handshake and the SASL exchange as the
//! cluster's master user and then writes and reads documents is served; an address this
//! emulator opened no plane at is answered the way a host with nothing listening there answers
//! it. Amazon DocumentDB is reachable from inside its cluster's VPC alone, so this is the ONLY
//! vantage point its data plane is observable from: a cluster whose published address no
//! in-process client can open a connection to has no observable data plane at all.
//!
//! Three crates have to agree for that to hold — EC2 owns the VPC, the subnets and the security
//! group, DocumentDB opens each member's socket and publishes its address, and Lambda runs the
//! handler that dials it — so the suite lives here. The crate's own suite
//! (`services/service-com-amazonaws-docdb-amazonrdsv19/tests/all/instance_data_plane.rs`) pins
//! the same rule at the socket; this one pins it from the vantage point a client of AWS has.
//!
//! The rows differ along what the rule generalises over: which ROLE of its cluster the member
//! whose address is dialled holds (the one the cluster writes through, a replica), whether the
//! wire is encrypted (the cluster's `tls` parameter), what the session asks of it (a handshake,
//! the SASL exchange, writes of documents of differing shape, reads under differing filters),
//! and an address this emulator opened no plane at.
//!
//! What the handler reaches is decided by the environment it runs in, not by this suite: the
//! addresses a run may open a connection to are the ones a service of this emulator opened a
//! plane at, and every other address is answered the way a host answers one without anything
//! being dialled. The last rows assert exactly that, pinned to the NUMBER the environment
//! answers each with — a port of this machine nothing was opened at is refused, an address of
//! another host has no route — so a row cannot be satisfied by a connection that went
//! somewhere and failed late.

use std::collections::BTreeMap;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{block_of, element, elements, form_encode, opt_element, physical_ids_of};

/// The Region every request here is signed for.
const REGION: &str = "us-west-2";

/// The EC2 API version an ec2Query form body names.
const EC2_API_VERSION: &str = "2016-11-15";

/// The API version every awsQuery view of the Amazon RDS front door declares, which a
/// DocumentDB form body names.
const DOCDB_API_VERSION: &str = "2014-10-31";

/// The network the cluster and the function both sit in. A DB subnet group must cover at least
/// two Availability Zones, so the network carries a subnet in each.
const VPC_CIDR: &str = "10.46.0.0/16";
const SUBNET_CIDRS: [(&str, &str); 2] = [
    ("10.46.1.0/24", "us-west-2a"),
    ("10.46.2.0/24", "us-west-2b"),
];
const SECURITY_GROUP: &str = "xsvc-docdb-invpc-group";

/// The port a client of this engine is admitted on, which is the `Port` a cluster created
/// without one reports (`CreateDBClusterMessage$Port`, the engine's own default) and the port
/// the inbound rule admitting the cluster's clients names.
const CLIENT_PORT: &str = "27017";

/// The engine the clusters of this suite run (`CreateDBClusterMessage$Engine`: "Valid values:
/// docdb") and the class their members are created with.
const ENGINE: &str = "docdb";
const INSTANCE_CLASS: &str = "db.t3.medium";

/// The DB subnet group both clusters are placed in, over the network's own subnets.
const SUBNET_GROUP: &str = "xsvc-docdb-invpc-subnets";

/// The cluster whose endpoints are encrypted — the `tls` cluster parameter's own default — and
/// its two members: the one the cluster writes through, and a replica.
const SECURED_CLUSTER: &str = "xsvc-docdb-invpc-secured";
const WRITER: &str = "xsvc-docdb-invpc-writer";
const REPLICA: &str = "xsvc-docdb-invpc-replica";

/// The cluster whose own parameter group turns that parameter off, so its endpoint is served in
/// the clear, and its single member.
const PLAINTEXT_CLUSTER: &str = "xsvc-docdb-invpc-plaintext";
const PLAINTEXT_GROUP: &str = "xsvc-docdb-invpc-tls-off";
const PLAINTEXT_MEMBER: &str = "xsvc-docdb-invpc-plain-member";

/// The parameter-group family the clusters of this suite run in, and the parameter that decides
/// whether an endpoint is encrypted together with the value that turns it off.
const PARAMETER_GROUP_FAMILY: &str = "docdb5.0";
const TLS_PARAMETER: &str = "tls";
const TLS_DISABLED: &str = "disabled";

/// The master user both clusters are created with, which every session authenticates as.
const MASTER_USERNAME: &str = "dbadmin";
/// Its password, which only this suite and the credential derived from it ever see.
const MASTER_PASSWORD: &str = "a-Secret-99";

/// The database and the collection every row reads and writes.
const DATABASE: &str = "shop";
const COLLECTION: &str = "widgets";

/// The stack, the execution role and the function that carry the probe.
const STACK: &str = "xsvc-docdb-invpc-probe";
const EXEC_ROLE: &str = "xsvc-docdb-invpc-exec";
const FUNCTION: &str = "xsvc-docdb-invpc-probe-fn";

/// The `StackStatus` the probe stack must reach.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The `DBInstanceStatus` a member that serves connections is in.
const AVAILABLE: &str = "available";

/// How long one invocation is given, as the function's own `Timeout`: well past what a run
/// needs on any host, because what is under test is what the handler can reach and not how fast
/// an interpreter gets through one.
const FUNCTION_TIMEOUT: u64 = 120;

/// The memory the probe function is configured for. No row here tests a memory limit, so the
/// limit is given generous room: an interpreter standing up on a loaded host must not be the
/// thing that decides the outcome.
const FUNCTION_MEMORY: u64 = 3_008;

/// How often to look while waiting for work a service's own clock owes — a member reaching
/// `available`, a stack completing, a function reaching `Active` — and how many looks to take.
/// Nothing here sleeps for a fixed span: every wait is for the CONDITION, and the deadline is
/// generous so a loaded host cannot decide the outcome.
const LOOK: Duration = Duration::from_millis(50);
/// See [`LOOK`].
const LOOKS: usize = 2_400;

/// A port of this machine no plane of this emulator answers at, which the row for an address
/// with nothing behind it dials. The core hands its listeners ephemeral ports, so this one is
/// outside that range.
const UNSERVED_PORT: u64 = 1;

/// An address of another host, which no plane of this emulator is open at and which nothing of
/// this suite is reachable from: a documentation-range address (RFC 5737 TEST-NET-2), so no
/// machine answers at it anywhere.
const OFF_EMULATOR_ADDRESS: &str = "198.51.100.7";

/// What a handler is answered when it addresses a port of THIS machine no plane of this
/// emulator was opened at, as a program reads it off `errno`: `ECONNREFUSED`, the number a host
/// answers a refused connection with.
const CONNECTION_REFUSED: i64 = 111;

/// What it is answered for an address of ANOTHER host, which is not an address of this machine
/// and which no plane of this emulator is open at either: `ENETUNREACH`, the number a host
/// answers an address it has no route to with. The environment the handler runs in decides this
/// from the addresses it holds, so nothing is dialled for such a row — which is why it is
/// answered at once rather than after a wait.
const NETWORK_UNREACHABLE: i64 = 101;

/// The refusal code this engine answers a write addressed to a member that is not the one its
/// cluster writes through with (`NotWritablePrimary`, as the engine's clients read it).
const NOT_WRITABLE_PRIMARY: i64 = 10107;

/// The handler every row runs.
///
/// Pure standard library: `socket` opens the connection to the address the event names, `ssl`
/// secures it where the event asks for it, and `hashlib`, `hmac`, `base64` and `struct` compute
/// the SASL proof and frame the engine's own messages — so what a row asserts is what the
/// endpoint really answered on the wire.
const PROBE_HANDLER: &str = r#"
import base64
import hashlib
import hmac
import socket
import ssl
import struct

# The message every command of a driver travels in, and the header that precedes every
# message's body.
OP_MSG = 2013
HEADER = 16

# How long the handler waits on its own connection. Generous: what is under test is which
# address it reaches, never how fast a host answers, and the invocation's own Timeout bounds
# every wait besides.
WAIT = 60

# The nonce this client contributes to the SASL exchange. The server contributes one of its
# own, which is what makes the combined nonce of each exchange its own.
CLIENT_NONCE = "aW4tdnBjLXByb2JlLW5vbmNl"

# The mechanism this client authenticates with, of the two the handshake offers.
MECHANISM = "SCRAM-SHA-256"


def _read_exactly(stream, count):
    held = b""
    while len(held) < count:
        block = stream.recv(count - len(held))
        if not block:
            raise OSError("the endpoint closed the connection")
        held += block
    return held


def _encode_value(value):
    if isinstance(value, bool):
        return 0x08, b"\x01" if value else b"\x00"
    if isinstance(value, int):
        if -2147483648 <= value <= 2147483647:
            return 0x10, struct.pack("<i", value)
        return 0x12, struct.pack("<q", value)
    if isinstance(value, float):
        return 0x01, struct.pack("<d", value)
    if isinstance(value, str):
        encoded = value.encode("utf-8") + b"\x00"
        return 0x02, struct.pack("<i", len(encoded)) + encoded
    if isinstance(value, bytes):
        return 0x05, struct.pack("<i", len(value)) + b"\x00" + value
    if isinstance(value, dict):
        return 0x03, _encode_document(value)
    if isinstance(value, list):
        held = {}
        for at, item in enumerate(value):
            held[str(at)] = item
        return 0x04, _encode_document(held)
    if value is None:
        return 0x0A, b""
    raise TypeError("this client encodes no value of type " + type(value).__name__)


def _encode_document(document):
    body = b""
    for name, value in document.items():
        kind, encoded = _encode_value(value)
        body += bytes([kind]) + name.encode("utf-8") + b"\x00" + encoded
    return struct.pack("<i", len(body) + 5) + body + b"\x00"


def _decode_value(kind, data, at):
    if kind == 0x01:
        return struct.unpack_from("<d", data, at)[0], at + 8
    if kind == 0x02:
        length = struct.unpack_from("<i", data, at)[0]
        return data[at + 4 : at + 3 + length].decode("utf-8"), at + 4 + length
    if kind == 0x03:
        return _decode_document(data, at)
    if kind == 0x04:
        held, at = _decode_document(data, at)
        return [held[name] for name in sorted(held, key=int)], at
    if kind == 0x05:
        length = struct.unpack_from("<i", data, at)[0]
        return data[at + 5 : at + 5 + length], at + 5 + length
    if kind == 0x07:
        return data[at : at + 12].hex(), at + 12
    if kind == 0x08:
        return data[at] != 0, at + 1
    if kind == 0x0A:
        return None, at
    if kind in (0x09, 0x11, 0x12):
        return struct.unpack_from("<q", data, at)[0], at + 8
    if kind == 0x10:
        return struct.unpack_from("<i", data, at)[0], at + 4
    raise ValueError("this client decodes no element of type %d" % kind)


def _decode_document(data, at):
    length = struct.unpack_from("<i", data, at)[0]
    end = at + length - 1
    at += 4
    document = {}
    while at < end:
        kind = data[at]
        at += 1
        stop = data.index(b"\x00", at)
        name = data[at:stop].decode("utf-8")
        at = stop + 1
        document[name], at = _decode_value(kind, data, at)
    return document, end + 1


def _plain(value):
    """The value as JSON carries it: the bytes of a payload as their hex, and everything
    else as it came off the wire."""
    if isinstance(value, bytes):
        return value.hex()
    if isinstance(value, dict):
        held = {}
        for name, item in value.items():
            held[name] = _plain(item)
        return held
    if isinstance(value, list):
        return [_plain(item) for item in value]
    return value


def _command(stream, state, database, command):
    body = dict(command)
    body["$db"] = database
    section = struct.pack("<i", 0) + b"\x00" + _encode_document(body)
    request_id = state["request_id"]
    state["request_id"] = request_id + 1
    stream.sendall(
        struct.pack("<iiii", HEADER + len(section), request_id, 0, OP_MSG) + section
    )
    length, _own_id, response_to, opcode = struct.unpack(
        "<iiii", _read_exactly(stream, HEADER)
    )
    rest = _read_exactly(stream, length - HEADER)
    if opcode != OP_MSG:
        raise OSError("the endpoint answered opcode %d" % opcode)
    if response_to != request_id:
        raise OSError("the answer names request %d" % response_to)
    # The body of a reply: its flags, the kind of its single section, then the document.
    reply, _end = _decode_document(rest, 5)
    return reply


def _authenticate(stream, state, username, password):
    first_bare = "n=%s,r=%s" % (username, CLIENT_NONCE)
    started = _command(
        stream,
        state,
        "admin",
        {
            "saslStart": 1,
            "mechanism": MECHANISM,
            "payload": ("n,," + first_bare).encode("utf-8"),
            "options": {"skipEmptyExchange": True},
        },
    )
    if not isinstance(started.get("payload"), bytes):
        return {"started": _plain(started)}
    server_first = started["payload"].decode("utf-8")
    fields = {}
    for field in server_first.split(","):
        name, _is, value = field.partition("=")
        fields[name] = value
    salt = base64.b64decode(fields["s"])
    iterations = int(fields["i"])
    salted = hashlib.pbkdf2_hmac("sha256", password.encode("utf-8"), salt, iterations)
    client_key = hmac.new(salted, b"Client Key", "sha256").digest()
    stored_key = hashlib.sha256(client_key).digest()
    server_key = hmac.new(salted, b"Server Key", "sha256").digest()
    without_proof = "c=biws,r=" + fields["r"]
    auth_message = "%s,%s,%s" % (first_bare, server_first, without_proof)
    signature = hmac.new(stored_key, auth_message.encode("utf-8"), "sha256").digest()
    proof = bytes(left ^ right for left, right in zip(client_key, signature))
    final = "%s,p=%s" % (
        without_proof,
        base64.b64encode(proof).decode("ascii"),
    )
    continued = _command(
        stream,
        state,
        "admin",
        {
            "saslContinue": 1,
            "conversationId": started.get("conversationId"),
            "payload": final.encode("utf-8"),
        },
    )
    answered = {
        "mechanism": MECHANISM,
        "iterations": iterations,
        "nonce_extends_the_clients": fields["r"].startswith(CLIENT_NONCE),
        "started": _plain(started),
        "continued": _plain(continued),
    }
    # The server proves it holds the same credential: its signature over the same message,
    # computed from the key only a holder of the password can derive.
    if isinstance(continued.get("payload"), bytes):
        expected = base64.b64encode(
            hmac.new(server_key, auth_message.encode("utf-8"), "sha256").digest()
        ).decode("ascii")
        answered["server_signature_verified"] = continued["payload"].decode(
            "utf-8"
        ) == ("v=" + expected)
    return answered


def handler(event, context):
    try:
        opened = socket.create_connection((event["host"], event["port"]), WAIT)
    except OSError as refusal:
        # The number the environment answers this address with is reported beside the text,
        # because WHICH refusal it is is what a client branches on.
        return {"ok": False, "refused": str(refusal), "errno": refusal.errno}
    try:
        opened.settimeout(WAIT)
        stream = opened
        answered = {"ok": True}
        if event.get("tls"):
            secure = ssl.create_default_context()
            stream = secure.wrap_socket(opened, server_hostname=event["host"])
            answered["tls_version"] = stream.version()
        state = {"request_id": 1}
        answered["hello"] = _plain(
            _command(
                stream,
                state,
                "admin",
                {
                    "hello": 1,
                    "helloOk": True,
                    "saslSupportedMechs": "admin." + event["username"],
                },
            )
        )
        answered["auth"] = _authenticate(
            stream, state, event["username"], event["password"]
        )
        written = []
        for document in event.get("documents") or []:
            written.append(
                _plain(
                    _command(
                        stream,
                        state,
                        event["database"],
                        {"insert": event["collection"], "documents": [document]},
                    )
                )
            )
        answered["inserts"] = written
        read = {}
        for selection in event.get("finds") or []:
            read[selection["name"]] = _plain(
                _command(
                    stream,
                    state,
                    event["database"],
                    {"find": event["collection"], "filter": selection["filter"]},
                )
            )
        answered["finds"] = read
        return answered
    finally:
        opened.close()
"#;

/// The emulator as it is ASSEMBLED, on one engine: the member the handler dials is the one this
/// suite created, and the socket it opens is a connection to that member's own server.
fn build_router() -> MultiServiceRouter {
    let engine = std::sync::Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    // The signature is never checked here, as in every other suite of this crate; the
    // credential scope is what routes a request.
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what routes
/// the request.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// One form-encoded call of `service`, answered with the HTTP status and the XML body.
async fn form_call(
    router: &MultiServiceRouter,
    service: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(service))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// One EC2 action that must succeed, answered with its XML body.
async fn ec2(router: &MultiServiceRouter, action: &str, members: &[(&str, &str)]) -> String {
    let (status, body) = form_call(router, "ec2", EC2_API_VERSION, action, members).await;
    assert_eq!(
        status, 200,
        "setup: EC2 {action} must succeed, got {status} with {body}"
    );
    body
}

/// One Amazon DocumentDB action that must succeed, answered with its XML body.
async fn docdb(router: &MultiServiceRouter, action: &str, members: &[(&str, &str)]) -> String {
    let (status, body) = form_call(router, "docdb", DOCDB_API_VERSION, action, members).await;
    assert_eq!(
        status, 200,
        "setup: DocumentDB {action} must succeed, got {status} with {body}"
    );
    body
}

/// One CloudFormation action.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    form_call(router, "cloudformation", "2010-05-15", action, members).await
}

/// One Lambda `restJson1` call, as its status, its headers and its parsed body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
) -> (u16, http::HeaderMap, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("lambda"))
        .body(Bytes::from(body.to_vec()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    let parsed = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).to_string()))
    };
    (parts.status.as_u16(), parts.headers, parsed)
}

/// The network the cluster and the function share, minted by EC2 itself.
struct Network {
    vpc: String,
    subnets: Vec<String>,
    group: String,
}

/// Create that network through EC2's own operations.
async fn ec2_network(router: &MultiServiceRouter) -> Network {
    let vpc = element(
        &ec2(router, "CreateVpc", &[("CidrBlock", VPC_CIDR)]).await,
        "vpcId",
    );
    let mut subnets = Vec::new();
    for (cidr, zone) in SUBNET_CIDRS {
        subnets.push(element(
            &ec2(
                router,
                "CreateSubnet",
                &[
                    ("VpcId", vpc.as_str()),
                    ("CidrBlock", cidr),
                    ("AvailabilityZone", zone),
                ],
            )
            .await,
            "subnetId",
        ));
    }
    let group = element(
        &ec2(
            router,
            "CreateSecurityGroup",
            &[
                ("VpcId", vpc.as_str()),
                ("GroupName", SECURITY_GROUP),
                (
                    "GroupDescription",
                    "the group of the cluster and its clients",
                ),
            ],
        )
        .await,
        "groupId",
    );
    // The inbound rule that admits the cluster's clients: a cluster placed in the account's VPC
    // is reached through an interface this group governs, and the source is the group ITSELF —
    // the group the function's own interface carries too — which is how a client admits its own
    // workloads to its cluster.
    ec2(
        router,
        "AuthorizeSecurityGroupIngress",
        &[
            ("GroupId", group.as_str()),
            ("IpPermissions.1.IpProtocol", "tcp"),
            ("IpPermissions.1.FromPort", CLIENT_PORT),
            ("IpPermissions.1.ToPort", CLIENT_PORT),
            ("IpPermissions.1.Groups.1.GroupId", group.as_str()),
        ],
    )
    .await;
    Network {
        vpc,
        subnets,
        group,
    }
}

/// Create the subnet group, the two clusters and their members in that network, and answer the
/// address each member publishes once DocumentDB reports it `available`.
async fn plant_the_clusters(
    router: &MultiServiceRouter,
    network: &Network,
) -> BTreeMap<String, (String, u64)> {
    let mut subnet_members: Vec<(String, &str)> = Vec::new();
    for (at, subnet) in network.subnets.iter().enumerate() {
        subnet_members.push((
            format!("SubnetIds.SubnetIdentifier.{}", at + 1),
            subnet.as_str(),
        ));
    }
    let mut group_members: Vec<(&str, &str)> = vec![
        ("DBSubnetGroupName", SUBNET_GROUP),
        ("DBSubnetGroupDescription", "the cluster's own subnets"),
    ];
    for (key, value) in &subnet_members {
        group_members.push((key.as_str(), value));
    }
    docdb(router, "CreateDBSubnetGroup", &group_members).await;

    // The parameter group that turns this engine's transport encryption off, which is what makes
    // one of the two clusters' endpoints a cleartext wire.
    docdb(
        router,
        "CreateDBClusterParameterGroup",
        &[
            ("DBClusterParameterGroupName", PLAINTEXT_GROUP),
            ("DBParameterGroupFamily", PARAMETER_GROUP_FAMILY),
            ("Description", "a group that serves its endpoints in the clear"),
        ],
    )
    .await;
    docdb(
        router,
        "ModifyDBClusterParameterGroup",
        &[
            ("DBClusterParameterGroupName", PLAINTEXT_GROUP),
            ("Parameters.Parameter.1.ParameterName", TLS_PARAMETER),
            ("Parameters.Parameter.1.ParameterValue", TLS_DISABLED),
            ("Parameters.Parameter.1.ApplyMethod", "pending-reboot"),
        ],
    )
    .await;

    for (cluster, parameter_group) in [
        (SECURED_CLUSTER, None),
        (PLAINTEXT_CLUSTER, Some(PLAINTEXT_GROUP)),
    ] {
        let mut members: Vec<(&str, &str)> = vec![
            ("DBClusterIdentifier", cluster),
            ("Engine", ENGINE),
            ("MasterUsername", MASTER_USERNAME),
            ("MasterUserPassword", MASTER_PASSWORD),
            ("DBSubnetGroupName", SUBNET_GROUP),
            (
                "VpcSecurityGroupIds.VpcSecurityGroupId.1",
                network.group.as_str(),
            ),
        ];
        if let Some(group) = parameter_group {
            members.push(("DBClusterParameterGroupName", group));
        }
        docdb(router, "CreateDBCluster", &members).await;
    }

    // The members: the secured cluster's first member is the one it writes through and its
    // second is a replica, and the cleartext cluster has one of its own.
    for (instance, cluster) in [
        (WRITER, SECURED_CLUSTER),
        (REPLICA, SECURED_CLUSTER),
        (PLAINTEXT_MEMBER, PLAINTEXT_CLUSTER),
    ] {
        docdb(
            router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", instance),
                ("DBInstanceClass", INSTANCE_CLASS),
                ("Engine", ENGINE),
                ("DBClusterIdentifier", cluster),
            ],
        )
        .await;
    }

    let mut published = BTreeMap::new();
    for instance in [WRITER, REPLICA, PLAINTEXT_MEMBER] {
        published.insert(
            instance.to_string(),
            published_endpoint(router, instance).await,
        );
    }
    published
}

/// The `Endpoint{Address,Port}` the member `instance` publishes, read through
/// `DescribeDBInstances` exactly as a client reads it once the member is `available`.
async fn published_endpoint(router: &MultiServiceRouter, instance: &str) -> (String, u64) {
    let mut last = String::new();
    for _ in 0..LOOKS {
        let body = docdb(
            router,
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", instance)],
        )
        .await;
        if opt_element(&body, "DBInstanceStatus").as_deref() == Some(AVAILABLE) {
            let block = block_of(&body, "Endpoint").unwrap_or_else(|| {
                panic!("setup: {instance} must publish an Endpoint, got {body}")
            });
            let address = element(block, "Address");
            let port = element(block, "Port").parse::<u64>().unwrap_or_else(|_| {
                panic!("setup: {instance} must publish a numeric port, got {block}")
            });
            return (address, port);
        }
        last = body;
        tokio::time::sleep(LOOK).await;
    }
    panic!("setup: {instance} must reach {AVAILABLE}, last reported {last}")
}

/// The template the suite deploys: the execution role, and the probe function ATTACHED to the
/// cluster's own VPC. The inline source is archived into the deployment package by
/// CloudFormation, which is how a function whose handler is written here comes to exist.
fn stack_template(network: &Network) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "ExecRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": EXEC_ROLE,
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": {"Service": "lambda.amazonaws.com"},
                            "Action": "sts:AssumeRole",
                        }],
                    },
                    "Policies": [{
                        "PolicyName": "probe-grants",
                        "PolicyDocument": {
                            "Version": "2012-10-17",
                            "Statement": [{
                                "Effect": "Allow",
                                "Action": [
                                    "logs:CreateLogGroup",
                                    "logs:CreateLogStream",
                                    "logs:PutLogEvents",
                                    "ec2:CreateNetworkInterface",
                                    "ec2:DescribeNetworkInterfaces",
                                    "ec2:DeleteNetworkInterface",
                                ],
                                "Resource": "*",
                            }],
                        },
                    }],
                },
            },
            "Probe": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": FUNCTION,
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Role": {"Fn::GetAtt": ["ExecRole", "Arn"]},
                    "Timeout": FUNCTION_TIMEOUT,
                    "MemorySize": FUNCTION_MEMORY,
                    "VpcConfig": {
                        "SubnetIds": network.subnets,
                        "SecurityGroupIds": [network.group],
                    },
                    "Code": {"ZipFile": PROBE_HANDLER},
                },
            },
        },
    })
    .to_string()
}

/// Deploy the stack, wait for `CREATE_COMPLETE` and answer the physical id of each resource.
async fn deploy(router: &MultiServiceRouter, network: &Network) -> BTreeMap<String, String> {
    let template = stack_template(network);
    let (status, answered) = cloudformation(
        router,
        "CreateStack",
        &[
            ("StackName", STACK),
            ("TemplateBody", &template),
            ("Capabilities.member.1", "CAPABILITY_NAMED_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateStack({STACK:?}) must be accepted, got {status} with {answered}"
    );
    let mut reported = String::new();
    let mut described = String::new();
    for _ in 0..LOOKS {
        let (status, body) =
            cloudformation(router, "DescribeStacks", &[("StackName", STACK)]).await;
        assert_eq!(
            status, 200,
            "setup: DescribeStacks({STACK:?}) must succeed, got {status} with {body}"
        );
        reported = opt_element(&body, "StackStatus").unwrap_or_default();
        described = body;
        if !reported.ends_with("_IN_PROGRESS") {
            break;
        }
        tokio::time::sleep(LOOK).await;
    }
    let failures = if reported == CREATE_COMPLETE {
        Vec::new()
    } else {
        let (_, events) =
            cloudformation(router, "DescribeStackEvents", &[("StackName", STACK)]).await;
        elements(&events, "ResourceStatusReason")
    };
    assert_eq!(
        reported, CREATE_COMPLETE,
        "setup: the stack must reach {CREATE_COMPLETE}, got {described} with resource failures \
         {failures:?}"
    );
    let (status, resources) =
        cloudformation(router, "DescribeStackResources", &[("StackName", STACK)]).await;
    assert_eq!(
        status, 200,
        "setup: DescribeStackResources({STACK:?}) must succeed, got {status} with {resources}"
    );
    physical_ids_of(&resources)
}

/// Wait until Lambda reports the probe function `Active`, and answer its configuration.
async fn wait_until_active(router: &MultiServiceRouter) -> Value {
    let mut last = Value::Null;
    for _ in 0..LOOKS {
        let (_, _, configuration) = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{FUNCTION}/configuration"),
            &[],
        )
        .await;
        if configuration["State"].as_str() == Some("Active") {
            return configuration;
        }
        last = configuration;
        tokio::time::sleep(LOOK).await;
    }
    panic!("setup: Lambda must report {FUNCTION:?} Active, last reported {last}")
}

/// Invoke the probe with one row's event, and answer the report the handler returned.
async fn probe(router: &MultiServiceRouter, case: &str, event: &Value) -> Value {
    let (status, headers, payload) = lambda(
        router,
        "POST",
        &format!("/2015-03-31/functions/{FUNCTION}/invocations"),
        event.to_string().as_bytes(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the invocation must be served, got {status} with {payload}"
    );
    let raised = headers
        .get("x-amz-function-error")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    assert_eq!(
        raised, None,
        "case {case:?}: the handler module must import and run to completion, got {raised:?} \
         with {payload}"
    );
    payload
}

/// One row of the dial table: whose published address the probe dials, and what the session on
/// it must be answered.
struct Dialled {
    case: &'static str,
    /// The member whose `Endpoint` the handler is handed.
    instance: &'static str,
    /// Whether the wire is encrypted, which is what the member's cluster's `tls` parameter says.
    encrypted: bool,
    /// Whether this member is the one its cluster writes through.
    writer: bool,
    /// The documents the session writes, differing in their fields, their types and whether
    /// they carry an `_id` of their own.
    documents: Value,
    /// What the session reads back: the name of the selection, its filter, and the `name` of
    /// every document it must select, in the order a find answers them.
    finds: Vec<(&'static str, Value, Vec<&'static str>)>,
}

/// The `cursor.firstBatch` of a find reply.
fn batch_of(reply: &Value) -> Vec<Value> {
    reply
        .pointer("/cursor/firstBatch")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

#[tokio::test]
async fn an_in_vpc_function_runs_a_document_session_on_the_address_docdb_published() {
    let router = build_router();
    let network = ec2_network(&router).await;
    let published = plant_the_clusters(&router, &network).await;

    // What is published is an address of THIS emulator, not a name in a service's public DNS
    // zone: a client that resolves such a name inside the emulated environment resolves nothing,
    // so a name under one is not an endpoint at all. And each member is reached at an address of
    // its own, because each has a socket of its own.
    for (instance, (address, port)) in &published {
        assert!(
            !address.ends_with("amazonaws.com"),
            "case {instance:?}: the published address must be one this emulator serves, not a \
             name in AWS's own DNS namespace, got {address:?}"
        );
        assert!(
            *port > 0,
            "case {instance:?}: the published port must be the one the socket was assigned, got \
             {port}"
        );
    }
    let distinct: std::collections::BTreeSet<&(String, u64)> = published.values().collect();
    assert_eq!(
        distinct.len(),
        published.len(),
        "each member must publish an address of its own, got {published:?}"
    );

    let physical = deploy(&router, &network).await;
    assert_eq!(
        physical.get("Probe").map(String::as_str),
        Some(FUNCTION),
        "setup: the stack must report the probe function as {FUNCTION:?}, got {physical:?}"
    );
    let configuration = wait_until_active(&router).await;
    // The function really is attached to the cluster's own VPC: the id reported for it is the
    // one EC2 holds for the subnets it was placed in, not a value the request carried.
    assert_eq!(
        configuration.pointer("/VpcConfig/VpcId").cloned(),
        Some(json!(network.vpc)),
        "setup: the probe must be attached to {:?}, got {configuration}",
        network.vpc
    );

    let rows = vec![
        Dialled {
            case: "the member its cluster writes through, on an encrypted endpoint",
            instance: WRITER,
            encrypted: true,
            writer: true,
            documents: json!([
                {"_id": 1, "name": "left", "stock": 3},
                {"name": "right", "stock": 7},
                {"_id": "third", "name": "nested", "tags": ["a", "b"], "spec": {"size": 2}},
            ]),
            finds: vec![
                (
                    "every document",
                    json!({}),
                    vec!["left", "right", "nested"],
                ),
                (
                    "the documents a filter selects",
                    json!({"stock": {"$gt": 5}}),
                    vec!["right"],
                ),
            ],
        },
        Dialled {
            case: "a replica of the same cluster, on an encrypted endpoint",
            instance: REPLICA,
            encrypted: true,
            writer: false,
            documents: json!([{"_id": 99, "name": "refused"}]),
            finds: vec![(
                "every document the writer wrote",
                json!({}),
                vec!["left", "right", "nested"],
            )],
        },
        Dialled {
            case: "the member of a cluster whose endpoints are served in the clear",
            instance: PLAINTEXT_MEMBER,
            encrypted: false,
            writer: true,
            documents: json!([{"_id": "only", "name": "single"}]),
            finds: vec![(
                "the document it wrote",
                json!({"name": "single"}),
                vec!["single"],
            )],
        },
    ];

    for row in &rows {
        let case = row.case;
        let (address, port) = published
            .get(row.instance)
            .unwrap_or_else(|| panic!("case {case:?}: {} published an address", row.instance));
        let report = probe(
            &router,
            case,
            &json!({
                "host": address,
                "port": port,
                "tls": row.encrypted,
                "username": MASTER_USERNAME,
                "password": MASTER_PASSWORD,
                "database": DATABASE,
                "collection": COLLECTION,
                "documents": row.documents,
                "finds": row.finds
                    .iter()
                    .map(|(name, filter, _)| json!({"name": name, "filter": filter}))
                    .collect::<Vec<Value>>(),
            }),
        )
        .await;
        assert_eq!(
            report.get("ok").and_then(Value::as_bool),
            Some(true),
            "case {case:?}: the published address {address}:{port} must be reachable from inside \
             the cluster's own VPC, got {report}"
        );
        // The wire is the one the cluster's parameter says it is: an encrypted endpoint is
        // reached through a handshake the session reports the version of, and a cleartext one
        // negotiates nothing.
        let negotiated = report.get("tls_version").and_then(Value::as_str);
        match row.encrypted {
            true => assert!(
                negotiated.is_some_and(|version| version.starts_with("TLS")),
                "case {case:?}: expected the session to run inside the handshake the endpoint \
                 terminates, got {negotiated:?} in {report}"
            ),
            false => assert_eq!(
                negotiated, None,
                "case {case:?}: a cleartext endpoint negotiates no session, got {negotiated:?} \
                 in {report}"
            ),
        }

        // The handshake: what answered is this member, with the role its cluster gives it and
        // the mechanisms its master user may authenticate with.
        let hello = report.get("hello").cloned().unwrap_or(Value::Null);
        assert_eq!(
            hello.get("ok").and_then(Value::as_f64),
            Some(1.0),
            "case {case:?}: the handshake must be answered, got {report}"
        );
        assert_eq!(
            hello.get("isWritablePrimary").and_then(Value::as_bool),
            Some(row.writer),
            "case {case:?}: expected the member's own role, got {hello}"
        );
        assert!(
            hello
                .get("maxWireVersion")
                .and_then(Value::as_i64)
                .is_some_and(|version| version > 0),
            "case {case:?}: the handshake must publish a wire version, got {hello}"
        );
        assert!(
            hello
                .get("saslSupportedMechs")
                .and_then(Value::as_array)
                .is_some_and(|offered| offered.contains(&json!("SCRAM-SHA-256"))),
            "case {case:?}: the handshake must offer the mechanism the session used, got {hello}"
        );

        // The exchange: the session is the cluster's master user, and the endpoint proved it
        // holds the same credential.
        let auth = report.get("auth").cloned().unwrap_or(Value::Null);
        assert_eq!(
            (
                auth.pointer("/continued/ok").and_then(Value::as_f64),
                auth.pointer("/continued/done").and_then(Value::as_bool),
            ),
            (Some(1.0), Some(true)),
            "case {case:?}: the master user's password must authenticate the session, got {auth}"
        );
        assert_eq!(
            auth.get("server_signature_verified").and_then(Value::as_bool),
            Some(true),
            "case {case:?}: the endpoint must prove it holds the same credential, got {auth}"
        );
        assert_eq!(
            auth.get("nonce_extends_the_clients").and_then(Value::as_bool),
            Some(true),
            "case {case:?}: the exchange must run on the nonce the client contributed, got {auth}"
        );

        // The writes: a member its cluster writes through stores each document, and one that is
        // not is refused by the engine's own code rather than answering a write it never took.
        let inserts = report
            .get("inserts")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let documents = row.documents.as_array().cloned().unwrap_or_default();
        assert_eq!(
            inserts.len(),
            documents.len(),
            "case {case:?}: every document the session wrote must be answered, got {report}"
        );
        for (at, answered) in inserts.iter().enumerate() {
            let written = &documents[at];
            match row.writer {
                true => assert_eq!(
                    (
                        answered.get("ok").and_then(Value::as_f64),
                        answered.get("n").and_then(Value::as_i64),
                    ),
                    (Some(1.0), Some(1)),
                    "case {case:?}: writing {written} must store one document, got {answered}"
                ),
                false => assert_eq!(
                    (
                        answered.get("ok").and_then(Value::as_f64),
                        answered.get("code").and_then(Value::as_i64),
                    ),
                    (Some(0.0), Some(NOT_WRITABLE_PRIMARY)),
                    "case {case:?}: a member that is not the writer must refuse {written}, got \
                     {answered}"
                ),
            }
        }

        // The reads: each selection answers the documents of the member's own CLUSTER, so a
        // replica reads what the writer wrote.
        for (name, filter, expected) in &row.finds {
            let answered = report
                .pointer(&format!("/finds/{name}"))
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                answered.get("ok").and_then(Value::as_f64),
                Some(1.0),
                "case {case:?} / {name:?}: the find under {filter} must be answered, got \
                 {answered}"
            );
            let batch = batch_of(&answered);
            let names: Vec<String> = batch
                .iter()
                .map(|document| {
                    document
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("<no name>")
                        .to_string()
                })
                .collect();
            assert_eq!(
                names,
                expected
                    .iter()
                    .map(|name| (*name).to_string())
                    .collect::<Vec<String>>(),
                "case {case:?} / {name:?}: expected the documents {expected:?} under {filter}, \
                 got {answered}"
            );
            // Every stored document carries an `_id`: the one it was written with, or the one
            // the engine minted for a document that carried none.
            for document in &batch {
                assert!(
                    document.get("_id").is_some(),
                    "case {case:?} / {name:?}: every stored document carries an _id, got \
                     {document}"
                );
            }
        }
    }

    // And an address this emulator opened no plane at is answered the way a host answers one,
    // by the environment the handler runs in and never by a connection leaving this process:
    // another port of the same host is REFUSED, and an address of another host has no route at
    // all. Each row pins the refusal to the number that names it, so a row cannot be satisfied
    // by some other failure — a wait that ran out, a name nothing resolved — which is what
    // would be reported if the attempt had gone anywhere.
    let (served_host, _) = published
        .get(WRITER)
        .expect("the writer published an address")
        .clone();
    let unreachable: [(&str, Value, u64, i64); 2] = [
        (
            "a port of the same host nothing was opened at",
            json!(served_host),
            UNSERVED_PORT,
            CONNECTION_REFUSED,
        ),
        (
            "an address of another host, outside this emulator entirely",
            json!(OFF_EMULATOR_ADDRESS),
            27_017,
            NETWORK_UNREACHABLE,
        ),
    ];
    for (case, host, port, expected_errno) in unreachable {
        let report = probe(
            &router,
            case,
            &json!({
                "host": host,
                "port": port,
                "tls": false,
                "username": MASTER_USERNAME,
                "password": MASTER_PASSWORD,
                "database": DATABASE,
                "collection": COLLECTION,
            }),
        )
        .await;
        assert_eq!(
            report.get("ok").and_then(Value::as_bool),
            Some(false),
            "case {case:?}: expected {host}:{port} to answer nothing, got {report}"
        );
        assert_eq!(
            report.get("errno").and_then(Value::as_i64),
            Some(expected_errno),
            "case {case:?}: expected the environment's own refusal {expected_errno} for \
             {host}:{port}, got {report}"
        );
        assert!(
            report.get("refused").and_then(Value::as_str).is_some(),
            "case {case:?}: expected the refusal a host answers with, got {report}"
        );
    }
}
