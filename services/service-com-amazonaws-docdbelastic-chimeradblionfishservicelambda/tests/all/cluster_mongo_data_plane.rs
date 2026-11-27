// SPDX-License-Identifier: CC0-1.0

//! The DATA PLANE of an elastic cluster: that the `clusterEndpoint` the control plane
//! publishes is an address a client of THIS emulator opens a MongoDB session on, and that the
//! session authenticates, shards a collection and reads back the documents it wrote.
//!
//! The rule under test: `Cluster$clusterEndpoint` is "The URL used to connect to the elastic
//! cluster", so it is an address the cluster ANSWERS on — one of this emulator's own, with the
//! port a client dials — and not a name in Amazon DocumentDB's own DNS zone, which resolves to
//! nothing on the machine this emulator runs on. The tables below span what the rule
//! generalises over: which cluster is asked (several, each with its own administrator,
//! password and shape), which operation published the address (the create and every later
//! read), which shard key a collection is sharded on, which commands the session sends, and
//! which way a session fails to authenticate.
//!
//! Every assertion goes over a real socket the core opened for the cluster, dialled at the
//! address the control plane published; nothing reaches into the service's state.

use std::sync::Arc;

use ::bson::{doc, Bson, Document};
use ::serde_json::{json, Value};
use ::smithy_server_runtime::MultiServiceRouter;
use ::tokio::io::{AsyncReadExt, AsyncWriteExt};
use ::tokio::net::TcpStream;

/// The region the suite signs with: the credential scope is what routes the request and what
/// the handlers read the region from.
const REGION: &str = "eu-west-2";

/// The administrator a cluster under test is created with.
const ADMIN_USER: &str = "clusteradmin";

/// Its password, which only this suite and the credential derived from it ever see.
const ADMIN_PASSWORD: &str = "a-Secret-99";

/// `OP_MSG`, the message a driver sends every command in.
const OP_MSG: i32 = 2013;

/// The mechanism an elastic cluster authenticates a client with.
const SCRAM_SHA_1: &str = "SCRAM-SHA-1";

/// One emulator: the shared engine plus a router carrying only this service.
struct Emulator {
    router: MultiServiceRouter,
    /// The ONE shared engine the service was built on, so the suite can reach the register of
    /// the data planes this emulator serves in process — where a client that can open no host
    /// socket resolves a published address.
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests exercise handler and data-plane behaviour, not SigV4 verification.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_docdbelastic_chimeradblionfishservicelambda::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// Send one request and return its status and parsed JSON body.
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let payload = body.map(|body| body.to_string()).unwrap_or_default();
        let request = ::http::Request::builder()
            .method(method)
            .uri(format!("https://docdb-elastic.{REGION}.amazonaws.com{path}"))
            .header("host", format!("docdb-elastic.{REGION}.amazonaws.com"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}/docdb-elastic/\
                     aws4_request, SignedHeaders=host;x-amz-date, Signature=deadbeef"
                ),
            )
            .header("content-type", "application/json")
            .body(::bytes::Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = match bytes.is_empty() {
            true => Value::Null,
            false => ::serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "{method} {path}: the body must be JSON ({error}): {}",
                    String::from_utf8_lossy(&bytes)
                )
            }),
        };
        (status, parsed)
    }

    /// Create one cluster and answer the `cluster` the create reported.
    async fn create_cluster(&self, case: &str, name: &str, admin: &str, password: &str) -> Value {
        let (status, body) = self
            .call(
                "POST",
                "/cluster",
                Some(json!({
                    "clusterName": name,
                    "authType": "PLAIN_TEXT",
                    "adminUserName": admin,
                    "adminUserPassword": password,
                    "shardCapacity": 2,
                    "shardCount": 1,
                })),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected CreateCluster to answer 200, got {status} with {body}"
        );
        body.get("cluster")
            .cloned()
            .unwrap_or_else(|| panic!("case {case}: the create answers a cluster, got {body}"))
    }

    /// Read one cluster back, as a client polling for it does.
    async fn get_cluster(&self, case: &str, cluster_arn: &str) -> Value {
        let (status, body) = self
            .call("GET", &format!("/cluster/{}", label(cluster_arn)), None)
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected GetCluster to answer 200, got {status} with {body}"
        );
        body.get("cluster")
            .cloned()
            .unwrap_or_else(|| panic!("case {case}: the read answers a cluster, got {body}"))
    }
}

