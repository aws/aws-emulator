// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the endpoints an Amazon DocumentDB DB CLUSTER and its DB instances
//! publish HERE: an AWS SDK DocumentDB client signs `rds`, so `CreateDBCluster` /
//! `CreateDBInstance` with `Engine: docdb` are served by this front door, and the resource they
//! create is an Amazon DocumentDB cluster.
//!
//! The rule under test: an endpoint a service advertises is an address the service ANSWERS on,
//! so `DBCluster$Endpoint`, `DBCluster$ReaderEndpoint` and `Endpoint{Address,Port}` of such a
//! cluster and its members are addresses of THIS emulator's — its own address on the interface
//! the core bound, with the port published beside it — and a client dialling one is served the
//! document wire out of that cluster's own documents. A name under Amazon DocumentDB's own DNS
//! zone resolves to nothing on the machine this emulator runs on and would be no connection
//! endpoint at all.
//!
//! The tables span what the rule generalises over: whether the cluster chose its engine port,
//! which endpoint of the cluster is dialled (the cluster's own two, each member's), how many
//! members it has (each reached at an endpoint of its own), whether the session has
//! authenticated, and which commands it sends. Every assertion goes over a real connection to
//! the address the control plane published, opened the way a client inside the cluster's
//! virtual private cloud opens one — the only vantage point such an endpoint is reachable from
//! — and nothing reaches into the service's state to shortcut it.

use std::sync::Arc;

use ::bson::{doc, Bson, Document};
use ::service_common::dialable_data_planes::DialablePlanes;
use ::tokio::io::{AsyncReadExt, AsyncWriteExt};
use ::tokio::net::TcpStream;

use crate::db_instances::{call, emulator_with_engine, expect_ok, subtrees, text};

/// The master user every cluster below is created with.
const MASTER_USER: &str = "docdbadmin";

/// Its password. A test credential, and the only place in this suite it appears.
const MASTER_PASSWORD: &str = "docdb-Secret-99";

/// The engine whose data plane is the document wire.
const DOCDB_ENGINE: &str = "docdb";

/// The DB instance class every member below runs on.
const INSTANCE_CLASS: &str = "db.r5.large";

/// `[(name, value.to_string()), ..]` — the query parameters of one call.
macro_rules! params {
    ($($name:expr => $value:expr),* $(,)?) => {
        [$(($name, $value.to_string())),*]
    };
}

