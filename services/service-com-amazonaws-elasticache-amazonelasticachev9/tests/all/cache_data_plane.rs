// SPDX-License-Identifier: CC0-1.0

//! The DATA PLANE of a cache: that the endpoint the control plane publishes is an address
//! a client of THIS emulator can open a RESP session on, and that the session reads the
//! keyspace it wrote.
//!
//! The rule under test: an endpoint a service advertises is an address the service answers
//! on, so every `Endpoint` / `ConfigurationEndpoint` / `PrimaryEndpoint` a cache publishes
//! is one of the emulator's OWN addresses and a RESP client dialling it is served out of
//! that cache's keyspace. The tables below span what the rule generalises over — which
//! KIND of cache publishes the endpoint (a cache cluster, a replication group's primary
//! and reader, a serverless cache), which ENGINE it runs, whether the wire is encrypted,
//! which COMMANDS the session sends, and who the session is (unauthenticated, the cache's
//! auth token, a user of its user group) — and the recorded `SET`/`GET` over a cluster
//! endpoint is one row of it.
//!
//! Every assertion goes over a real socket the core opened for the cache, dialled at the
//! address the control plane published; nothing reaches into the service's state.

use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::harness::{params, poll_until, Emulator, Reply, REGION};

/// A byte stream a RESP session runs on: a plaintext socket, or the TLS stream over one.
type Session = Box<dyn Stream>;

/// What a session needs of its connection.
trait Stream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> Stream for T {}

/// One RESP reply, as a client reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Resp {
    Status(String),
    Error(String),
    Integer(i64),
    Bulk(String),
    Nil,
    Array(Vec<Resp>),
}

impl Resp {
    /// The error code a refusal carries (its first word), or `None` for a success.
    fn error_code(&self) -> Option<&str> {
        match self {
            Self::Error(message) => message.split_whitespace().next(),
            _ => None,
        }
    }
}

/// Open a plaintext RESP session on the address the control plane published.
///
/// The assertion is what pins the endpoint to this machine: the core binds its listeners
/// on the operator-chosen interface, loopback by default, so the address a cache publishes
/// is this machine's own loopback address — and that is the address dialled.
async fn open(case: &str, address: &str, port: &str) -> Session {
    assert_eq!(
        address, "127.0.0.1",
        "case {case}: the published endpoint must be this machine's loopback address, got \
         {address:?}"
    );
    let port: u16 = port
        .parse()
        .unwrap_or_else(|_| panic!("case {case}: the published port must be a port, got {port:?}"));
    let stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap_or_else(|error| {
            panic!("case {case}: {address}:{port} must accept a connection, got {error}")
        });
    Box::new(stream)
}