/// Percent-encode one `@httpLabel` value the way an AWS SDK does: a label is a single path
/// segment and an ARN carries both `:` and `/`, so neither may survive raw.
fn label(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// One member of a cluster projection, as a string.
fn member(case: &str, cluster: &Value, name: &str) -> String {
    cluster
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: the cluster carries {name}, got {cluster}"))
        .to_string()
}

/// The host and the port a published `clusterEndpoint` names.
fn host_and_port(case: &str, endpoint: &str) -> (String, u16) {
    let (host, port) = endpoint.rsplit_once(':').unwrap_or_else(|| {
        panic!(
            "case {case}: the published clusterEndpoint must carry the port a client dials, \
             got {endpoint:?}"
        )
    });
    let port: u16 = port.parse().unwrap_or_else(|_| {
        panic!("case {case}: the published port must be a port, got {endpoint:?}")
    });
    (host.to_string(), port)
}

/// A byte stream a session runs on: the TLS stream over the socket.
type Stream = Box<dyn Transport>;

/// What a session needs of its connection.
trait Transport: ::tokio::io::AsyncRead + ::tokio::io::AsyncWrite + Unpin + Send {}
impl<T: ::tokio::io::AsyncRead + ::tokio::io::AsyncWrite + Unpin + Send> Transport for T {}

/// One client session on a cluster's endpoint.
struct Session {
    stream: Stream,
    request_id: i32,
}

impl Session {
    /// Open a session on the address the control plane published — over a REAL socket, as any
    /// client of this machine dials it, and over TLS, which is the wire an elastic cluster's
    /// endpoint is served on.
    ///
    /// The register of the planes this emulator serves in process holds that same address
    /// (`service_common::dialable_data_planes`), asserted here too: a client that can open no
    /// host socket — the code of a function attached to the cluster's virtual private cloud,
    /// which is the vantage point such an endpoint is reachable from — reaches the same server
    /// at the same address.
    async fn open(emulator: &Emulator, case: &str, endpoint: &str) -> Self {
        let (host, port) = host_and_port(case, endpoint);
        let planes = ::service_common::dialable_data_planes::DialablePlanes::of(&*emulator.engine)
            .expect("the engine vends the register of served data planes");
        let served = planes.served_by(endpoint);
        assert_eq!(
            served.as_deref(),
            Some("docdb-elastic"),
            "case {case}: the published endpoint {endpoint} must be open as an address this \
             service serves a cluster at, got {served:?}"
        );
        let published: std::net::IpAddr = host.trim_matches(['[', ']']).parse().unwrap_or_else(
            |_| panic!("case {case}: the published host must be an address, got {host:?}"),
        );
        assert!(
            published.is_loopback(),
            "case {case}: the published host must be this machine's loopback address, got \
             {host:?}"
        );
        let plain = match published {
            std::net::IpAddr::V4(_) => {
                TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).await
            }
            std::net::IpAddr::V6(_) => {
                TcpStream::connect((std::net::Ipv6Addr::LOCALHOST, port)).await
            }
        }
        .unwrap_or_else(|error| {
            panic!("case {case}: {endpoint} must accept a connection, got {error}")
        });
        let provider = Arc::new(::rustls::crypto::aws_lc_rs::default_provider());
        let config = ::rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
            .with_safe_default_protocol_versions()
            .expect("the provider supports the default protocol versions")
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptTheEmulatorsCertificate(provider)))
            .with_no_client_auth();
        let connector = ::tokio_rustls::TlsConnector::from(Arc::new(config));
        let server_name = ::rustls::pki_types::ServerName::try_from("localhost")
            .expect("localhost is a server name");
        let stream = connector
            .connect(server_name, plain)
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "case {case}: the cluster's socket must complete a TLS handshake, got {error}"
                )
            });
        Self {
            stream: Box::new(stream),
            request_id: 1,
        }
    }

    /// Send one command as an `OP_MSG`, the form every command of a driver travels in, and read
    /// the reply document.
    async fn command(&mut self, case: &str, database: &str, command: Document) -> Document {
        let mut command = command;
        command.insert("$db".to_string(), database);
        let mut body: Vec<u8> = Vec::new();
        body.extend_from_slice(&0_u32.to_le_bytes());
        body.push(0);
        command
            .to_writer(&mut body)
            .expect("the command document encodes");
        let length = i32::try_from(16 + body.len()).expect("the message is small");
        let mut message: Vec<u8> = Vec::new();
        message.extend_from_slice(&length.to_le_bytes());
        message.extend_from_slice(&self.request_id.to_le_bytes());
        message.extend_from_slice(&0_i32.to_le_bytes());
        message.extend_from_slice(&OP_MSG.to_le_bytes());
        message.extend_from_slice(&body);
        self.request_id += 1;
        self.stream
            .write_all(&message)
            .await
            .unwrap_or_else(|error| panic!("case {case}: the command must be sent, got {error}"));
        self.stream.flush().await.unwrap_or_else(|error| {
            panic!("case {case}: the command must be flushed, got {error}")
        });
        let mut header = [0u8; 16];
        self.stream
            .read_exact(&mut header)
            .await
            .unwrap_or_else(|error| {
                panic!("case {case}: the endpoint must answer with a message, got {error}")
            });
        let reply_length = i32::from_le_bytes([header[0], header[1], header[2], header[3]]);
        let reply_length = usize::try_from(reply_length).unwrap_or_else(|_| {
            panic!("case {case}: the reply names a length, got {reply_length}")
        });
        assert!(
            (16..=48_000_000).contains(&reply_length),
            "case {case}: the reply length must be a message length, got {reply_length}"
        );
        let mut reply = vec![0u8; reply_length - 16];
        self.stream
            .read_exact(&mut reply)
            .await
            .unwrap_or_else(|error| panic!("case {case}: the whole reply must arrive, got {error}"));
        // An `OP_MSG` body is four flag bytes and one section-kind byte before its document.
        let document = reply
            .get(5..)
            .unwrap_or_else(|| panic!("case {case}: the reply carries a document"));
        Document::from_reader(&mut std::io::Cursor::new(document)).unwrap_or_else(|error| {
            panic!("case {case}: the reply must be a BSON document, got {error}")
        })
    }

    /// Whether the endpoint has CLOSED this connection: a further command cannot be exchanged
    /// on it at all, because the stream is at its end.
    async fn is_closed(&mut self, case: &str) -> bool {
        let _ = case;
        let mut command = doc! { "ping": 1_i32 };
        command.insert("$db".to_string(), "admin");
        let mut body: Vec<u8> = Vec::new();
        body.extend_from_slice(&0_u32.to_le_bytes());
        body.push(0);
        command
            .to_writer(&mut body)
            .expect("the command document encodes");
        let length = i32::try_from(16 + body.len()).expect("the message is small");
        let mut message: Vec<u8> = Vec::new();
        message.extend_from_slice(&length.to_le_bytes());
        message.extend_from_slice(&self.request_id.to_le_bytes());
        message.extend_from_slice(&0_i32.to_le_bytes());
        message.extend_from_slice(&OP_MSG.to_le_bytes());
        message.extend_from_slice(&body);
        self.request_id += 1;
        if self.stream.write_all(&message).await.is_err() || self.stream.flush().await.is_err() {
            return true;
        }
        let mut header = [0u8; 16];
        self.stream.read_exact(&mut header).await.is_err()
    }

    /// Authenticate as `username` with `password` over `mechanism`, the exchange a driver runs,
    /// and answer the reply of its last step.
    async fn authenticate(
        &mut self,
        case: &str,
        mechanism: &str,
        username: &str,
        password: &str,
    ) -> Document {
        let client_nonce = "Y2xpZW50bm9uY2U5OQ==";
        let first_bare = format!("n={username},r={client_nonce}");
        let started = self
            .command(
                case,
                "admin",
                doc! {
                    "saslStart": 1_i32,
                    "mechanism": mechanism,
                    "payload": binary(format!("n,,{first_bare}").into_bytes()),
                    "options": { "skipEmptyExchange": true },
                },
            )
            .await;
        if ok(&started) != 1.0 {
            return started;
        }
        let conversation_id = started
            .get_i32("conversationId")
            .unwrap_or_else(|_| panic!("case {case}: saslStart answers a conversation id"));
        let server_first = String::from_utf8(payload_of(&started))
            .unwrap_or_else(|_| panic!("case {case}: the server-first message is text"));
        let salt = ::aws_smithy_types::base64::decode(attribute(&server_first, 's'))
            .unwrap_or_else(|_| panic!("case {case}: the salt is base64"));
        let iterations: u32 = attribute(&server_first, 'i')
            .parse()
            .unwrap_or_else(|_| panic!("case {case}: the iteration count is a number"));
        let combined_nonce = attribute(&server_first, 'r').to_string();
        // RFC 5802 over SHA-1, with the password prepared as the engine's own SCRAM-SHA-1
        // credential prepares it: the hex MD5 digest of `<user>:mongo:<password>`. The proof is
        // the client key masked with the signature over the whole conversation, so only a
        // client holding the password can produce it.
        let prepared = {
            use ::md5::Digest;
            ::hex::encode(::md5::Md5::digest(
                format!("{username}:mongo:{password}").as_bytes(),
            ))
        };
        let mut salted = [0u8; 20];
        ::pbkdf2::pbkdf2_hmac::<::sha1::Sha1>(
            prepared.as_bytes(),
            &salt,
            iterations,
            &mut salted,
        );
        let client_key = hmac_sha_1(&salted, b"Client Key");
        let stored_key = {
            use ::sha1::Digest;
            ::sha1::Sha1::digest(&client_key).to_vec()
        };
        let without_proof = format!("c=biws,r={combined_nonce}");
        let auth_message = format!("{first_bare},{server_first},{without_proof}");
        let signature = hmac_sha_1(&stored_key, auth_message.as_bytes());
        let proof: Vec<u8> = client_key
            .iter()
            .zip(signature.iter())
            .map(|(key, sign)| key ^ sign)
            .collect();
        let client_final = format!(
            "{without_proof},p={}",
            ::aws_smithy_types::base64::encode(&proof)
        );
        self.command(
            case,
            "admin",
            doc! {
                "saslContinue": 1_i32,
                "conversationId": conversation_id,
                "payload": binary(client_final.into_bytes()),
            },
        )
        .await
    }
}