/// A cluster and its members, settled, with the endpoint each member publishes.
async fn cluster_with_members(
    router: &smithy_server_runtime::MultiServiceRouter,
    cluster: &str,
    members: &[&str],
    port: Option<&str>,
) -> Vec<(String, u16)> {
    let mut request: Vec<(&str, String)> = params![
        "DBClusterIdentifier" => cluster,
        "Engine" => DOCDB_ENGINE,
        "MasterUsername" => MASTER_USER,
        "MasterUserPassword" => MASTER_PASSWORD,
    ]
    .to_vec();
    if let Some(port) = port {
        request.push(("Port", port.to_string()));
    }
    expect_ok(cluster, call(router, "CreateDBCluster", &request).await);
    let mut endpoints = Vec::new();
    for member in members {
        let created = expect_ok(
            member,
            call(
                router,
                "CreateDBInstance",
                &params![
                    "DBInstanceIdentifier" => member,
                    "Engine" => DOCDB_ENGINE,
                    "DBInstanceClass" => INSTANCE_CLASS,
                    "DBClusterIdentifier" => cluster,
                ],
            )
            .await,
        );
        let published = endpoint_of(member, &created);
        // The instance comes up on its own, so what this waits for is the CONDITION a client
        // has to see before it connects: read again until it holds or a deadline far longer
        // than the span this service owes that transition in has passed.
        let deadline = std::time::SystemTime::now() + std::time::Duration::from_secs(120);
        loop {
            let described = expect_ok(
                member,
                call(
                    router,
                    "DescribeDBInstances",
                    &params!["DBInstanceIdentifier" => member],
                )
                .await,
            );
            if text(&described, "DBInstanceStatus") == "available" {
                // ROUND TRIP: the describe reports the endpoint the create published.
                assert_eq!(
                    endpoint_of(member, &described),
                    published,
                    "case {member:?}: the endpoint the create published must be the one the \
                     describe reports"
                );
                break;
            }
            assert!(
                std::time::SystemTime::now() < deadline,
                "case {member:?}: the instance must settle to available before a client \
                 connects, last read {described}"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        endpoints.push(published);
    }
    endpoints
}

/// The `Endpoint` an envelope publishes, as a client reads it — asserted to be an address of
/// this emulator's rather than a name in Amazon DocumentDB's own DNS zone.
fn endpoint_of(case: &str, envelope: &str) -> (String, u16) {
    let endpoint = subtrees(envelope, "Endpoint")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: the envelope must carry an Endpoint: {envelope}"));
    let address = text(&endpoint, "Address");
    let port = text(&endpoint, "Port");
    assert!(
        !address.ends_with("docdb.amazonaws.com"),
        "case {case:?}: the published address must be one this emulator answers on, not a name \
         in Amazon DocumentDB's own DNS zone, got {address:?}"
    );
    assert!(
        address.parse::<std::net::IpAddr>().is_ok(),
        "case {case:?}: the published address must name the interface the core bound, got \
         {address:?}"
    );
    (
        address,
        port.parse()
            .unwrap_or_else(|_| panic!("case {case:?}: the endpoint must publish a port, got {port:?}")),
    )
}

/// One client session on an instance's published endpoint.
struct Session {
    stream: Box<dyn Transport>,
    request_id: i32,
}

/// What a session needs of its connection.
trait Transport: ::tokio::io::AsyncRead + ::tokio::io::AsyncWrite + Unpin + Send {}
impl<T: ::tokio::io::AsyncRead + ::tokio::io::AsyncWrite + Unpin + Send> Transport for T {}

impl Session {
    /// Open a session on the address the control plane published for a CLUSTER, the way a
    /// client inside the cluster's virtual private cloud opens one: through the register of the
    /// planes this emulator serves in process, at exactly the `host:port` it was handed.
    ///
    /// A cluster's two endpoint members carry no port of their own — the cluster has one `Port`
    /// member for both — so the pair a client composes is the published host with that port,
    /// and that is the address asked for here.
    async fn on_cluster_endpoint(
        engine: &Arc<engine::InMemoryEngine>,
        case: &str,
        address: &str,
        port: u16,
    ) -> Self {
        let planes = DialablePlanes::of(&**engine)
            .expect("the engine vends the register of served data planes");
        let dialed = planes.dialable(address, port).unwrap_or_else(|| {
            panic!("case {case:?}: nothing is served at the published pair {address}:{port}")
        });
        assert_eq!(
            dialed.served_by, "rds",
            "case {case:?}: the published pair must be served by this front door, got {dialed:?}"
        );
        let conn = dialed.plane.dial(None).await.unwrap_or_else(|error| {
            panic!("case {case:?}: {address}:{port} must accept a connection, got {error}")
        });
        Self::over_tls(case, Box::new(conn)).await
    }

    /// Run the TLS handshake a client of an encrypted endpoint opens with — the cluster's `tls`
    /// parameter is at its default (`enabled`), and the core terminates that handshake at both
    /// ends of the address — and answer the session over the stream it yields.
    async fn over_tls(case: &str, conn: Box<dyn Transport>) -> Self {
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let config = rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
            .with_safe_default_protocol_versions()
            .expect("the provider supports the default protocol versions")
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptTheEmulatorsCertificate(provider)))
            .with_no_client_auth();
        let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
        let server_name = rustls::pki_types::ServerName::try_from("localhost")
            .expect("localhost is a server name");
        let stream = connector
            .connect(server_name, conn)
            .await
            .unwrap_or_else(|error| {
                panic!("case {case:?}: the endpoint must complete a TLS handshake, got {error}")
            });
        Self {
            stream: Box::new(stream),
            request_id: 1,
        }
    }

    /// Open a session on the published endpoint over a REAL socket, as any client of this
    /// machine dials it: the pair the control plane published is the one the core bound and
    /// assigned. The register of the planes this emulator serves in process holds that same
    /// address, asserted here too, because a client that cannot open a host socket — the code
    /// of a function attached to the cluster's virtual private cloud — reaches the same
    /// server at the same address.
    ///
    /// The cluster's `tls` parameter is at its default (`enabled`), so the endpoint is
    /// encrypted and the client runs the handshake the core's upgrader answers.
    async fn open(
        engine: &Arc<engine::InMemoryEngine>,
        case: &str,
        address: &str,
        port: u16,
    ) -> Self {
        let planes = DialablePlanes::of(&**engine)
            .expect("the engine vends the register of served data planes");
        let authority = ::service_common::data_plane_addressing::host_port_authority(address, port);
        let served = planes.served_by(&authority);
        assert_eq!(
            served.as_deref(),
            Some("rds"),
            "case {case:?}: the published endpoint {authority} must be open as an address this \
             front door serves an instance at, got {served:?}"
        );
        // The published host is this machine's own loopback address — the core binds its
        // listeners there — so the dial names that loopback literal, and the assertion here is
        // what pins the published address to it.
        let published: std::net::IpAddr = address
            .trim_matches(['[', ']'])
            .parse()
            .unwrap_or_else(|_| panic!("case {case:?}: the published address must be an address, got {address:?}"));
        assert!(
            published.is_loopback(),
            "case {case:?}: the published address must be this machine's loopback address, got \
             {address:?}"
        );
        let conn = match published {
            std::net::IpAddr::V4(_) => TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).await,
            std::net::IpAddr::V6(_) => TcpStream::connect((std::net::Ipv6Addr::LOCALHOST, port)).await,
        }
        .unwrap_or_else(|error| {
            panic!("case {case:?}: {authority} must accept a connection, got {error}")
        });
        Self::over_tls(case, Box::new(conn)).await
    }

    /// Send one command in the message form a driver sends every command in, and read the
    /// reply document.
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
        // Opcode 2013: the message every command of a driver travels in.
        message.extend_from_slice(&2013_i32.to_le_bytes());
        message.extend_from_slice(&body);
        self.request_id += 1;
        self.stream
            .write_all(&message)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: the command must be sent, got {error}"));
        self.stream.flush().await.unwrap_or_else(|error| {
            panic!("case {case:?}: the command must be flushed, got {error}")
        });
        let mut header = [0u8; 16];
        self.stream
            .read_exact(&mut header)
            .await
            .unwrap_or_else(|error| {
                panic!("case {case:?}: the endpoint must answer with a message, got {error}")
            });
        let reply_length = i32::from_le_bytes([header[0], header[1], header[2], header[3]]);
        let reply_length = usize::try_from(reply_length).unwrap_or_else(|_| {
            panic!("case {case:?}: the reply must name a length, got {reply_length}")
        });
        assert!(
            (16..=48_000_000).contains(&reply_length),
            "case {case:?}: the reply length must be a message length, got {reply_length}"
        );
        let mut reply = vec![0u8; reply_length - 16];
        self.stream
            .read_exact(&mut reply)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: the whole reply must arrive, got {error}"));
        // The reply's own flags and section kind come before its document.
        let document = reply
            .get(5..)
            .unwrap_or_else(|| panic!("case {case:?}: the reply must carry a document"));
        Document::from_reader(&mut std::io::Cursor::new(document)).unwrap_or_else(|error| {
            panic!("case {case:?}: the reply must be a BSON document, got {error}")
        })
    }

    /// Authenticate as the cluster's master user over SCRAM-SHA-256 (RFC 5802), the exchange a
    /// driver runs, and answer the reply of its last step.
    async fn authenticate(&mut self, case: &str, username: &str, password: &str) -> Document {
        let client_nonce = "Y2xpZW50bm9uY2VyZHM=";
        let first_bare = format!("n={username},r={client_nonce}");
        let started = self
            .command(
                case,
                "admin",
                doc! {
                    "saslStart": 1_i32,
                    "mechanism": "SCRAM-SHA-256",
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
            .unwrap_or_else(|_| panic!("case {case:?}: saslStart answers a conversation id"));
        let server_first = String::from_utf8(payload_of(&started))
            .unwrap_or_else(|_| panic!("case {case:?}: the server-first message is text"));
        let salt = ::aws_smithy_types::base64::decode(attribute(&server_first, 's'))
            .unwrap_or_else(|_| panic!("case {case:?}: the salt is base64"));
        let iterations: u32 = attribute(&server_first, 'i')
            .parse()
            .unwrap_or_else(|_| panic!("case {case:?}: the iteration count is a number"));
        let combined_nonce = attribute(&server_first, 'r').to_string();
        let mut salted = [0u8; 32];
        ::pbkdf2::pbkdf2_hmac::<::sha2::Sha256>(
            password.as_bytes(),
            &salt,
            iterations,
            &mut salted,
        );
        let client_key = hmac_sha_256(&salted, b"Client Key");
        let stored_key = {
            use ::sha2::Digest;
            ::sha2::Sha256::digest(&client_key).to_vec()
        };
        let without_proof = format!("c=biws,r={combined_nonce}");
        let auth_message = format!("{first_bare},{server_first},{without_proof}");
        let signature = hmac_sha_256(&stored_key, auth_message.as_bytes());
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

fn hmac_sha_256(key: &[u8], data: &[u8]) -> Vec<u8> {
    use ::hmac::digest::KeyInit;
    use ::hmac::Mac;
    let mut mac = <::hmac::Hmac<::sha2::Sha256> as KeyInit>::new_from_slice(key)
        .expect("HMAC-SHA-256 takes a key of any length");
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

/// The documents a `find` answered with.
fn batch(case: &str, reply: &Document, member: &str) -> Vec<Bson> {
    reply
        .get_document("cursor")
        .and_then(|cursor| cursor.get_array(member).cloned())
        .unwrap_or_else(|_| panic!("case {case:?}: expected a cursor carrying {member}, got {reply:?}"))
}

/// The verifier a client of the emulator's OWN plane uses: the certificate is the one the core
/// generated for its listeners, self-signed and naming loopback, and this client is the
/// emulator's own test connecting to a plane the same process serves — so there is no authority
/// to check it against.
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

#[tokio::test]
async fn a_documentdb_instance_publishes_an_endpoint_that_serves_the_document_wire() {
    let (router, engine) = emulator_with_engine();
    let case = "a cluster with one member";
    let endpoints = cluster_with_members(
        &router,
        "docdb-wire-cluster",
        &["docdb-wire-instance"],
        None,
    )
    .await;
    let (address, port) = endpoints
        .first()
        .cloned()
        .expect("the member publishes an endpoint");
    // The core owns the port range and takes no port from this service, so what the member
    // publishes is the port the core assigned its socket — a port a client can reach it at,
    // which is what an endpoint is. The cluster goes on reporting the engine port it was
    // created with; the open issue
    // `20261105T090000Z-rds-a-db-instance-publishes-the-listeners-port-not-the-requested-port`
    // owns that difference in the VALUE for every engine this door serves.
    assert!(
        port > 0,
        "case {case:?}: expected the port the core assigned the socket, got {port}"
    );

    let mut session = Session::open(&engine, case, &address, port).await;
    let hello = session.command(case, "admin", doc! { "hello": 1_i32 }).await;
    assert_eq!(
        ok(&hello),
        1.0,
        "case {case:?}: the endpoint must answer the handshake, got {hello:?}"
    );
    assert_eq!(
        hello.get_bool("isWritablePrimary").ok(),
        Some(true),
        "case {case:?}: the cluster's only member is the one it writes through, got {hello:?}"
    );

    // Before the SASL exchange the session is nobody, so a command that reads or writes
    // documents is refused rather than served.
    let unauthenticated = session
        .command(case, "shop", doc! { "insert": "widgets", "documents": [{ "_id": 1_i32 }] })
        .await;
    assert_eq!(
        (ok(&unauthenticated), code(&unauthenticated)),
        (0.0, 13),
        "case {case:?}: an unauthenticated write must be refused, got {unauthenticated:?}"
    );
    let refused = session
        .authenticate(case, MASTER_USER, "another-Secret-99")
        .await;
    assert_eq!(
        (ok(&refused), code(&refused)),
        (0.0, 18),
        "case {case:?}: a proof computed from another password must not authenticate, got \
         {refused:?}"
    );

    let mut session = Session::open(&engine, case, &address, port).await;
    let authenticated = session
        .authenticate(case, MASTER_USER, MASTER_PASSWORD)
        .await;
    assert_eq!(
        ok(&authenticated),
        1.0,
        "case {case:?}: the master user's password must authenticate the session, got \
         {authenticated:?}"
    );

    // The commands, over a table of document shapes: the rule generalises over what a document
    // carries, so the rows differ in their fields, their types and their count.
    let rows: Vec<(&str, Document)> = vec![
        ("a document with an _id of its own", doc! { "_id": 1_i32, "sku": "left", "stock": 3_i32 }),
        ("a document the engine mints an _id for", doc! { "sku": "right", "stock": 9_i32 }),
        (
            "a document carrying an array and a nested document",
            doc! { "_id": "third", "tags": ["a", "b"], "spec": { "size": 2_i32 } },
        ),
    ];
    for (row, document) in &rows {
        let inserted = session
            .command(
                case,
                "shop",
                doc! { "insert": "widgets", "documents": [document.clone()] },
            )
            .await;
        assert_eq!(
            (ok(&inserted), inserted.get_i32("n").ok()),
            (1.0, Some(1)),
            "case {case:?} / {row}: the insert must store one document, got {inserted:?}"
        );
    }
    let found = session
        .command(case, "shop", doc! { "find": "widgets", "filter": {} })
        .await;
    assert_eq!(
        batch(case, &found, "firstBatch").len(),
        rows.len(),
        "case {case:?}: the find must read back every document written, got {found:?}"
    );
    let filtered = session
        .command(
            case,
            "shop",
            doc! { "find": "widgets", "filter": { "stock": { "$gt": 5_i32 } } },
        )
        .await;
    assert_eq!(
        batch(case, &filtered, "firstBatch").len(),
        1,
        "case {case:?}: the filter must select only the matching document, got {filtered:?}"
    );
    let updated = session
        .command(
            case,
            "shop",
            doc! {
                "update": "widgets",
                "updates": [{ "q": { "_id": 1_i32 }, "u": { "$set": { "stock": 11_i32 } } }],
            },
        )
        .await;
    assert_eq!(
        (updated.get_i32("n").ok(), updated.get_i32("nModified").ok()),
        (Some(1), Some(1)),
        "case {case:?}: the update must change the document it selected, got {updated:?}"
    );
    let counted = session
        .command(
            case,
            "shop",
            doc! { "count": "widgets", "query": { "stock": { "$gte": 11_i32 } } },
        )
        .await;
    assert_eq!(
        counted.get_i32("n").ok(),
        Some(1),
        "case {case:?}: the count must see the update, got {counted:?}"
    );
    let deleted = session
        .command(
            case,
            "shop",
            doc! { "delete": "widgets", "deletes": [{ "q": { "_id": 1_i32 }, "limit": 1_i32 }] },
        )
        .await;
    assert_eq!(
        deleted.get_i32("n").ok(),
        Some(1),
        "case {case:?}: the delete must remove the document it selected, got {deleted:?}"
    );
    // A command this endpoint does not serve is refused by name, never answered as an empty
    // success.
    let unknown = session
        .command(case, "shop", doc! { "mapReduce": "widgets" })
        .await;
    assert_eq!(
        (ok(&unknown), code(&unknown)),
        (0.0, 59),
        "case {case:?}: a command this endpoint does not serve must be refused, got {unknown:?}"
    );
}

#[tokio::test]
async fn every_member_publishes_an_address_of_its_own_onto_one_set_of_documents() {
    // "Each Amazon DocumentDB cluster hosts copies of its data": a member is a second address
    // onto its cluster's storage, so what the writer wrote is what a replica reads — and each
    // member is reached at an endpoint of its OWN, on the engine port the cluster publishes,
    // which the row below varies.
    let (router, engine) = emulator_with_engine();
    let case = "two members on a chosen engine port";
    let endpoints = cluster_with_members(
        &router,
        "docdb-members-cluster",
        &["docdb-members-writer", "docdb-members-replica"],
        Some("27018"),
    )
    .await;
    let [(writer_address, writer_port), (replica_address, replica_port)] = endpoints
        .try_into()
        .unwrap_or_else(|endpoints| panic!("case {case:?}: both members publish one endpoint each, got {endpoints:?}"));
    assert_ne!(
        (writer_address.as_str(), writer_port),
        (replica_address.as_str(), replica_port),
        "case {case:?}: each member is reached at an endpoint of its own"
    );

    let mut writer = Session::open(&engine, case, &writer_address, writer_port).await;
    assert_eq!(
        ok(&writer.authenticate(case, MASTER_USER, MASTER_PASSWORD).await),
        1.0,
        "case {case:?}: the writer must authenticate the cluster's master user"
    );
    let inserted = writer
        .command(
            case,
            "shop",
            doc! { "insert": "widgets", "documents": [{ "_id": 1_i32, "sku": "shared" }] },
        )
        .await;
    assert_eq!(
        ok(&inserted),
        1.0,
        "case {case:?}: the writer must take the write, got {inserted:?}"
    );

    let mut replica = Session::open(&engine, case, &replica_address, replica_port).await;
    assert_eq!(
        ok(&replica.authenticate(case, MASTER_USER, MASTER_PASSWORD).await),
        1.0,
        "case {case:?}: the replica must authenticate the same master user"
    );
    let hello = replica.command(case, "admin", doc! { "hello": 1_i32 }).await;
    assert_eq!(
        hello.get_bool("isWritablePrimary").ok(),
        Some(false),
        "case {case:?}: a replica must report itself as no writer, got {hello:?}"
    );
    let read = replica
        .command(case, "shop", doc! { "find": "widgets", "filter": { "sku": "shared" } })
        .await;
    assert_eq!(
        batch(case, &read, "firstBatch").len(),
        1,
        "case {case:?}: the replica must read what the writer wrote, got {read:?}"
    );
    let refused = replica
        .command(case, "shop", doc! { "insert": "widgets", "documents": [{ "_id": 2_i32 }] })
        .await;
    assert_eq!(
        (ok(&refused), code(&refused)),
        (0.0, 10107),
        "case {case:?}: a replica serves reads only, got {refused:?}"
    );
}

#[tokio::test]
async fn the_endpoint_answers_nothing_once_the_instance_is_deleted() {
    // The endpoint is the resource: deleting the instance closes the socket it published, so
    // the address it published answers nothing any more.
    let (router, engine) = emulator_with_engine();
    let case = "a deleted member";
    let endpoints =
        cluster_with_members(&router, "docdb-closing-cluster", &["docdb-closing-instance"], None)
            .await;
    let (address, port) = endpoints
        .first()
        .cloned()
        .expect("the member publishes an endpoint");
    let mut session = Session::open(&engine, case, &address, port).await;
    assert_eq!(
        ok(&session.command(case, "admin", doc! { "ping": 1_i32 }).await),
        1.0,
        "case {case:?}: the endpoint must answer while the instance is there"
    );
    expect_ok(
        case,
        call(
            &router,
            "DeleteDBInstance",
            &params![
                "DBInstanceIdentifier" => "docdb-closing-instance",
                "SkipFinalSnapshot" => "true",
            ],
        )
        .await,
    );
    let planes = DialablePlanes::of(&*engine)
        .expect("the engine vends the register of served data planes");
    let authority = ::service_common::data_plane_addressing::host_port_authority(&address, port);
    // The delete marks the instance for removal and the clock takes it out; the socket goes
    // with the handle the store held, so the test looks again until the address answers
    // nothing or a deadline far longer than that span has passed.
    let deadline = std::time::SystemTime::now() + std::time::Duration::from_secs(120);
    loop {
        // A read of the instances is what settles the removal the clock owes.
        let _ = call(
            &router,
            "DescribeDBInstances",
            &params!["MaxRecords" => "20"],
        )
        .await;
        // `127.0.0.1`: this machine's own loopback address, which is the host the endpoint
        // published (asserted when the session above was opened on it).
        if planes.served_by(&authority).is_none()
            && TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).await.is_err()
        {
            break;
        }
        assert!(
            std::time::SystemTime::now() < deadline,
            "case {case:?}: {authority} must answer nothing once the instance is deleted"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[tokio::test]
async fn a_documentdb_cluster_publishes_endpoints_that_serve_its_documents() {
    // RULE: a cluster's own endpoint members — `DBCluster$Endpoint`, "the connection endpoint
    // for the primary instance of the DB cluster", and `DBCluster$ReaderEndpoint` — are the
    // HOST this emulator answers that cluster's document wire on, and the one `Port` the
    // cluster publishes beside them completes the address a client composes, which is an
    // address that plane answers at. The rows span the port the create asked for: none, so the
    // engine's documented default stands, and one of its own.
    let (router, engine) = emulator_with_engine();
    for (case, cluster, member, asked_port, expected_port) in [
        (
            "a cluster that took the engine's default port",
            "docdb-cluster-endpoint-default",
            "docdb-cluster-endpoint-default-1",
            None,
            // `CreateDBClusterMessage$Port` of the DocumentDB model: "Default: 27017".
            "27017",
        ),
        (
            "a cluster that asked for a port of its own",
            "docdb-cluster-endpoint-named",
            "docdb-cluster-endpoint-named-1",
            Some("27019"),
            "27019",
        ),
    ] {
        cluster_with_members(&router, cluster, &[member], asked_port).await;
        let described = expect_ok(
            case,
            call(
                &router,
                "DescribeDBClusters",
                &params!["DBClusterIdentifier" => cluster],
            )
            .await,
        );
        let writer = text(&described, "Endpoint");
        let reader = text(&described, "ReaderEndpoint");
        let port = text(&described, "Port");
        assert_eq!(
            port, expected_port,
            "case {case:?}: expected the port the cluster's engine is configured to listen on, \
             got {port:?} in {described}"
        );
        for (member_name, host) in [("Endpoint", &writer), ("ReaderEndpoint", &reader)] {
            assert!(
                !host.ends_with("docdb.amazonaws.com"),
                "case {case:?}: {member_name} must be an address this emulator answers on, not \
                 a name in Amazon DocumentDB's own DNS zone, got {host:?}"
            );
            assert!(
                host.trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|address| address.is_loopback()),
                "case {case:?}: {member_name} must name the interface the core bound this \
                 cluster's listener on, got {host:?}"
            );
        }
        let dialed_port: u16 = port
            .parse()
            .unwrap_or_else(|_| panic!("case {case:?}: the published port must be a port"));

        // The pair the control plane published is dialled exactly as handed over, and what
        // answers is the cluster's primary over the CLUSTER's documents.
        let mut session =
            Session::on_cluster_endpoint(&engine, case, &writer, dialed_port).await;
        let hello = session.command(case, "admin", doc! { "hello": 1_i32 }).await;
        assert_eq!(
            ok(&hello),
            1.0,
            "case {case:?}: the published pair must answer the handshake, got {hello:?}"
        );
        assert_eq!(
            hello.get_bool("isWritablePrimary").ok(),
            Some(true),
            "case {case:?}: the cluster endpoint leads to the primary, so it serves writes, \
             got {hello:?}"
        );
        assert_eq!(
            ok(&session.authenticate(case, MASTER_USER, MASTER_PASSWORD).await),
            1.0,
            "case {case:?}: the cluster's master user must authenticate a session on the \
             cluster's own endpoint"
        );
        let inserted = session
            .command(
                case,
                "shop",
                doc! {
                    "insert": "orders",
                    "documents": [{ "_id": 11_i32, "sku": "through-the-cluster" }],
                },
            )
            .await;
        assert_eq!(
            (ok(&inserted), inserted.get_i32("n").ok()),
            (1.0, Some(1)),
            "case {case:?}: the write sent to the cluster endpoint must be served, got \
             {inserted:?}"
        );

        // The READER endpoint fronts the same plane, so a session on it finds that write.
        let mut through_reader =
            Session::on_cluster_endpoint(&engine, case, &reader, dialed_port).await;
        through_reader
            .authenticate(case, MASTER_USER, MASTER_PASSWORD)
            .await;
        let read = through_reader
            .command(case, "shop", doc! { "find": "orders", "filter": { "_id": 11_i32 } })
            .await;
        assert_eq!(
            batch(case, &read, "firstBatch").len(),
            1,
            "case {case:?}: the reader endpoint must serve the read, got {read:?}"
        );

        // And the documents are the CLUSTER's: the MEMBER's own endpoint, on a socket and a
        // port of its own, answers out of the same volume.
        let member_envelope = expect_ok(
            case,
            call(
                &router,
                "DescribeDBInstances",
                &params!["DBInstanceIdentifier" => member],
            )
            .await,
        );
        let (member_address, member_port) = endpoint_of(case, &member_envelope);
        let mut through_member =
            Session::open(&engine, case, &member_address, member_port).await;
        through_member
            .authenticate(case, MASTER_USER, MASTER_PASSWORD)
            .await;
        let from_member = through_member
            .command(case, "shop", doc! { "find": "orders", "filter": { "_id": 11_i32 } })
            .await;
        assert_eq!(
            batch(case, &from_member, "firstBatch").len(),
            1,
            "case {case:?}: one cluster is one volume, so the member's endpoint must find what \
             the cluster endpoint wrote, got {from_member:?}"
        );
    }
}