/// Open a RESP session over TLS, which is how a client reaches a cache with in-transit
/// encryption — the wire the core terminates for such a cache.
async fn open_tls(case: &str, address: &str, port: &str) -> Session {
    let conn = open(case, address, port).await;
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .expect("the provider supports the default protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptTheEmulatorsCertificate(provider)))
        .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    let server_name =
        rustls::pki_types::ServerName::try_from("localhost").expect("localhost is a server name");
    let stream = connector
        .connect(server_name, conn)
        .await
        .unwrap_or_else(|error| {
            panic!("case {case}: the cache's socket must complete a TLS handshake, got {error}")
        });
    Box::new(stream)
}

/// The verifier a client of the emulator's OWN socket uses: the certificate is the one the
/// core generated for its listeners, self-signed and naming loopback, and this client is
/// the emulator's own test connecting to a port the same process opened — so there is no
/// authority to check it against.
#[derive(Debug)]
struct AcceptTheEmulatorsCertificate(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for AcceptTheEmulatorsCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// Send one command as a RESP array of bulk strings — the form every client library sends
/// — and read the one reply it is answered with.
async fn command(case: &str, session: &mut Session, arguments: &[&str]) -> Resp {
    let mut request = format!("*{}\r\n", arguments.len());
    for argument in arguments {
        request.push_str(&format!("${}\r\n{argument}\r\n", argument.len()));
    }
    session
        .write_all(request.as_bytes())
        .await
        .unwrap_or_else(|error| panic!("case {case}: {arguments:?} must be sent, got {error}"));
    session
        .flush()
        .await
        .unwrap_or_else(|error| panic!("case {case}: {arguments:?} must be flushed, got {error}"));
    read_reply(case, session, arguments).await
}

/// Read one reply off the session, as a client's parser does.
async fn read_reply(case: &str, session: &mut Session, sent: &[&str]) -> Resp {
    let marker = read_line(case, session, sent).await;
    let (kind, rest) = marker.split_at(1);
    match kind {
        "+" => Resp::Status(rest.to_string()),
        "-" => Resp::Error(rest.to_string()),
        ":" => Resp::Integer(rest.parse().unwrap_or_else(|_| {
            panic!("case {case}: {sent:?} answered an integer reply, got {marker:?}")
        })),
        "$" => match rest.parse::<i64>() {
            Ok(-1) => Resp::Nil,
            Ok(length) => {
                let mut value = vec![0u8; usize::try_from(length).unwrap_or_default() + 2];
                session.read_exact(&mut value).await.unwrap_or_else(|error| {
                    panic!("case {case}: {sent:?} must answer {length} bytes, got {error}")
                });
                value.truncate(usize::try_from(length).unwrap_or_default());
                Resp::Bulk(String::from_utf8_lossy(&value).to_string())
            }
            Err(_) => panic!("case {case}: {sent:?} answered a bulk reply, got {marker:?}"),
        },
        "*" => match rest.parse::<i64>() {
            Ok(-1) => Resp::Nil,
            Ok(count) => {
                let mut elements = Vec::new();
                for _ in 0..count.max(0) {
                    elements.push(Box::pin(read_reply(case, session, sent)).await);
                }
                Resp::Array(elements)
            }
            Err(_) => panic!("case {case}: {sent:?} answered an array reply, got {marker:?}"),
        },
        _ => panic!("case {case}: {sent:?} must be answered a RESP reply, got {marker:?}"),
    }
}

/// One `\r\n`-terminated line off the session.
async fn read_line(case: &str, session: &mut Session, sent: &[&str]) -> String {
    let mut line = Vec::new();
    loop {
        let mut byte = [0u8; 1];
        let read = session.read(&mut byte).await.unwrap_or_else(|error| {
            panic!("case {case}: {sent:?} must be answered, got {error}")
        });
        assert_eq!(
            read, 1,
            "case {case}: {sent:?} must be answered, the session ended after {line:?}"
        );
        if byte[0] == b'\n' {
            break;
        }
        if byte[0] != b'\r' {
            line.push(byte[0]);
        }
    }
    String::from_utf8_lossy(&line).to_string()
}

/// Create one cache cluster and wait until the clock has made it `available`, then answer
/// the endpoint it publishes for its one node: `(address, port)`.
async fn cluster_endpoint(emulator: &Emulator, id: &str, extra: &[(&str, &str)]) -> (String, String) {
    let mut pairs = params(&[("CacheClusterId", id)]);
    pairs.extend(params(extra));
    let created = emulator.call("CreateCacheCluster", &pairs).await;
    assert_eq!(
        created.status, 200,
        "the cluster {id} must be created, got {} {}",
        created.status, created.body
    );
    let described = poll_until(
        emulator,
        REGION,
        id,
        "the cluster reaching available with its node provisioned",
        "DescribeCacheClusters",
        &params(&[("CacheClusterId", id), ("ShowCacheNodeInfo", "true")]),
        |reply| reply.text("CacheCluster/CacheClusterStatus") == Some("available"),
    )
    .await;
    endpoint_of(
        &described,
        "CacheNodes/CacheNode/Endpoint/Address",
        "CacheNodes/CacheNode/Endpoint/Port",
        id,
    )
}

/// The `(address, port)` pair one reply reports under the two element names.
fn endpoint_of(reply: &Reply, address: &str, port: &str, what: &str) -> (String, String) {
    (
        reply
            .text(address)
            .unwrap_or_else(|| panic!("{what} must publish {address}, got {}", reply.body))
            .to_string(),
        reply
            .text(port)
            .unwrap_or_else(|| panic!("{what} must publish {port}, got {}", reply.body))
            .to_string(),
    )
}

#[tokio::test]
async fn a_cache_is_reached_at_the_address_it_publishes() {
    // The axis: WHICH kind of cache publishes the endpoint and which engine it runs. Each
    // row is dialled at the address its own describe published.
    let emulator = Emulator::new();
    let cases: [(&str, &[(&str, &str)]); 3] = [
        ("resp-redis", &[("Engine", "redis")]),
        ("resp-valkey", &[("Engine", "valkey")]),
        (
            "resp-port-named",
            &[("Engine", "redis"), ("Port", "6379")],
        ),
    ];
    for (id, extra) in cases {
        let (address, port) = cluster_endpoint(&emulator, id, extra).await;
        let mut session = open(id, &address, &port).await;
        assert_eq!(
            command(id, &mut session, &["PING"]).await,
            Resp::Status("PONG".to_string()),
            "case {id}: the cache must answer a PING on the address it published"
        );
        // The keyspace is this cache's own: what the session wrote is what it reads.
        assert_eq!(
            command(id, &mut session, &["SET", "greeting", "hello"]).await,
            Resp::Status("OK".to_string()),
            "case {id}: a SET on the published endpoint must be accepted"
        );
        assert_eq!(
            command(id, &mut session, &["GET", "greeting"]).await,
            Resp::Bulk("hello".to_string()),
            "case {id}: a GET must answer what the SET stored"
        );
        // And it is NOT another cache's: a key written on one cache is not on another.
        assert_eq!(
            command(id, &mut session, &["GET", "written-elsewhere"]).await,
            Resp::Nil,
            "case {id}: a key no session of this cache wrote must be absent"
        );
        assert_eq!(
            command(id, &mut session, &["SET", &format!("key-of-{id}"), id]).await,
            Resp::Status("OK".to_string()),
            "case {id}: the cache must take a key of its own"
        );
        for (other, _) in cases {
            if other == id {
                continue;
            }
            assert_eq!(
                command(id, &mut session, &["GET", &format!("key-of-{other}")]).await,
                Resp::Nil,
                "case {id}: the keyspace of {other} must not be reachable here"
            );
        }
    }
}

#[tokio::test]
async fn a_memcached_cluster_publishes_the_name_its_own_clients_resolve() {
    // The counter-case to the rule: this emulator serves no Memcached wire, so a Memcached
    // cluster keeps the name ElastiCache's own zone would carry rather than publishing a
    // port that would answer the wrong protocol.
    let emulator = Emulator::new();
    let (address, port) = cluster_endpoint(
        &emulator,
        "memcached-cluster",
        &[("Engine", "memcached"), ("NumCacheNodes", "2")],
    )
    .await;
    assert!(
        address.ends_with(&format!("{REGION}.cache.amazonaws.com")),
        "a Memcached cluster keeps its zone name, got {address:?}"
    );
    assert_eq!(
        port, "11211",
        "a Memcached cluster keeps its engine's port, got {port:?}"
    );
}

#[tokio::test]
async fn the_commands_a_client_sends_read_the_keyspace_they_wrote() {
    let emulator = Emulator::new();
    let (address, port) = cluster_endpoint(&emulator, "command-surface", &[("Engine", "valkey")])
        .await;
    let mut session = open("command-surface", &address, &port).await;

    // The axis: the command, and the reply the engine answers it with. Each row is
    // (case, command, expected reply) over a keyspace the rows before it built.
    let cases: &[(&str, &[&str], Resp)] = &[
        ("echo", &["ECHO", "spoken"], Resp::Bulk("spoken".to_string())),
        ("set", &["SET", "a", "1"], Resp::Status("OK".to_string())),
        ("get", &["GET", "a"], Resp::Bulk("1".to_string())),
        ("strlen", &["STRLEN", "a"], Resp::Integer(1)),
        ("append", &["APPEND", "a", "23"], Resp::Integer(3)),
        ("get after append", &["GET", "a"], Resp::Bulk("123".to_string())),
        ("type", &["TYPE", "a"], Resp::Status("string".to_string())),
        ("incr", &["INCR", "a"], Resp::Integer(124)),
        ("incrby", &["INCRBY", "a", "6"], Resp::Integer(130)),
        ("decrby", &["DECRBY", "a", "30"], Resp::Integer(100)),
        ("decr", &["DECR", "a"], Resp::Integer(99)),
        // The ENCODING the engine holds a value in: an integer-valued string is held as an
        // integer, which is what makes an `INCR` an increment rather than a rewrite.
        (
            "object encoding of an integer",
            &["OBJECT", "ENCODING", "a"],
            Resp::Bulk("int".to_string()),
        ),
        (
            "object encoding of a key that is not there",
            &["OBJECT", "ENCODING", "ghost"],
            Resp::Error("ERR no such key".to_string()),
        ),
        ("setnx on a held key", &["SETNX", "a", "x"], Resp::Integer(0)),
        ("setnx on a free key", &["SETNX", "b", "x"], Resp::Integer(1)),
        ("exists", &["EXISTS", "a", "b"], Resp::Integer(2)),
        ("set nx on a held key", &["SET", "a", "z", "NX"], Resp::Nil),
        ("set xx on a free key", &["SET", "nothing", "z", "XX"], Resp::Nil),
        ("get of a key xx left alone", &["GET", "nothing"], Resp::Nil),
        ("mset", &["MSET", "m1", "v1", "m2", "v2"], Resp::Status("OK".to_string())),
        (
            "object encoding of a short string",
            &["OBJECT", "ENCODING", "m1"],
            Resp::Bulk("embstr".to_string()),
        ),
        (
            "mget",
            &["MGET", "m1", "m2", "missing"],
            Resp::Array(vec![
                Resp::Bulk("v1".to_string()),
                Resp::Bulk("v2".to_string()),
                Resp::Nil,
            ]),
        ),
        ("ttl of a key with no expiry", &["TTL", "a"], Resp::Integer(-1)),
        ("ttl of a key that is not there", &["TTL", "ghost"], Resp::Integer(-2)),
        ("expire", &["EXPIRE", "b", "100"], Resp::Integer(1)),
        ("ttl after expire", &["TTL", "b"], Resp::Integer(100)),
        ("persist", &["PERSIST", "b"], Resp::Integer(1)),
        ("ttl after persist", &["TTL", "b"], Resp::Integer(-1)),
        ("expire in the past deletes", &["EXPIRE", "b", "-1"], Resp::Integer(1)),
        ("get after that", &["GET", "b"], Resp::Nil),
        ("del", &["DEL", "m1", "m2", "missing"], Resp::Integer(2)),
        ("dbsize", &["DBSIZE"], Resp::Integer(1)),
        ("keys", &["KEYS", "*"], Resp::Array(vec![Resp::Bulk("a".to_string())])),
        // `SCAN … TYPE` narrows the walk to one data type: this keyspace holds strings, so
        // a scan for any other type selects nothing.
        (
            "scan for the type the keyspace holds",
            &["SCAN", "0", "TYPE", "string"],
            Resp::Array(vec![
                Resp::Bulk("0".to_string()),
                Resp::Array(vec![Resp::Bulk("a".to_string())]),
            ]),
        ),
        (
            "scan for a type the keyspace does not hold",
            &["SCAN", "0", "TYPE", "list"],
            Resp::Array(vec![Resp::Bulk("0".to_string()), Resp::Array(Vec::new())]),
        ),
        ("flushall", &["FLUSHALL"], Resp::Status("OK".to_string())),
        ("dbsize after flushall", &["DBSIZE"], Resp::Integer(0)),
        ("select the one database", &["SELECT", "0"], Resp::Status("OK".to_string())),
        ("an unknown command", &["NOSUCHCOMMAND"], Resp::Error(
            "ERR unknown command 'nosuchcommand', with args beginning with: ".to_string(),
        )),
        ("a command with the wrong arity", &["GET"], Resp::Error(
            "ERR wrong number of arguments for 'get' command".to_string(),
        )),
        // A caller-supplied expiry past what an instant can carry is an invalid expire
        // time, not a dropped session.
        (
            "setex with a span no instant can carry",
            &["SETEX", "overflowing", "9223372036854775807", "v"],
            Resp::Error("ERR invalid expire time in 'setex' command".to_string()),
        ),
        (
            "psetex with the largest span an instant can carry",
            &["PSETEX", "far-future", "9223372036854775807", "v"],
            Resp::Status("OK".to_string()),
        ),
        (
            "expire with a span no instant can carry",
            &["EXPIRE", "a", "9223372036854775807"],
            Resp::Error("ERR invalid expire time in 'expire' command".to_string()),
        ),
        ("the key it left alone", &["GET", "overflowing"], Resp::Nil),
    ];
    for (case, arguments, expected) in cases {
        let answered = command(case, &mut session, arguments).await;
        assert_eq!(
            answered, *expected,
            "case {case}: {arguments:?} must answer {expected:?}, got {answered:?}"
        );
    }

    // A key NAME is request-controlled and is held for as long as the key is, so it is
    // bounded exactly as a value is: one past the bound is refused rather than stored.
    let long_key = "k".repeat(4 * 1024 + 1);
    let refused = command("a key past the bound", &mut session, &["SET", &long_key, "v"]).await;
    assert!(
        matches!(&refused, Resp::Error(message) if message.contains("key exceeds maximum allowed size")),
        "a key past the bound must be refused, got {refused:?}"
    );
    assert_eq!(
        command("a key past the bound", &mut session, &["EXISTS", &long_key]).await,
        Resp::Integer(0),
        "the refused key must not have been stored"
    );
    let longest_key = "k".repeat(4 * 1024);
    assert_eq!(
        command("the longest key", &mut session, &["SET", &longest_key, "v"]).await,
        Resp::Status("OK".to_string()),
        "a key at the bound must be taken"
    );

    // A key whose expiry the clock has passed is GONE from the moment it passed — the one
    // thing a cache's clients rely on a TTL for. The expiry is a span this test waits out
    // by looking again, not by asserting on how fast the host is.
    assert_eq!(
        command("expiry", &mut session, &["SET", "short", "value", "PX", "50"]).await,
        Resp::Status("OK".to_string()),
        "a SET with a millisecond expiry must be accepted"
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if command("expiry", &mut session, &["GET", "short"]).await == Resp::Nil {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "a key whose expiry has passed must be gone"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        command("expiry", &mut session, &["EXISTS", "short"]).await,
        Resp::Integer(0),
        "a key whose expiry has passed must not be reported as existing"
    );
}

#[tokio::test]
async fn a_replication_groups_members_share_one_keyspace_and_only_the_primary_takes_writes() {
    let emulator = Emulator::new();
    let created = emulator
        .call(
            "CreateReplicationGroup",
            &params(&[
                ("ReplicationGroupId", "shared-keyspace"),
                ("ReplicationGroupDescription", "a group with a replica"),
                ("Engine", "redis"),
                ("NumCacheClusters", "2"),
                ("AutomaticFailoverEnabled", "true"),
            ]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the group must be created, got {} {}",
        created.status, created.body
    );
    let described = poll_until(
        &emulator,
        REGION,
        "shared-keyspace",
        "the group reaching available",
        "DescribeReplicationGroups",
        &params(&[("ReplicationGroupId", "shared-keyspace")]),
        |reply| reply.text("ReplicationGroup/Status") == Some("available"),
    )
    .await;
    let (primary_address, primary_port) = endpoint_of(
        &described,
        "NodeGroup/PrimaryEndpoint/Address",
        "NodeGroup/PrimaryEndpoint/Port",
        "the group",
    );
    let (reader_address, reader_port) = endpoint_of(
        &described,
        "NodeGroup/ReaderEndpoint/Address",
        "NodeGroup/ReaderEndpoint/Port",
        "the group",
    );
    assert_ne!(
        primary_port, reader_port,
        "the primary and the reader are different members, so they answer on different sockets"
    );

    let mut writer = open("primary", &primary_address, &primary_port).await;
    assert_eq!(
        command("primary", &mut writer, &["SET", "replicated", "written-on-the-primary"]).await,
        Resp::Status("OK".to_string()),
        "the primary endpoint must take a write"
    );
    let mut reader = open("reader", &reader_address, &reader_port).await;
    assert_eq!(
        command("reader", &mut reader, &["GET", "replicated"]).await,
        Resp::Bulk("written-on-the-primary".to_string()),
        "a replica answers what the primary wrote: the members of a group share one keyspace"
    );
    // "You can't write against a read only replica."
    let refused = command("reader", &mut reader, &["SET", "replicated", "written-on-a-replica"])
        .await;
    assert_eq!(
        refused.error_code(),
        Some("READONLY"),
        "a write on a replica's endpoint must be refused READONLY, got {refused:?}"
    );
    assert_eq!(
        command("reader", &mut reader, &["GET", "replicated"]).await,
        Resp::Bulk("written-on-the-primary".to_string()),
        "the refused write must have changed nothing"
    );
    // A read is served on either endpoint, and an INCR on the primary is counted once per
    // call however many sessions are open.
    for round in 1..=3 {
        assert_eq!(
            command("counter", &mut writer, &["INCR", "counter"]).await,
            Resp::Integer(round),
            "every INCR on the primary must count once"
        );
        assert_eq!(
            command("counter", &mut reader, &["GET", "counter"]).await,
            Resp::Bulk(round.to_string()),
            "the replica must report the count the primary reached"
        );
    }
}

#[tokio::test]
async fn a_cache_with_an_auth_token_serves_nothing_until_a_client_authenticates_with_it() {
    let emulator = Emulator::new();
    // The token the group is created with, and the ones it is rotated onto: the digests of
    // the two the rotation window holds are what the cache checks an `AUTH` against, so
    // the token being replaced goes on working while the rotation is live
    // (`AuthTokenUpdateStrategyType` "ROTATE … without interrupting access").
    const FIRST: &str = "a-token-of-at-least-sixteen-characters";
    const SECOND: &str = "another-token-of-sixteen-plus-characters";
    const THIRD: &str = "a-third-token-of-sixteen-plus-characters";
    let created = emulator
        .call(
            "CreateReplicationGroup",
            &params(&[
                ("ReplicationGroupId", "authenticated"),
                ("ReplicationGroupDescription", "a group behind an auth token"),
                ("Engine", "redis"),
                ("NumCacheClusters", "1"),
                ("TransitEncryptionEnabled", "true"),
                ("AuthToken", FIRST),
            ]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the group must be created, got {} {}",
        created.status, created.body
    );
    let described = poll_until(
        &emulator,
        REGION,
        "authenticated",
        "the group reaching available",
        "DescribeReplicationGroups",
        &params(&[("ReplicationGroupId", "authenticated")]),
        |reply| reply.text("ReplicationGroup/Status") == Some("available"),
    )
    .await;
    assert_eq!(
        described.text("ReplicationGroup/AuthTokenEnabled"),
        Some("true"),
        "the group must report that it carries a token, got {}",
        described.body
    );
    let (address, port) = endpoint_of(
        &described,
        "NodeGroup/PrimaryEndpoint/Address",
        "NodeGroup/PrimaryEndpoint/Port",
        "the group",
    );

    // The axis: what the session sends before its first command. A cache behind a token
    // serves nothing until the right one arrives.
    let mut session = open_tls("before auth", &address, &port).await;
    let unauthenticated = command("before auth", &mut session, &["GET", "key"]).await;
    assert_eq!(
        unauthenticated.error_code(),
        Some("NOAUTH"),
        "a session that has not authenticated must be refused NOAUTH, got {unauthenticated:?}"
    );
    let wrong = command("wrong token", &mut session, &["AUTH", "not-the-token-at-all"]).await;
    assert_eq!(
        wrong.error_code(),
        Some("WRONGPASS"),
        "the wrong token must be refused WRONGPASS, got {wrong:?}"
    );
    assert_eq!(
        command("right token", &mut session, &["AUTH", FIRST]).await,
        Resp::Status("OK".to_string()),
        "the cache's own token must authenticate the session"
    );
    assert_eq!(
        command("after auth", &mut session, &["SET", "key", "value"]).await,
        Resp::Status("OK".to_string()),
        "an authenticated session is served"
    );

    // A ROTATE leaves exactly TWO tokens live — the one being replaced and the new one —
    // so a second rotation drops the oldest rather than adding a third, and a SET leaves
    // only the new one. The axis: the strategy, the token it names, and which of the
    // EARLIER tokens still authenticates afterwards.
    for (case, strategy, token, still_live, no_longer_live) in [
        ("rotate", "ROTATE", SECOND, Some(FIRST), None),
        ("rotate again", "ROTATE", THIRD, Some(SECOND), Some(FIRST)),
        ("set", "SET", THIRD, None, Some(SECOND)),
    ] {
        let modified = emulator
            .call(
                "ModifyReplicationGroup",
                &params(&[
                    ("ReplicationGroupId", "authenticated"),
                    ("AuthToken", token),
                    ("AuthTokenUpdateStrategy", strategy),
                    ("ApplyImmediately", "true"),
                ]),
            )
            .await;
        assert_eq!(
            modified.status, 200,
            "case {case}: the token update must be accepted, got {} {}",
            modified.status, modified.body
        );
        let mut session = open_tls(case, &address, &port).await;
        assert_eq!(
            command(case, &mut session, &["AUTH", token]).await,
            Resp::Status("OK".to_string()),
            "case {case}: the token the update named must authenticate"
        );
        if let Some(earlier) = still_live {
            let mut old = open_tls(case, &address, &port).await;
            assert_eq!(
                command(case, &mut old, &["AUTH", earlier]).await,
                Resp::Status("OK".to_string()),
                "case {case}: a rotation keeps the token being replaced live"
            );
        }
        if let Some(dropped) = no_longer_live {
            let mut old = open_tls(case, &address, &port).await;
            let answered = command(case, &mut old, &["AUTH", dropped]).await;
            assert_eq!(
                answered.error_code(),
                Some("WRONGPASS"),
                "case {case}: a token the rotation window has left behind must authenticate \
                 nobody, got {answered:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_command_frame_that_is_not_a_flat_multibulk_is_a_protocol_error() {
    // What a client sends is not what a server may trust: a command is a FLAT array of
    // bulk strings, so a nested aggregate is refused as the protocol error it is — before
    // anything walks it — and the session ends. The rows span the shapes that are not a
    // command: one level of nesting, many, and an element of a type a command never
    // carries.
    let emulator = Emulator::new();
    let (address, port) = cluster_endpoint(&emulator, "framing", &[("Engine", "redis")]).await;
    let nested_deeply: String = format!("{}$4\r\nPING\r\n", "*1\r\n".repeat(2048));
    let cases: [(&str, &str); 3] = [
        ("one nested array", "*1\r\n*1\r\n$4\r\nPING\r\n"),
        ("many nested arrays", &nested_deeply),
        ("an element that is not a bulk string", "*1\r\n:7\r\n"),
    ];
    for (case, bytes) in cases {
        let mut session = open(case, &address, &port).await;
        session
            .write_all(bytes.as_bytes())
            .await
            .unwrap_or_else(|error| panic!("case {case}: the frame must be sent, got {error}"));
        session
            .flush()
            .await
            .unwrap_or_else(|error| panic!("case {case}: the frame must be flushed, got {error}"));
        let answered = read_reply(case, &mut session, &["a frame that is not a command"]).await;
        assert_eq!(
            answered.error_code(),
            Some("ERR"),
            "case {case}: a frame that is not a command must be a protocol error, got \
             {answered:?}"
        );
        assert!(
            matches!(&answered, Resp::Error(message) if message.contains("Protocol error")),
            "case {case}: the refusal must say so, got {answered:?}"
        );
    }
    // The cache is still serving every other client: one bad frame ends its own session
    // and nothing else.
    let mut session = open("after the bad frames", &address, &port).await;
    assert_eq!(
        command("after the bad frames", &mut session, &["PING"]).await,
        Resp::Status("PONG".to_string()),
        "a frame that was not a command must not have taken the cache down"
    );
}

/// An IAM authentication token for the user `iam-user`: the presigned request to connect
/// as that user an IAM-authenticated client sends as its credential
/// (`Action=connect&User=<user>` plus the SigV4 fields of the signature). The signature is
/// not a real one — this emulator verifies no caller's identity — and nothing in the token
/// is secret.
const IAM_TOKEN: &str = "rbac-cache/?Action=connect&User=iam-user\
                         &X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Expires=900\
                         &X-Amz-SignedHeaders=host&X-Amz-Signature=0123456789abcdef";

/// The same token made out for ANOTHER user, which is not a credential for `iam-user`.
const OTHER_IAM_TOKEN: &str = "rbac-cache/?Action=connect&User=somebody-else\
                               &X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Expires=900\
                               &X-Amz-SignedHeaders=host&X-Amz-Signature=0123456789abcdef";

#[tokio::test]
async fn a_user_group_decides_who_a_session_is_and_which_keys_it_may_reach() {
    let emulator = Emulator::new();
    // The RBAC pattern AWS documents: the group's `default`-named member is the caller's
    // OWN user with no access, so unauthenticated traffic is served nothing, and the users
    // that do the work authenticate by name.
    let users: [(&str, &str, &str, Option<&str>); 4] = [
        ("own-default", "default", "off -@all", None),
        ("app-user", "app", "on ~app:* +@all", Some("the-application-password")),
        ("read-only", "readonly", "on ~app:* +@read", Some("the-read-only-password")),
        // A pattern granted for READS alone (`%R~`): the direction is part of the rule, so
        // this user reads those keys and does not write them, even though it may run every
        // command.
        (
            "scoped-reader",
            "scoped",
            "on %R~app:* +@all",
            Some("the-scoped-password"),
        ),
    ];
    for (id, name, access, password) in users {
        let mut pairs = params(&[
            ("UserId", id),
            ("UserName", name),
            ("Engine", "redis"),
            ("AccessString", access),
        ]);
        match password {
            Some(password) => pairs.extend(params(&[("Passwords.member.1", password)])),
            None => pairs.extend(params(&[("NoPasswordRequired", "true")])),
        }
        let created = emulator.call("CreateUser", &pairs).await;
        assert_eq!(
            created.status, 200,
            "the user {id} must be created, got {} {}",
            created.status, created.body
        );
    }
    // An IAM-authenticated user, whose credential is an IAM authentication token rather
    // than a password — ElastiCache requires such a user's id and name to be identical.
    let iam_user = emulator
        .call(
            "CreateUser",
            &params(&[
                ("UserId", "iam-user"),
                ("UserName", "iam-user"),
                ("Engine", "redis"),
                ("AccessString", "on ~app:* +@read"),
                ("AuthenticationMode.Type", "iam"),
            ]),
        )
        .await;
    assert_eq!(
        iam_user.status, 200,
        "the IAM user must be created, got {} {}",
        iam_user.status, iam_user.body
    );
    let group = emulator
        .call(
            "CreateUserGroup",
            &params(&[
                ("UserGroupId", "app-group"),
                ("Engine", "redis"),
                ("UserIds.member.1", "own-default"),
                ("UserIds.member.2", "app-user"),
                ("UserIds.member.3", "read-only"),
                ("UserIds.member.4", "iam-user"),
                ("UserIds.member.5", "scoped-reader"),
            ]),
        )
        .await;
    assert_eq!(
        group.status, 200,
        "a group whose `default`-named member is the caller's own user must be accepted, got {} {}",
        group.status, group.body
    );
    // A cache can only be attached to an ACTIVE user group, which the clock owes the group
    // a span after its create; the suite waits for that condition rather than for a time.
    poll_until(
        &emulator,
        REGION,
        "app-group",
        "the user group reaching active",
        "DescribeUserGroups",
        &params(&[("UserGroupId", "app-group")]),
        |reply| reply.text("member/Status") == Some("active"),
    )
    .await;
    let created = emulator
        .call(
            "CreateServerlessCache",
            &params(&[
                ("ServerlessCacheName", "rbac-cache"),
                ("Engine", "valkey"),
                ("UserGroupId", "app-group"),
            ]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the cache must be created, got {} {}",
        created.status, created.body
    );
    let described = poll_until(
        &emulator,
        REGION,
        "rbac-cache",
        "the cache reaching available",
        "DescribeServerlessCaches",
        &params(&[("ServerlessCacheName", "rbac-cache")]),
        |reply| reply.text("member/Status") == Some("available"),
    )
    .await;
    let (address, port) = endpoint_of(
        &described,
        "Endpoint/Address",
        "Endpoint/Port",
        "the cache",
    );

    // A session that does not authenticate is the group's `default`-named user, which is
    // `off`: it is served nothing.
    let mut anonymous = open_tls("anonymous", &address, &port).await;
    let refused = command("anonymous", &mut anonymous, &["GET", "app:key"]).await;
    assert_eq!(
        refused.error_code(),
        Some("NOAUTH"),
        "a cache whose default user is off serves an unauthenticated session nothing, got \
         {refused:?}"
    );

    // The axis: who the session authenticates as, which command it sends and which key it
    // names. Each row is the reply that user gets.
    type AuthorizationCase = (
        &'static str,
        &'static str,
        &'static str,
        &'static [&'static str],
        Option<&'static str>,
    );
    let rows: [AuthorizationCase; 12] = [
        ("the wrong password", "app", "no", &["GET", "app:key"], Some("WRONGPASS")),
        ("a user the group does not hold", "stranger", "the-application-password", &["GET", "app:key"], Some("WRONGPASS")),
        ("the application user writing its own keys", "app", "the-application-password", &["SET", "app:key", "value"], None),
        ("the application user reading them back", "app", "the-application-password", &["GET", "app:key"], None),
        ("the application user outside its pattern", "app", "the-application-password", &["GET", "other:key"], Some("NOPERM")),
        ("the read-only user reading", "readonly", "the-read-only-password", &["GET", "app:key"], None),
        ("the read-only user writing", "readonly", "the-read-only-password", &["SET", "app:key", "other"], Some("NOPERM")),
        // An IAM user's credential is an IAM authentication token for itself: a signed
        // request to connect as that user. A password is not one, and neither is a token
        // naming somebody else.
        ("the IAM user with a token of its own", "iam-user", IAM_TOKEN, &["GET", "app:key"], None),
        ("the IAM user with a password", "iam-user", "the-application-password", &["GET", "app:key"], Some("WRONGPASS")),
        ("the IAM user with another user's token", "iam-user", OTHER_IAM_TOKEN, &["GET", "app:key"], Some("WRONGPASS")),
        // The direction a pattern was granted in is honoured: a read of a key the user may
        // read is served, a write of the same key is not.
        ("the read-scoped user reading", "scoped", "the-scoped-password", &["GET", "app:key"], None),
        ("the read-scoped user writing", "scoped", "the-scoped-password", &["SET", "app:key", "other"], Some("NOPERM")),
    ];
    for (case, user, password, arguments, expected_code) in rows {
        let mut session = open_tls(case, &address, &port).await;
        let authenticated = command(case, &mut session, &["AUTH", user, password]).await;
        if expected_code == Some("WRONGPASS") {
            assert_eq!(
                authenticated.error_code(),
                Some("WRONGPASS"),
                "case {case}: the pair must authenticate nobody, got {authenticated:?}"
            );
            continue;
        }
        assert_eq!(
            authenticated,
            Resp::Status("OK".to_string()),
            "case {case}: the user must authenticate, got {authenticated:?}"
        );
        let answered = command(case, &mut session, arguments).await;
        match expected_code {
            Some(code) => assert_eq!(
                answered.error_code(),
                Some(code),
                "case {case}: {arguments:?} must be refused {code}, got {answered:?}"
            ),
            None => assert!(
                answered.error_code().is_none(),
                "case {case}: {arguments:?} must be served, got {answered:?}"
            ),
        }
    }

    // A user REMOVED from the group is no longer a user of the cache it serves: the
    // membership is what decides, so the same credential authenticates nobody afterwards.
    let modified = emulator
        .call(
            "ModifyUserGroup",
            &params(&[
                ("UserGroupId", "app-group"),
                ("UserIdsToRemove.member.1", "read-only"),
            ]),
        )
        .await;
    assert_eq!(
        modified.status, 200,
        "the user group must take the change, got {} {}",
        modified.status, modified.body
    );
    poll_until(
        &emulator,
        REGION,
        "revocation",
        "the group's membership change being applied",
        "DescribeUserGroups",
        &params(&[("UserGroupId", "app-group")]),
        |reply| !reply.all("UserIds/member").contains(&"read-only"),
    )
    .await;
    let mut revoked = open_tls("revoked", &address, &port).await;
    let answered = command(
        "revoked",
        &mut revoked,
        &["AUTH", "readonly", "the-read-only-password"],
    )
    .await;
    assert_eq!(
        answered.error_code(),
        Some("WRONGPASS"),
        "a user the group no longer holds must authenticate nobody, got {answered:?}"
    );
}

#[tokio::test]
async fn a_full_cache_makes_room_by_the_policy_its_parameter_group_names() {
    let emulator = Emulator::new();
    // The axis: the `maxmemory-policy` the cache's parameter group holds. A policy that
    // evicts makes room and keeps serving; `noeviction` answers the engine's own OOM
    // error. The values are two of the eight the `maxmemory-policy` parameter publishes
    // (`DescribeEngineDefaultParameters`' allowed values for it).
    let cases: [(&str, &str, bool); 3] = [
        ("evicts-lru", "allkeys-lru", true),
        ("evicts-random", "allkeys-random", true),
        ("refuses", "noeviction", false),
    ];
    for (id, policy, evicts) in cases {
        let group_name = format!("{id}-parameters");
        let created = emulator
            .call(
                "CreateCacheParameterGroup",
                &params(&[
                    ("CacheParameterGroupName", &group_name),
                    ("CacheParameterGroupFamily", "redis7"),
                    ("Description", "the policy under test"),
                ]),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {id}: the parameter group must be created, got {} {}",
            created.status, created.body
        );
        let modified = emulator
            .call(
                "ModifyCacheParameterGroup",
                &params(&[
                    ("CacheParameterGroupName", &group_name),
                    ("ParameterNameValues.ParameterNameValue.1.ParameterName", "maxmemory-policy"),
                    ("ParameterNameValues.ParameterNameValue.1.ParameterValue", policy),
                ]),
            )
            .await;
        assert_eq!(
            modified.status, 200,
            "case {id}: the policy must be set, got {} {}",
            modified.status, modified.body
        );
        let (address, port) = cluster_endpoint(
            &emulator,
            id,
            &[("Engine", "redis"), ("CacheParameterGroupName", &group_name)],
        )
        .await;
        let mut session = open(id, &address, &port).await;
        assert_eq!(
            command(id, &mut session, &["CONFIG", "GET", "maxmemory-policy"]).await,
            Resp::Array(vec![
                Resp::Bulk("maxmemory-policy".to_string()),
                Resp::Bulk(policy.to_string()),
            ]),
            "case {id}: a client reads back the policy the parameter group holds"
        );
        // Write past what one keyspace of this emulator may hold, in values of a size the
        // cache takes, until the cache either made room or refused.
        let value = "v".repeat(512 * 1024);
        let mut refusal = None;
        let mut written = 0;
        for index in 0..40 {
            let answered = command(id, &mut session, &["SET", &format!("key-{index}"), &value])
                .await;
            if let Some(code) = answered.error_code() {
                refusal = Some(code.to_string());
                break;
            }
            written += 1;
        }
        let held = command(id, &mut session, &["DBSIZE"]).await;
        let Resp::Integer(held) = held else {
            panic!("case {id}: DBSIZE must answer an integer, got {held:?}");
        };
        if evicts {
            assert_eq!(
                refusal, None,
                "case {id}: a cache that evicts keeps taking writes, got {refusal:?}"
            );
            assert!(
                held < written,
                "case {id}: a full cache under {policy} must have evicted something, it holds \
                 {held} of {written} keys"
            );
            assert_eq!(
                command(id, &mut session, &["SET", "after-eviction", "still-serving"]).await,
                Resp::Status("OK".to_string()),
                "case {id}: a cache that evicts goes on serving writes"
            );
        } else {
            assert_eq!(
                refusal.as_deref(),
                Some("OOM"),
                "case {id}: a full cache under {policy} must answer the engine's OOM error"
            );
            assert_eq!(
                held, written,
                "case {id}: a cache that evicts nothing holds every key it took"
            );
            // A read is still served.
            assert_eq!(
                command(id, &mut session, &["GET", "key-0"]).await.error_code(),
                None,
                "case {id}: a full cache still serves reads"
            );
        }
    }
}

/// Open a fresh TLS session on `address`:`port` and `AUTH` as `user`, looking again until
/// the reply's error code is `expected` — `None` for an accepted one — or a generous
/// deadline passes.
///
/// The one place this suite waits for a membership change to be in force on the data plane,
/// and it waits for the CONDITION rather than for a time: the control plane is NOT read in
/// between, so the only thing that can change a session's verdict is the data plane
/// materialising the change the clock owes the user group when the command arrives.
async fn auth_until(
    case: &str,
    address: &str,
    port: &str,
    user: &str,
    secret: &str,
    expected: Option<&str>,
    description: &str,
) -> Resp {
    let deadline = std::time::Instant::now() + SESSION_VERDICT_DEADLINE;
    loop {
        let mut session = open_tls(case, address, port).await;
        let answered = command(case, &mut session, &["AUTH", user, secret]).await;
        if answered.error_code() == expected {
            return answered;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case}: {description} never came about; the last AUTH as {user} answered \
             {answered:?}"
        );
        tokio::time::sleep(LOOK_AGAIN_AFTER_SESSION).await;
    }
}

/// How long [`auth_until`] looks for the verdict it waits on, and how often it looks again.
///
/// Generous on purpose: what it waits for is a change the clock makes due within a
/// transition span, so nothing here depends on how fast or how loaded the host is.
const SESSION_VERDICT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);
/// See [`SESSION_VERDICT_DEADLINE`].
const LOOK_AGAIN_AFTER_SESSION: std::time::Duration = std::time::Duration::from_millis(10);

#[tokio::test]
async fn a_user_group_change_decides_access_to_the_replication_group_it_is_attached_to() {
    let emulator = Emulator::new();

    // The axis: the FORM of the credential the user authenticates with — a password it
    // holds, and a user that requires none — because the rule is about the group's
    // MEMBERSHIP deciding access, whatever authenticates the member.
    type Member = (&'static str, &'static str, &'static str);
    let members: [Member; 2] = [
        ("a user with a password", "with-password", "the-members-password"),
        // A user that requires no password authenticates with any credential, so the one
        // sent below is not what lets it in: its membership is.
        ("a user that needs no password", "without-password", "anything"),
    ];
    for (case, id, _) in members {
        let mut pairs = params(&[
            ("UserId", id),
            ("UserName", id),
            ("Engine", "redis"),
            ("AccessString", "on ~* +@all"),
        ]);
        if id == "with-password" {
            pairs.extend(params(&[("Passwords.member.1", "the-members-password")]));
        } else {
            pairs.extend(params(&[("NoPasswordRequired", "true")]));
        }
        let created = emulator.call("CreateUser", &pairs).await;
        assert_eq!(
            created.status, 200,
            "case {case}: the user {id} must be created, got {} {}",
            created.status, created.body
        );
    }
    let group = emulator
        .call(
            "CreateUserGroup",
            &params(&[
                ("UserGroupId", "rg-group"),
                ("Engine", "redis"),
                ("UserIds.member.1", "default"),
                ("UserIds.member.2", "with-password"),
                ("UserIds.member.3", "without-password"),
            ]),
        )
        .await;
    assert_eq!(
        group.status, 200,
        "the user group must be created, got {} {}",
        group.status, group.body
    );
    poll_until(
        &emulator,
        REGION,
        "rg-group",
        "the user group reaching active",
        "DescribeUserGroups",
        &params(&[("UserGroupId", "rg-group")]),
        |reply| reply.text("member/Status") == Some("active"),
    )
    .await;

    // The replication group the group is attached to, with in-transit encryption on —
    // which is the wire a client of such a group speaks.
    let created = emulator
        .call(
            "CreateReplicationGroup",
            &params(&[
                ("ReplicationGroupId", "rbac-plane"),
                ("ReplicationGroupDescription", "a group behind a user group"),
                ("Engine", "redis"),
                ("CacheNodeType", "cache.t3.micro"),
                ("NumCacheClusters", "1"),
                ("TransitEncryptionEnabled", "true"),
            ]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the replication group must be created, got {} {}",
        created.status, created.body
    );
    poll_until(
        &emulator,
        REGION,
        "rbac-plane",
        "the replication group reaching available",
        "DescribeReplicationGroups",
        &params(&[("ReplicationGroupId", "rbac-plane")]),
        |reply| reply.text("ReplicationGroup/Status") == Some("available"),
    )
    .await;
    let attached = emulator
        .call(
            "ModifyReplicationGroup",
            &params(&[
                ("ReplicationGroupId", "rbac-plane"),
                ("UserGroupIdsToAdd.member.1", "rg-group"),
                ("ApplyImmediately", "true"),
            ]),
        )
        .await;
    assert_eq!(
        attached.status, 200,
        "the user group must be attachable, got {} {}",
        attached.status, attached.body
    );
    let described = poll_until(
        &emulator,
        REGION,
        "rbac-plane",
        "the replication group reporting the group attached and available again",
        "DescribeReplicationGroups",
        &params(&[("ReplicationGroupId", "rbac-plane")]),
        |reply| {
            reply.text("ReplicationGroup/Status") == Some("available")
                && reply.all("UserGroupIds/member").contains(&"rg-group")
        },
    )
    .await;
    let (address, port) = endpoint_of(
        &described,
        "NodeGroup/PrimaryEndpoint/Address",
        "NodeGroup/PrimaryEndpoint/Port",
        "the group",
    );

    // Each row: the member authenticates while the group holds it, is refused once the
    // group no longer does, and is accepted again when the group holds it once more — the
    // IDENTICAL credential every time, so what changed is the membership. NOTHING reads the
    // group through the control plane between the change and the session: what has to put
    // the change in force on the data plane is the data plane itself, materialising what
    // the clock owes the group when the command arrives.
    for (case, id, secret) in members {
        let accepted = auth_until(
            case,
            &address,
            &port,
            id,
            secret,
            None,
            "a member of the attached group authenticating",
        )
        .await;
        assert_eq!(
            accepted,
            Resp::Status("OK".to_string()),
            "case {case}: a member of the attached group must authenticate, got {accepted:?}"
        );

        let removed = emulator
            .call(
                "ModifyUserGroup",
                &params(&[
                    ("UserGroupId", "rg-group"),
                    ("UserIdsToRemove.member.1", id),
                ]),
            )
            .await;
        assert_eq!(
            removed.status, 200,
            "case {case}: the removal must be accepted, got {} {}",
            removed.status, removed.body
        );
        let refused = auth_until(
            case,
            &address,
            &port,
            id,
            secret,
            Some("WRONGPASS"),
            "the removed user's AUTH being refused",
        )
        .await;
        assert_eq!(
            refused.error_code(),
            Some("WRONGPASS"),
            "case {case}: a user the attached group no longer holds must authenticate \
             nobody, got {refused:?}"
        );
        // The revocation is this user's own: the other member of the group still reaches
        // the cache with its own credential.
        let (other_case, other_id, other_secret) = members
            .iter()
            .find(|(_, other, _)| *other != id)
            .copied()
            .expect("the table holds a second member");
        let mut other = open_tls(other_case, &address, &port).await;
        let still_accepted = command(other_case, &mut other, &["AUTH", other_id, other_secret])
            .await;
        assert_eq!(
            still_accepted,
            Resp::Status("OK".to_string()),
            "case {case}: removing one member must leave {other_id} its access, got \
             {still_accepted:?}"
        );

        let restored = emulator
            .call(
                "ModifyUserGroup",
                &params(&[("UserGroupId", "rg-group"), ("UserIdsToAdd.member.1", id)]),
            )
            .await;
        assert_eq!(
            restored.status, 200,
            "case {case}: the restore must be accepted, got {} {}",
            restored.status, restored.body
        );
        let reaccepted = auth_until(
            case,
            &address,
            &port,
            id,
            secret,
            None,
            "the restored user's identical AUTH being accepted again",
        )
        .await;
        assert_eq!(
            reaccepted,
            Resp::Status("OK".to_string()),
            "case {case}: the identical credential must be accepted once the group holds \
             the user again, got {reaccepted:?}"
        );

        // And the control plane agrees about the membership the sessions were judged
        // against, read AFTER the data plane has already shown it.
        let described = emulator
            .call("DescribeUserGroups", &params(&[("UserGroupId", "rg-group")]))
            .await;
        assert!(
            described.all("UserIds/member").contains(&id),
            "case {case}: the group the sessions were judged against must hold {id}, got {}",
            described.body
        );
    }
}

#[tokio::test]
async fn the_socket_a_cache_was_served_on_closes_with_the_cache() {
    let emulator = Emulator::new();
    let (address, port) = cluster_endpoint(&emulator, "leaving", &[("Engine", "redis")]).await;
    let mut session = open("leaving", &address, &port).await;
    assert_eq!(
        command("leaving", &mut session, &["PING"]).await,
        Resp::Status("PONG".to_string()),
        "the cache must answer while it is there"
    );
    let deleted = emulator
        .call("DeleteCacheCluster", &params(&[("CacheClusterId", "leaving")]))
        .await;
    assert_eq!(
        deleted.status, 200,
        "the delete must be accepted, got {} {}",
        deleted.status, deleted.body
    );
    // The port goes with the cache: a client dialling the address it published afterwards
    // reaches nothing. The listener is closed by the delete, so this is waited for by
    // looking again rather than by assuming it has happened by now.
    let dialled: u16 = port.parse().expect("the published port is a port");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        match TcpStream::connect(("127.0.0.1", dialled)).await {
            Err(_) => break,
            Ok(_) => assert!(
                std::time::Instant::now() < deadline,
                "the socket of a deleted cache at {address}:{dialled} must stop accepting \
                 connections"
            ),
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}