/// A SASL payload, as a driver sends one.
fn binary(bytes: Vec<u8>) -> Bson {
    Bson::Binary(::bson::Binary {
        subtype: ::bson::spec::BinarySubtype::Generic,
        bytes,
    })
}

/// The payload a SASL reply carries.
fn payload_of(reply: &Document) -> Vec<u8> {
    match reply.get("payload") {
        Some(Bson::Binary(binary)) => binary.bytes.clone(),
        other => panic!("expected a binary SASL payload, got {other:?}"),
    }
}

/// The value of one attribute of a SCRAM message.
fn attribute(message: &str, key: char) -> &str {
    message
        .split(',')
        .filter_map(|field| field.split_once('='))
        .find(|(name, _)| name.len() == 1 && name.starts_with(key))
        .map(|(_, value)| value)
        .unwrap_or_else(|| panic!("expected a {key}= attribute in {message:?}"))
}

fn hmac_sha_1(key: &[u8], data: &[u8]) -> Vec<u8> {
    use ::hmac::digest::KeyInit;
    use ::hmac::Mac;
    let mut mac = <::hmac::Hmac<::sha1::Sha1> as KeyInit>::new_from_slice(key)
        .expect("HMAC-SHA-1 takes a key of any length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// The `ok` of a reply, which every reply carries.
fn ok(reply: &Document) -> f64 {
    match reply.get("ok") {
        Some(Bson::Double(value)) => *value,
        Some(Bson::Int32(value)) => f64::from(*value),
        Some(Bson::Int64(value)) => *value as f64,
        other => panic!("expected an ok in the reply, got {other:?}"),
    }
}

/// The `code` of a refusal.
fn code(reply: &Document) -> i32 {
    reply
        .get_i32("code")
        .unwrap_or_else(|_| panic!("expected a refusal carrying a code, got {reply:?}"))
}

/// The `errmsg` of a refusal.
fn errmsg(reply: &Document) -> String {
    reply
        .get_str("errmsg")
        .unwrap_or_else(|_| panic!("expected a refusal carrying an errmsg, got {reply:?}"))
        .to_string()
}

/// The documents the first batch of a cursor reply carries.
fn first_batch(case: &str, reply: &Document) -> Vec<Document> {
    let cursor = reply
        .get_document("cursor")
        .unwrap_or_else(|_| panic!("case {case}: a read answers a cursor, got {reply:?}"));
    cursor
        .get_array("firstBatch")
        .unwrap_or_else(|_| panic!("case {case}: the cursor carries a first batch, got {cursor:?}"))
        .iter()
        .filter_map(|value| match value {
            Bson::Document(document) => Some(document.clone()),
            _ => None,
        })
        .collect()
}

/// The verifier a client of the emulator's OWN socket uses: the certificate is the one the core
/// generated for its listeners, self-signed and naming loopback, and this client is the
/// emulator's own test connecting to a port the same process opened — so there is no authority
/// to check it against.
#[derive(Debug)]
struct AcceptTheEmulatorsCertificate(Arc<::rustls::crypto::CryptoProvider>);

impl ::rustls::client::danger::ServerCertVerifier for AcceptTheEmulatorsCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &::rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[::rustls::pki_types::CertificateDer<'_>],
        _server_name: &::rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: ::rustls::pki_types::UnixTime,
    ) -> Result<::rustls::client::danger::ServerCertVerified, ::rustls::Error> {
        Ok(::rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &::rustls::pki_types::CertificateDer<'_>,
        dss: &::rustls::DigitallySignedStruct,
    ) -> Result<::rustls::client::danger::HandshakeSignatureValid, ::rustls::Error> {
        ::rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &::rustls::pki_types::CertificateDer<'_>,
        dss: &::rustls::DigitallySignedStruct,
    ) -> Result<::rustls::client::danger::HandshakeSignatureValid, ::rustls::Error> {
        ::rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<::rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// RULE: the `clusterEndpoint` a cluster publishes is an address of THIS emulator, carrying the
/// port a client dials, every read of that cluster reports the same one, and a session opened
/// on it is answered by the cluster's own server.
///
/// The rows span the clusters a caller may create — each has an endpoint of its own, because
/// each has a socket of its own — and each row is checked on both paths the address is
/// published on: the create's own answer and a later read.
#[::tokio::test(flavor = "multi_thread")]
async fn the_published_cluster_endpoint_is_an_address_this_emulator_answers_on() {
    let emulator = Emulator::new();
    let rows: Vec<(&str, &str, &str, &str)> = vec![
        ("the first cluster", "orders-elastic", ADMIN_USER, ADMIN_PASSWORD),
        ("a second cluster, with its own administrator", "events-elastic", "eventsadmin", "An0ther-Secret"),
    ];
    let mut published: Vec<String> = Vec::new();
    for (case, name, admin, password) in rows {
        let created = emulator.create_cluster(case, name, admin, password).await;
        let cluster_arn = member(case, &created, "clusterArn");
        let endpoint = member(case, &created, "clusterEndpoint");
        assert!(
            !endpoint.contains("amazonaws.com"),
            "case {case}: the published endpoint must be an address of this emulator, got \
             {endpoint:?}"
        );
        let read = emulator.get_cluster(case, &cluster_arn).await;
        let read_endpoint = member(case, &read, "clusterEndpoint");
        assert_eq!(
            read_endpoint, endpoint,
            "case {case}: expected every read of the cluster to report the address the create \
             published ({endpoint}), got {read_endpoint}"
        );
        // A socket of its own per cluster, so no two clusters are published at one address.
        assert!(
            !published.contains(&endpoint),
            "case {case}: expected an endpoint of its own, got {endpoint} again (already \
             published: {published:?})"
        );
        published.push(endpoint.clone());
        let mut session = Session::open(&emulator, case, &endpoint).await;
        let hello = session
            .command(case, "admin", doc! { "hello": 1_i32, "helloOk": true })
            .await;
        assert_eq!(
            ok(&hello),
            1.0,
            "case {case}: expected the handshake to be answered, got {hello:?}"
        );
        assert_eq!(
            hello.get_bool("isWritablePrimary").ok(),
            Some(true),
            "case {case}: expected the endpoint to answer as the writable one, got {hello:?}"
        );
        // An elastic cluster distributes its collections over its shards, which is the topology
        // a driver reads out of the handshake.
        assert_eq!(
            hello.get_str("msg").ok(),
            Some("isdbgrid"),
            "case {case}: expected the handshake to report a sharded topology, got {hello:?}"
        );
    }
}

/// RULE: a collection sharded on one hashed field round-trips the documents written to it,
/// selected by that field — whatever the field is named and whatever its values are.
///
/// The rows span what the rule generalises over: the shard key's own form (a plain field, a
/// field inside a document), the type of the values it carries (text, numbers), and how many
/// documents one write stores.
#[::tokio::test(flavor = "multi_thread")]
async fn a_sharded_collection_round_trips_documents_by_their_shard_key() {
    let emulator = Emulator::new();
    let created = emulator
        .create_cluster("sharded round trip", "shop-elastic", ADMIN_USER, ADMIN_PASSWORD)
        .await;
    let endpoint = member("sharded round trip", &created, "clusterEndpoint");
    let mut session = Session::open(&emulator, "sharded round trip", &endpoint).await;
    let admitted = session
        .authenticate("sharded round trip", SCRAM_SHA_1, ADMIN_USER, ADMIN_PASSWORD)
        .await;
    assert_eq!(
        ok(&admitted),
        1.0,
        "sharded round trip: the administrator must be able to authenticate, got {admitted:?}"
    );
    let rows: Vec<(&str, &str, &str, Vec<Document>, Bson)> = vec![
        (
            "a text shard key",
            "orders",
            "tenant",
            vec![
                doc! { "tenant": "acme", "total": 12_i32 },
                doc! { "tenant": "globex", "total": 7_i32 },
            ],
            Bson::String("acme".to_string()),
        ),
        (
            "a numeric shard key",
            "readings",
            "sensor",
            vec![
                doc! { "sensor": 1_i32, "celsius": 19_i32 },
                doc! { "sensor": 2_i32, "celsius": 21_i32 },
                doc! { "sensor": 1_i32, "celsius": 20_i32 },
            ],
            Bson::Int32(1),
        ),
        (
            "a shard key inside a document",
            "events",
            "owner.tenant",
            vec![doc! { "owner": { "tenant": "acme" }, "kind": "signup" }],
            Bson::String("acme".to_string()),
        ),
    ];
    for (case, collection, field, documents, selected) in rows {
        let namespace = format!("shop.{collection}");
        let sharded = session
            .command(
                case,
                "admin",
                doc! { "shardCollection": namespace.clone(), "key": { field: "hashed" } },
            )
            .await;
        assert_eq!(
            ok(&sharded),
            1.0,
            "case {case}: expected {namespace} to be sharded on {field}, got {sharded:?}"
        );
        assert_eq!(
            sharded.get_str("collectionsharded").ok(),
            Some(namespace.as_str()),
            "case {case}: expected the reply to name the sharded namespace, got {sharded:?}"
        );
        let expected = documents.len();
        let inserted = session
            .command(
                case,
                "shop",
                doc! { "insert": collection, "documents": documents.clone() },
            )
            .await;
        assert_eq!(
            ok(&inserted),
            1.0,
            "case {case}: expected the write to be stored, got {inserted:?}"
        );
        assert_eq!(
            inserted.get_i32("n").ok(),
            Some(i32::try_from(expected).expect("the row is small")),
            "case {case}: expected every document of the write to be stored, got {inserted:?}"
        );
        let found = session
            .command(
                case,
                "shop",
                doc! { "find": collection, "filter": { field: selected.clone() } },
            )
            .await;
        assert_eq!(
            ok(&found),
            1.0,
            "case {case}: expected the read to be answered, got {found:?}"
        );
        let batch = first_batch(case, &found);
        let wanted: Vec<Document> = documents
            .iter()
            .filter(|document| value_at(document, field) == Some(selected.clone()))
            .cloned()
            .collect();
        assert_eq!(
            batch.len(),
            wanted.len(),
            "case {case}: expected the documents carrying {selected:?} at {field} back, got \
             {batch:?}"
        );
        for document in &wanted {
            assert!(
                batch.iter().any(|read| document
                    .iter()
                    .all(|(name, value)| read.get(name) == Some(value))),
                "case {case}: expected {document:?} to be read back, got {batch:?}"
            );
        }
    }
}

/// The value one document carries at `path`, which may name a field inside a document.
fn value_at(document: &Document, path: &str) -> Option<Bson> {
    let mut value = Bson::Document(document.clone());
    for segment in path.split('.') {
        value = value.as_document()?.get(segment)?.clone();
    }
    Some(value)
}

/// RULE: a collection of an elastic cluster is CREATED sharded on exactly one hashed field, so
/// `shardCollection` refuses — as a create that did not happen, in fixed text carrying no part
/// of the request and no `codeName` — a key that does not name exactly one field and a key whose
/// one field names an order instead of the hash, and refuses a namespace the cluster already
/// holds as a collection that already exists.
///
/// The rows span what the rule generalises over for the KEY: how many fields it names, which
/// sharding strategy each names, and the field's own form; and for the NAMESPACE: a collection
/// a client sharded, re-sharded on the same key and on another, and one an ordinary write
/// brought into being.
#[::tokio::test(flavor = "multi_thread")]
async fn a_shard_key_is_one_hashed_field_and_a_collection_is_sharded_once() {
    let emulator = Emulator::new();
    let created = emulator
        .create_cluster("shard key rules", "keys-elastic", ADMIN_USER, ADMIN_PASSWORD)
        .await;
    let endpoint = member("shard key rules", &created, "clusterEndpoint");
    let mut session = Session::open(&emulator, "shard key rules", &endpoint).await;
    session
        .authenticate("shard key rules", SCRAM_SHA_1, ADMIN_USER, ADMIN_PASSWORD)
        .await;
    // The two sentences an elastic cluster words a shard key it will not create a collection on
    // with: both FIXED text, carrying nothing of the request.
    let one_field = "Create collection failed. Must have exactly one Shard Key";
    let not_hashed = "Create collection failed. Shard Key type must be hashed not range based";
    let keys: Vec<(&str, Document, Option<&str>)> = vec![
        ("one hashed field", doc! { "tenant": "hashed" }, None),
        (
            "one hashed field named by a path",
            doc! { "owner.tenant": "hashed" },
            None,
        ),
        (
            "an ascending field",
            doc! { "tenant": 1_i32 },
            Some(not_hashed),
        ),
        (
            "a descending field",
            doc! { "tenant": -1_i32 },
            Some(not_hashed),
        ),
        (
            "a strategy spelled otherwise",
            doc! { "tenant": "HASHED" },
            Some(not_hashed),
        ),
        (
            "two hashed fields",
            doc! { "tenant": "hashed", "region": "hashed" },
            Some(one_field),
        ),
        (
            "a hashed field beside a range one",
            doc! { "tenant": "hashed", "region": 1_i32 },
            Some(one_field),
        ),
        ("no field at all", Document::new(), Some(one_field)),
    ];
    for (index, (case, key, refused)) in keys.into_iter().enumerate() {
        let namespace = format!("shop.keyed{index}");
        let reply = session
            .command(
                case,
                "admin",
                doc! { "shardCollection": namespace.clone(), "key": key.clone() },
            )
            .await;
        match refused {
            None => {
                assert_eq!(
                    ok(&reply),
                    1.0,
                    "case {case}: expected {key:?} to be admitted as a shard key, got {reply:?}"
                );
                assert_eq!(
                    reply.get_str("collectionsharded").ok(),
                    Some(namespace.as_str()),
                    "case {case}: expected the reply to name the sharded namespace, got {reply:?}"
                );
            }
            Some(sentence) => {
                assert_eq!(
                    ok(&reply),
                    0.0,
                    "case {case}: expected {key:?} to be refused as a shard key, got {reply:?}"
                );
                assert_eq!(
                    code(&reply),
                    61,
                    "case {case}: expected the refusal of {key:?} to carry the engine's shard \
                     key code, got {reply:?}"
                );
                // The whole sentence, so a change to the wording is a failing test rather than
                // a silent divergence.
                assert_eq!(
                    errmsg(&reply),
                    sentence,
                    "case {case}: expected the shard-key refusal's sentence, got {reply:?}"
                );
                assert!(
                    !reply.contains_key("codeName"),
                    "case {case}: expected the refusal to carry the code alone, got {reply:?}"
                );
                // The create did not happen, so the namespace is still one a client can shard:
                // a refused key leaves no collection behind.
                let shardable = session
                    .command(
                        case,
                        "admin",
                        doc! {
                            "shardCollection": namespace.clone(),
                            "key": { "tenant": "hashed" },
                        },
                    )
                    .await;
                assert_eq!(
                    ok(&shardable),
                    1.0,
                    "case {case}: expected {namespace} to be left unsharded by the refusal, got \
                     {shardable:?}"
                );
            }
        }
    }
    // A collection an ordinary write brought into being is as much a collection the cluster
    // holds as one a client sharded.
    session
        .command(
            "an unsharded write",
            "shop",
            doc! { "insert": "carts", "documents": [ doc! { "total": 5_i32 } ] },
        )
        .await;
    let held: Vec<(&str, &str, Document)> = vec![
        (
            "a collection sharded on the same key again",
            "shop.keyed0",
            doc! { "tenant": "hashed" },
        ),
        (
            "a collection sharded on another key",
            "shop.keyed0",
            doc! { "region": "hashed" },
        ),
        (
            "a collection an ordinary write created",
            "shop.carts",
            doc! { "tenant": "hashed" },
        ),
    ];
    for (case, namespace, key) in held {
        let reply = session
            .command(
                case,
                "admin",
                doc! { "shardCollection": namespace, "key": key },
            )
            .await;
        assert_eq!(
            ok(&reply),
            0.0,
            "case {case}: expected {namespace} to be refused as already held, got {reply:?}"
        );
        assert_eq!(
            code(&reply),
            48,
            "case {case}: expected the refusal to carry the exists code, got {reply:?}"
        );
        assert_eq!(
            errmsg(&reply),
            format!("a collection '{namespace}' already exists"),
            "case {case}: expected the sentence to name the namespace, got {reply:?}"
        );
        assert!(
            !reply.contains_key("codeName"),
            "case {case}: expected the refusal to carry the code alone, got {reply:?}"
        );
    }
}

/// One row of the write table: the case, the collection it writes to, the documents it carries,
/// how many of them are stored, and the index and code of each write error it is answered with.
type WriteRow = (
    &'static str,
    &'static str,
    Vec<Document>,
    i32,
    Vec<(i32, i32)>,
);

/// One row of the statement table: the case, whether the statement upserts, whether the write
/// can be replayed, its filter, its change, and the code and sentence it is refused with.
type StatementRow = (
    &'static str,
    bool,
    bool,
    Document,
    Document,
    Option<(i32, &'static str)>,
);

/// RULE: every entry of a write of a SHARDED collection is judged against that collection's
/// shard key — a document carries a value at it, an upserting statement's filter pins it, and no
/// statement alters it on a document the collection already holds — and an entry refused is
/// reported as that entry's write error of a command that otherwise ran, with the entries after
/// it left undone when the write is ordered.
///
/// The rows of the first table span the WRITE: a document carrying the key, one missing it, one
/// of several missing it at either end, and a write to a collection nobody sharded. The second
/// spans the STATEMENT: where the key value would come from, and each way a change would alter
/// the key (a modifier, an unset, a whole replacement).
#[::tokio::test(flavor = "multi_thread")]
async fn every_entry_of_a_sharded_write_is_judged_against_the_key() {
    let emulator = Emulator::new();
    let created = emulator
        .create_cluster("sharded writes", "writes-elastic", ADMIN_USER, ADMIN_PASSWORD)
        .await;
    let endpoint = member("sharded writes", &created, "clusterEndpoint");
    let mut session = Session::open(&emulator, "sharded writes", &endpoint).await;
    session
        .authenticate("sharded writes", SCRAM_SHA_1, ADMIN_USER, ADMIN_PASSWORD)
        .await;
    let sharded = session
        .command(
            "sharded writes",
            "admin",
            doc! { "shardCollection": "shop.keyed", "key": { "tenant": "hashed" } },
        )
        .await;
    assert_eq!(
        ok(&sharded),
        1.0,
        "sharded writes: the collection under test must be sharded, got {sharded:?}"
    );
    // `shop.keyed` is sharded on `tenant`; an unsharded collection of the same cluster is not
    // judged at all. Each row says how many documents the write stores and which entries it is
    // answered with write errors about.
    //
    // The sentences an elastic cluster words these refusals with, each FIXED text carrying no
    // part of the request.
    let no_shard_key = "Insert for sharded collection must have shardKey";
    let not_targetable = "Failed to target upsert by query :: could not extract exact shard key";
    let not_replayable = "Must run update to shard key field in a multi-statement transaction \
                          or with retryWrites: true.";
    let writes: Vec<WriteRow> = vec![
        (
            "a document carrying the shard key",
            "keyed",
            vec![doc! { "tenant": "acme", "total": 1_i32 }],
            1,
            vec![],
        ),
        (
            "a document missing the shard key",
            "keyed",
            vec![doc! { "total": 2_i32 }],
            0,
            vec![(0, 61)],
        ),
        (
            "the last document of several missing it",
            "keyed",
            vec![
                doc! { "tenant": "globex", "total": 3_i32 },
                doc! { "total": 4_i32 },
            ],
            1,
            vec![(1, 61)],
        ),
        (
            "the first document of several missing it",
            "keyed",
            vec![
                doc! { "total": 5_i32 },
                doc! { "tenant": "initech", "total": 6_i32 },
            ],
            0,
            vec![(0, 61)],
        ),
        (
            "a document with no shard key, in an unsharded collection",
            "carts",
            vec![doc! { "total": 7_i32 }],
            1,
            vec![],
        ),
    ];
    for (case, collection, documents, stored, errors) in writes {
        let reply = session
            .command(
                case,
                "shop",
                doc! { "insert": collection, "documents": documents.clone() },
            )
            .await;
        // The command itself was served: what the shard key refused is reported per entry, which
        // is what a driver raises as a bulk-write error rather than as a failed command.
        assert_eq!(
            ok(&reply),
            1.0,
            "case {case}: expected the write command to be served, got {reply:?}"
        );
        assert_eq!(
            reply.get_i32("n").ok(),
            Some(stored),
            "case {case}: expected {stored} of the documents to be stored, got {reply:?}"
        );
        assert_eq!(
            write_errors(&reply),
            errors,
            "case {case}: expected write errors {errors:?}, got {reply:?}"
        );
        // The sentence a client reads, whole: the elastic wording for a document that belongs
        // to no shard, with nothing of the request in it.
        let sentences: Vec<String> = vec![no_shard_key.to_string(); errors.len()];
        assert_eq!(
            write_error_sentences(&reply),
            sentences,
            "case {case}: expected the write errors to say {no_shard_key:?}, got {reply:?}"
        );
    }
    // Only the documents of the admitted entries are there — the ones an ordered write left
    // undone are not — and each of them carries the key the collection is distributed by.
    let found = session
        .command("after the writes", "shop", doc! { "find": "keyed" })
        .await;
    let stored = first_batch("after the writes", &found);
    assert_eq!(
        stored.len(),
        2,
        "after the writes: expected only the documents of the admitted entries to be stored, \
         got {stored:?}"
    );
    assert!(
        stored
            .iter()
            .all(|document| document.get("tenant").is_some()),
        "after the writes: expected every stored document to carry the shard key, got {stored:?}"
    );

    // An update statement: an upsert is targeted by the shard key its FILTER pins, and the key
    // of a document the collection holds moves only on a write the cluster can replay — one
    // carrying a transaction number, which a retryable write and a transaction statement do.
    let statements: Vec<StatementRow> = vec![
        (
            "an upsert whose filter pins the shard key",
            true,
            false,
            doc! { "tenant": "initech" },
            doc! { "$set": { "total": 8_i32 } },
            None,
        ),
        (
            "an upsert whose filter pins it by an equality operator",
            true,
            false,
            doc! { "tenant": { "$eq": "umbrella" } },
            doc! { "$set": { "total": 9_i32 } },
            None,
        ),
        (
            "an upsert whose filter pins no shard key",
            true,
            false,
            doc! { "total": 10_i32 },
            doc! { "$set": { "region": "eu" } },
            Some((61, not_targetable)),
        ),
        (
            "an upsert whose change sets the key its filter does not pin",
            true,
            false,
            doc! { "total": 11_i32 },
            doc! { "$set": { "tenant": "globex" } },
            Some((61, not_targetable)),
        ),
        (
            "an upsert whose filter pins the key by a range",
            true,
            false,
            doc! { "tenant": { "$gt": "acme" } },
            doc! { "$set": { "total": 12_i32 } },
            Some((61, not_targetable)),
        ),
        (
            "a change of the shard key itself",
            false,
            false,
            doc! { "tenant": "acme" },
            doc! { "$set": { "tenant": "globex" } },
            Some((20, not_replayable)),
        ),
        (
            "a change unsetting the shard key",
            false,
            false,
            doc! { "tenant": "acme" },
            doc! { "$unset": { "tenant": "" } },
            Some((20, not_replayable)),
        ),
        (
            "a replacement dropping the shard key",
            false,
            false,
            doc! { "tenant": "acme" },
            doc! { "total": 13_i32 },
            Some((20, not_replayable)),
        ),
        (
            "a change of another field",
            false,
            false,
            doc! { "tenant": "acme" },
            doc! { "$set": { "total": 14_i32 } },
            None,
        ),
        (
            "a change of the shard key on a write the cluster can replay",
            false,
            true,
            doc! { "tenant": "acme" },
            doc! { "$set": { "tenant": "acme-2" } },
            None,
        ),
    ];
    for (case, upsert, retryable, filter, change, refused) in statements {
        let mut command = doc! {
            "update": "keyed",
            "updates": [ doc! { "q": filter, "u": change, "upsert": upsert } ],
        };
        if retryable {
            // What a driver sends a write it may send again under: the session it belongs to and
            // the number that identifies it within that session.
            command.insert("txnNumber", 1_i64);
        }
        let reply = session.command(case, "shop", command).await;
        assert_eq!(
            ok(&reply),
            1.0,
            "case {case}: expected the update command to be served, got {reply:?}"
        );
        match refused {
            None => assert_eq!(
                write_errors(&reply),
                Vec::new(),
                "case {case}: expected the statement to be applied, got {reply:?}"
            ),
            Some((expected, sentence)) => {
                assert_eq!(
                    reply.get_i32("n").ok(),
                    Some(0),
                    "case {case}: expected the refused statement to match nothing, got {reply:?}"
                );
                assert_eq!(
                    write_errors(&reply),
                    vec![(0, expected)],
                    "case {case}: expected one write error carrying {expected}, got {reply:?}"
                );
                assert_eq!(
                    write_error_sentences(&reply),
                    vec![sentence.to_string()],
                    "case {case}: expected the write error to say {sentence:?}, got {reply:?}"
                );
            }
        }
    }
    // The two admitted upserts inserted a document each, the refused statements changed nothing,
    // and every document still carries the key it was distributed by.
    let found = session
        .command("after the statements", "shop", doc! { "find": "keyed" })
        .await;
    let stored = first_batch("after the statements", &found);
    assert_eq!(
        stored.len(),
        4,
        "after the statements: expected only the documents of the admitted writes to be stored, \
         got {stored:?}"
    );
    assert!(
        stored
            .iter()
            .all(|document| document.get("tenant").is_some()),
        "after the statements: expected every stored document to carry the shard key, got \
         {stored:?}"
    );
}

/// The entries a reply reports write errors about, as the index each is about and its code.
fn write_errors(reply: &Document) -> Vec<(i32, i32)> {
    let Ok(errors) = reply.get_array("writeErrors") else {
        return Vec::new();
    };
    errors
        .iter()
        .filter_map(|error| {
            let error = error.as_document()?;
            Some((
                error.get_i32("index").ok()?,
                error.get_i32("code").ok().unwrap_or_default(),
            ))
        })
        .collect()
}

/// The sentences a reply's write errors carry, in the order it reports them.
fn write_error_sentences(reply: &Document) -> Vec<String> {
    let Ok(errors) = reply.get_array("writeErrors") else {
        return Vec::new();
    };
    errors
        .iter()
        .filter_map(|error| Some(error.as_document()?.get_str("errmsg").ok()?.to_string()))
        .collect()
}

/// RULE: a session reaches a cluster's documents only once it has authenticated as that
/// cluster's administrator with the one mechanism the cluster offers — and each way of failing
/// to is answered with the refusal the engine words for it.
///
/// The rows span what the rule generalises over: the command a session sends before it has
/// authenticated (a read, a write), the mechanism it names, the user it claims to be and the
/// password it presents.
#[::tokio::test(flavor = "multi_thread")]
async fn the_endpoint_refuses_an_unauthenticated_command_an_unknown_user_and_a_wrong_password() {
    let emulator = Emulator::new();
    let created = emulator
        .create_cluster("refusals", "auth-elastic", ADMIN_USER, ADMIN_PASSWORD)
        .await;
    let endpoint = member("refusals", &created, "clusterEndpoint");

    // A session that has not authenticated is authorized for nothing, whichever command it
    // sends.
    let unauthenticated: Vec<(&str, Document)> = vec![
        ("a read", doc! { "find": "orders", "filter": {} }),
        (
            "a write",
            doc! { "insert": "orders", "documents": [ doc! { "tenant": "acme" } ] },
        ),
        ("a listing", doc! { "listCollections": 1_i32 }),
    ];
    for (case, command) in unauthenticated {
        let mut session = Session::open(&emulator, case, &endpoint).await;
        let reply = session.command(case, "shop", command).await;
        assert_eq!(
            ok(&reply),
            0.0,
            "case {case}: expected the command to be refused, got {reply:?}"
        );
        assert_eq!(
            code(&reply),
            13,
            "case {case}: expected the authorization refusal's code, got {reply:?}"
        );
        assert_eq!(
            reply.get_str("codeName").ok(),
            Some("AuthorizationFailed"),
            "case {case}: expected the authorization refusal's code name, got {reply:?}"
        );
        assert_eq!(
            errmsg(&reply),
            "Authorization failed.",
            "case {case}: expected the authorization refusal's sentence, got {reply:?}"
        );
    }

    // A mechanism this cluster does not offer is refused as that mechanism, whoever the client
    // claims to be.
    for mechanism in ["SCRAM-SHA-256", "MONGODB-X509"] {
        let case = format!("the mechanism {mechanism}");
        let mut session = Session::open(&emulator, &case, &endpoint).await;
        let reply = session
            .authenticate(&case, mechanism, ADMIN_USER, ADMIN_PASSWORD)
            .await;
        assert_eq!(
            ok(&reply),
            0.0,
            "case {case}: expected the exchange to be refused, got {reply:?}"
        );
        assert_eq!(
            code(&reply),
            300,
            "case {case}: expected the mechanism refusal's code, got {reply:?}"
        );
        assert_eq!(
            errmsg(&reply),
            format!("Invalid auth mechanism {mechanism}"),
            "case {case}: expected the mechanism refusal to name {mechanism}, got {reply:?}"
        );
    }

    // A user that is not this cluster's administrator does not exist; the administrator with
    // the wrong password did not authenticate. Both carry the engine's authentication code and
    // differ in the sentence, which is what tells a client which of the two it is.
    let logins: Vec<(&str, &str, &str, &str)> = vec![
        (
            "an unknown user",
            "nobody",
            ADMIN_PASSWORD,
            "Cascade Authentication failed. User does not exist",
        ),
        (
            "another unknown user",
            "rootuser",
            ADMIN_PASSWORD,
            "Cascade Authentication failed. User does not exist",
        ),
        (
            "the administrator with a wrong password",
            ADMIN_USER,
            "not-the-password",
            "Authentication failed.",
        ),
        (
            "the administrator with an empty password",
            ADMIN_USER,
            "",
            "Authentication failed.",
        ),
    ];
    for (case, user, password, sentence) in logins {
        let mut session = Session::open(&emulator, case, &endpoint).await;
        let reply = session
            .authenticate(case, SCRAM_SHA_1, user, password)
            .await;
        assert_eq!(
            ok(&reply),
            0.0,
            "case {case}: expected the exchange to be refused, got {reply:?}"
        );
        assert_eq!(
            code(&reply),
            18,
            "case {case}: expected the authentication refusal's code, got {reply:?}"
        );
        assert_eq!(
            errmsg(&reply),
            sentence,
            "case {case}: expected the refusal's sentence, got {reply:?}"
        );
        // A refused exchange takes the connection with it: the client does not get to keep
        // presenting credentials on it.
        assert!(
            session.is_closed(case).await,
            "case {case}: expected the endpoint to close the connection a failed exchange was \
             attempted on"
        );
    }

    // And the administrator with the right password reaches the cluster's documents.
    let mut session = Session::open(&emulator, "the administrator", &endpoint).await;
    let admitted = session
        .authenticate("the administrator", SCRAM_SHA_1, ADMIN_USER, ADMIN_PASSWORD)
        .await;
    assert_eq!(
        ok(&admitted),
        1.0,
        "the administrator: expected the exchange to hold, got {admitted:?}"
    );
    let found = session
        .command(
            "the administrator",
            "shop",
            doc! { "find": "orders", "filter": {} },
        )
        .await;
    assert_eq!(
        ok(&found),
        1.0,
        "the administrator: expected the read to be answered, got {found:?}"
    );
}
