// SPDX-License-Identifier: CC0-1.0

//! The DATA PLANE of an Amazon DocumentDB instance: that the endpoint the control plane
//! publishes is an address a client of THIS emulator can open a session on, and that the
//! session reads the documents it wrote.
//!
//! The rule under test: an endpoint a service advertises is an address the service ANSWERS
//! on, so the `Endpoint{Address,Port}` every `DBInstance`-returning operation reports is one
//! of the emulator's OWN addresses and a client dialling it is served out of that instance's
//! cluster. The tables below span what the rule generalises over — whether the wire is
//! encrypted (the cluster's `tls` parameter), whether the cluster authenticates a master user,
//! which ROLE of its cluster the member holds (the writer, a replica), which COMMANDS the
//! session sends, and which message form carries them (`OP_MSG`, the legacy `OP_QUERY`
//! handshake) — and a single insert-and-find round trip is one row of it.
//!
//! Every assertion goes over a real socket the core opened for the instance, dialled at the
//! address the control plane published; nothing reaches into the service's state.
//!
//! The session harness below ([`Session`] and the readers around it) is `pub(crate)`, so the
//! sibling suite that drives a CLUSTER's own endpoint the same way
//! (`tests/all/cluster_endpoint_own_address.rs`) speaks the wire through one harness rather
//! than a copy of it.

use std::sync::Arc;

use ::bson::{doc, Bson, Document};
use ::service_common::dialable_data_planes::DialablePlanes;
use ::tokio::io::{AsyncReadExt, AsyncWriteExt};
use ::tokio::net::TcpStream;

use crate::cluster_lifecycle::{let_the_clock_catch_up, text, Docdb};

/// The master user a cluster under test is created with.
const MASTER_USERNAME: &str = "docdbadmin";

/// Its password, which only this test and the credential derived from it ever see.
const MASTER_PASSWORD: &str = "a-Secret-99";

/// A byte stream a session runs on: a plaintext socket, the TLS stream over one, or a
/// connection an in-process client opened to a published address.
pub(crate) type Stream = Box<dyn Transport>;

/// What a session needs of its connection.
pub(crate) trait Transport: ::tokio::io::AsyncRead + ::tokio::io::AsyncWrite + Unpin + Send {}
impl<T: ::tokio::io::AsyncRead + ::tokio::io::AsyncWrite + Unpin + Send> Transport for T {}

/// `OP_MSG`, the message a driver sends every command in.
const OP_MSG: i32 = 2013;

/// `OP_QUERY`, the message a driver's first handshake travels in.
const OP_QUERY: i32 = 2004;

/// One client session on an endpoint of a cluster or of one of its members.
pub(crate) struct Session {
    stream: Stream,
    request_id: i32,
}

impl Session {
    /// A session on a connection a client has already opened — the one an IN-PROCESS client of
    /// this emulator opens to the address the control plane published
    /// (`service_common::dialable_data_planes`), which is the vantage point a cluster endpoint
    /// is reachable from.
    pub(crate) fn over(stream: Stream) -> Self {
        Self {
            stream,
            request_id: 1,
        }
    }

    /// Open a plaintext session on the address the control plane published — over a REAL
    /// socket, as any client of this machine dials it: the published pair is the one the core
    /// bound and assigned, so the client that reads it out of the control plane and connects
    /// to it reaches this instance's own server.
    ///
    /// The register of the planes this emulator serves in process holds that same address
    /// (`service_common::dialable_data_planes`), asserted here too: a client that cannot open
    /// a host socket — the code of a function attached to the cluster's virtual private cloud
    /// — reaches the same server at the same address.
    async fn open(emulator: &Docdb, case: &str, address: &str, port: &str) -> Self {
        let port: u16 = port
            .parse()
            .unwrap_or_else(|_| panic!("case {case}: the published port must be a port, got {port:?}"));
        let planes = DialablePlanes::of(&**emulator.engine())
            .expect("the engine vends the register of served data planes");
        let authority = ::service_common::data_plane_addressing::host_port_authority(address, port);
        let served = planes.served_by(&authority);
        assert_eq!(
            served.as_deref(),
            Some("rds"),
            "case {case}: the published endpoint {authority} must be open as an address this \
             front door serves an instance at, got {served:?}"
        );
        // The published host is this machine's own loopback address — the core binds its
        // listeners there — so the dial names that loopback literal, and the assertion above
        // the call is what pins the published address to it.
        let published: std::net::IpAddr = address
            .trim_matches(['[', ']'])
            .parse()
            .unwrap_or_else(|_| panic!("case {case}: the published address must be an address, got {address:?}"));
        assert!(
            published.is_loopback(),
            "case {case}: the published address must be this machine's loopback address, got \
             {address:?}"
        );
        let stream = match published {
            std::net::IpAddr::V4(_) => TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).await,
            std::net::IpAddr::V6(_) => TcpStream::connect((std::net::Ipv6Addr::LOCALHOST, port)).await,
        }
        .unwrap_or_else(|error| {
            panic!("case {case}: {authority} must accept a connection, got {error}")
        });
        Self {
            stream: Box::new(stream),
            request_id: 1,
        }
    }

    /// Open a session over TLS, which is how a client reaches a cluster whose `tls` parameter
    /// is on — the wire the core terminates for such an endpoint.
    async fn open_tls(emulator: &Docdb, case: &str, address: &str, port: &str) -> Self {
        let plain = Self::open(emulator, case, address, port).await;
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
            .connect(server_name, plain.stream)
            .await
            .unwrap_or_else(|error| {
                panic!("case {case}: the instance's socket must complete a TLS handshake, got {error}")
            });
        Self {
            stream: Box::new(stream),
            request_id: 1,
        }
    }

    /// Send one command as an `OP_MSG`, the form every command of a driver travels in, and
    /// read the reply document.
    pub(crate) async fn command(&mut self, case: &str, database: &str, command: Document) -> Document {
        let mut command = command;
        command.insert("$db".to_string(), database);
        let mut body: Vec<u8> = Vec::new();
        body.extend_from_slice(&0_u32.to_le_bytes());
        body.push(0);
        command
            .to_writer(&mut body)
            .expect("the command document encodes");
        self.exchange(case, OP_MSG, &body, 5).await
    }

    /// Send one command as an `OP_QUERY` against `<db>.$cmd`, which is how a driver that does
    /// not yet know what it is talking to sends its first handshake.
    pub(crate) async fn legacy_command(
        &mut self,
        case: &str,
        database: &str,
        command: Document,
    ) -> Document {
        let mut body: Vec<u8> = Vec::new();
        body.extend_from_slice(&0_i32.to_le_bytes());
        body.extend_from_slice(format!("{database}.$cmd\0").as_bytes());
        body.extend_from_slice(&0_i32.to_le_bytes());
        body.extend_from_slice(&(-1_i32).to_le_bytes());
        command
            .to_writer(&mut body)
            .expect("the command document encodes");
        // An `OP_REPLY` body is four header fields (20 bytes) before its first document.
        self.exchange(case, OP_QUERY, &body, 20).await
    }

    /// Write one message and read the reply, taking the reply document from `document_at`
    /// bytes into the answer's body.
    async fn exchange(
        &mut self,
        case: &str,
        opcode: i32,
        body: &[u8],
        document_at: usize,
    ) -> Document {
        let length = i32::try_from(16 + body.len()).expect("the message is small");
        let mut message: Vec<u8> = Vec::new();
        message.extend_from_slice(&length.to_le_bytes());
        message.extend_from_slice(&self.request_id.to_le_bytes());
        message.extend_from_slice(&0_i32.to_le_bytes());
        message.extend_from_slice(&opcode.to_le_bytes());
        message.extend_from_slice(body);
        self.request_id += 1;
        self.stream
            .write_all(&message)
            .await
            .unwrap_or_else(|error| panic!("case {case}: the command must be sent, got {error}"));
        self.stream
            .flush()
            .await
            .unwrap_or_else(|error| panic!("case {case}: the command must be flushed, got {error}"));
        let mut header = [0u8; 16];
        self.stream
            .read_exact(&mut header)
            .await
            .unwrap_or_else(|error| {
                panic!("case {case}: the endpoint must answer with a message, got {error}")
            });
        let reply_length = i32::from_le_bytes([header[0], header[1], header[2], header[3]]);
        let reply_length = usize::try_from(reply_length)
            .unwrap_or_else(|_| panic!("case {case}: the reply names a length, got {reply_length}"));
        assert!(
            (16..=48_000_000).contains(&reply_length),
            "case {case}: the reply length must be a message length, got {reply_length}"
        );
        let mut reply = vec![0u8; reply_length - 16];
        self.stream
            .read_exact(&mut reply)
            .await
            .unwrap_or_else(|error| {
                panic!("case {case}: the whole reply must arrive, got {error}")
            });
        let document = reply
            .get(document_at..)
            .unwrap_or_else(|| panic!("case {case}: the reply carries a document"));
        Document::from_reader(&mut std::io::Cursor::new(document)).unwrap_or_else(|error| {
            panic!("case {case}: the reply must be a BSON document, got {error}")
        })
    }

    /// Authenticate as `username` with `password` over SCRAM-SHA-256, the exchange a driver
    /// runs, and answer the reply of its last step.
    pub(crate) async fn authenticate(
        &mut self,
        case: &str,
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
            .unwrap_or_else(|_| panic!("case {case}: saslStart answers a conversation id"));
        let server_first = String::from_utf8(payload_of(&started))
            .unwrap_or_else(|_| panic!("case {case}: the server-first message is text"));
        let salt = ::aws_smithy_types::base64::decode(attribute(&server_first, 's'))
            .unwrap_or_else(|_| panic!("case {case}: the salt is base64"));
        let iterations: u32 = attribute(&server_first, 'i')
            .parse()
            .unwrap_or_else(|_| panic!("case {case}: the iteration count is a number"));
        let combined_nonce = attribute(&server_first, 'r').to_string();
        // RFC 5802 over SHA-256: the proof is the client key masked with the signature over
        // the whole conversation, so only a client holding the password can produce it.
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
pub(crate) fn ok(reply: &Document) -> f64 {
    match reply.get("ok") {
        Some(Bson::Double(value)) => *value,
        Some(Bson::Int32(value)) => f64::from(*value),
        Some(Bson::Int64(value)) => *value as f64,
        other => panic!("expected an ok in the reply, got {other:?}"),
    }
}

/// The `code` of a refusal.
pub(crate) fn code(reply: &Document) -> i32 {
    reply
        .get_i32("code")
        .unwrap_or_else(|_| panic!("expected a refusal carrying a code, got {reply:?}"))
}

/// The verifier a client of the emulator's OWN socket uses: the certificate is the one the
/// core generated for its listeners, self-signed and naming loopback, and this client is the
/// emulator's own test connecting to a port the same process opened — so there is no authority
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

/// The endpoint the instance `identifier` publishes: `(address, port)`, read through
/// `DescribeDBInstances` exactly as a client reads it.
async fn published_endpoint(docdb: &Docdb, case: &str, identifier: &str) -> (String, String) {
    let described = docdb
        .ok(
            case,
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", identifier)],
        )
        .await;
    (text(&described, "Address"), text(&described, "Port"))
}

/// A cluster and one member of it, settled past the window the clock owes a new resource its
/// `available` status, with the master user `MASTER_USERNAME` unless `credentials` is false.
async fn cluster_with_member(
    docdb: &Docdb,
    case: &str,
    cluster: &str,
    instance: &str,
    credentials: bool,
    parameter_group: Option<&str>,
) {
    let mut params: Vec<(&str, &str)> = vec![
        ("DBClusterIdentifier", cluster),
        ("Engine", "docdb"),
    ];
    if credentials {
        params.push(("MasterUsername", MASTER_USERNAME));
        params.push(("MasterUserPassword", MASTER_PASSWORD));
    }
    if let Some(group) = parameter_group {
        params.push(("DBClusterParameterGroupName", group));
    }
    docdb.ok(case, "CreateDBCluster", &params).await;
    docdb
        .ok(
            case,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", instance),
                ("DBInstanceClass", "db.r5.large"),
                ("Engine", "docdb"),
                ("DBClusterIdentifier", cluster),
            ],
        )
        .await;
    let_the_clock_catch_up();
}

#[tokio::test]
async fn an_instance_endpoint_serves_the_document_wire_it_published() {
    // RULE: the published endpoint is an address this emulator answers the engine's own wire
    // on. The row below is the whole path a client takes: the legacy handshake, the SASL
    // exchange, then the commands — and the published address is where every one of them is
    // sent.
    let docdb = Docdb::new();
    let case = "a cluster with a master user";
    cluster_with_member(&docdb, case, "wire-cluster", "wire-instance", true, None).await;
    let (address, port) = published_endpoint(&docdb, case, "wire-instance").await;
    let mut session = Session::open_tls(&docdb, case, &address, &port).await;

    // The handshake a driver opens with, in the legacy message it sends before it knows what
    // it is talking to.
    let hello = session
        .legacy_command(
            case,
            "admin",
            doc! { "isMaster": 1_i32, "helloOk": true, "saslSupportedMechs": "admin.docdbadmin" },
        )
        .await;
    assert_eq!(ok(&hello), 1.0, "case {case}: the handshake must succeed, got {hello:?}");
    assert_eq!(
        hello.get_bool("isWritablePrimary").ok(),
        Some(true),
        "case {case}: the cluster's only member is the one it writes through, got {hello:?}"
    );
    assert!(
        hello.get_i32("maxWireVersion").is_ok_and(|version| version > 0),
        "case {case}: the handshake must publish a wire version, got {hello:?}"
    );
    let mechanisms = hello
        .get_array("saslSupportedMechs")
        .unwrap_or_else(|_| panic!("case {case}: the handshake must offer mechanisms, got {hello:?}"));
    assert!(
        mechanisms.contains(&Bson::String("SCRAM-SHA-256".to_string())),
        "case {case}: expected SCRAM-SHA-256 to be offered, got {mechanisms:?}"
    );

    // Before the exchange the session is nobody, so a command that reads or writes documents
    // is refused rather than served.
    let unauthenticated = session
        .command(case, "shop", doc! { "insert": "widgets", "documents": [{ "_id": 1_i32 }] })
        .await;
    assert_eq!(
        (ok(&unauthenticated), code(&unauthenticated)),
        (0.0, 13),
        "case {case}: an unauthenticated write must be refused, got {unauthenticated:?}"
    );

    let authenticated = session
        .authenticate(case, MASTER_USERNAME, MASTER_PASSWORD)
        .await;
    assert_eq!(
        ok(&authenticated),
        1.0,
        "case {case}: the master user's password must authenticate the session, got \
         {authenticated:?}"
    );
    assert_eq!(
        authenticated.get_bool("done").ok(),
        Some(true),
        "case {case}: the exchange must be done once the proof holds, got {authenticated:?}"
    );

    // The commands, over a table of document shapes: the rule generalises over what a
    // document carries, so the rows differ in their fields, their types and their count.
    let rows: Vec<(&str, Document)> = vec![
        ("a document with an _id of its own", doc! { "_id": 1_i32, "name": "left", "stock": 3_i32 }),
        ("a document the engine mints an _id for", doc! { "name": "right", "stock": 7_i32 }),
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
            "case {case} / {row}: the insert must store one document, got {inserted:?}"
        );
    }
    let found = session
        .command(case, "shop", doc! { "find": "widgets", "filter": {} })
        .await;
    let batch = found
        .get_document("cursor")
        .and_then(|cursor| cursor.get_array("firstBatch").cloned())
        .unwrap_or_else(|_| panic!("case {case}: the find must answer a cursor, got {found:?}"));
    assert_eq!(
        batch.len(),
        rows.len(),
        "case {case}: the find must read back every document written, got {batch:?}"
    );
    // The documents came back with the fields they were written with, and the one written
    // without an `_id` came back with the one the engine minted for it.
    for document in &batch {
        let document = document
            .as_document()
            .unwrap_or_else(|| panic!("case {case}: every selected value is a document"));
        assert!(
            document.get("_id").is_some(),
            "case {case}: every stored document carries an _id, got {document:?}"
        );
    }
    let filtered = session
        .command(
            case,
            "shop",
            doc! { "find": "widgets", "filter": { "stock": { "$gt": 5_i32 } } },
        )
        .await;
    let filtered_batch = filtered
        .get_document("cursor")
        .and_then(|cursor| cursor.get_array("firstBatch").cloned())
        .expect("the find answers a cursor");
    assert_eq!(
        filtered_batch.len(),
        1,
        "case {case}: the filter must select only the matching document, got {filtered_batch:?}"
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
        "case {case}: the update must change the document it selected, got {updated:?}"
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
        "case {case}: the count must see the update, got {counted:?}"
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
        "case {case}: the delete must remove the document it selected, got {deleted:?}"
    );
    let after = session
        .command(case, "shop", doc! { "count": "widgets", "query": {} })
        .await;
    assert_eq!(
        after.get_i32("n").ok(),
        Some(i32::try_from(rows.len() - 1).expect("the row count is small")),
        "case {case}: the collection must hold what is left, got {after:?}"
    );
    // The collection the writes created is the one the database reports.
    let collections = session
        .command(case, "shop", doc! { "listCollections": 1_i32 })
        .await;
    let names: Vec<String> = collections
        .get_document("cursor")
        .and_then(|cursor| cursor.get_array("firstBatch").cloned())
        .expect("listCollections answers a cursor")
        .iter()
        .filter_map(|entry| entry.as_document()?.get_str("name").ok().map(str::to_string))
        .collect();
    assert_eq!(
        names,
        vec!["widgets".to_string()],
        "case {case}: the database must report the collection the writes created, got {names:?}"
    );
    // A command this endpoint does not serve is refused by name, never answered as an empty
    // success.
    let unknown = session
        .command(case, "shop", doc! { "mapReduce": "widgets" })
        .await;
    assert_eq!(
        (ok(&unknown), code(&unknown)),
        (0.0, 59),
        "case {case}: a command this endpoint does not serve must be refused, got {unknown:?}"
    );
}

#[tokio::test]
async fn a_cluster_with_tls_turned_off_is_served_in_the_clear() {
    // The `tls` cluster parameter decides the wire: with it disabled the endpoint answers a
    // connection in the clear, and with it at its default the endpoint is encrypted (the row
    // above). Who the session is is decided the same way on either wire: the handshake reaches
    // the endpoint as itself, and everything past it is served only to a session that has
    // authenticated as the cluster's master user.
    let docdb = Docdb::new();
    let case = "a cluster with tls disabled";
    docdb
        .ok(
            case,
            "CreateDBClusterParameterGroup",
            &[
                ("DBClusterParameterGroupName", "plaintext-group"),
                ("DBParameterGroupFamily", "docdb5.0"),
                ("Description", "tls off"),
            ],
        )
        .await;
    docdb
        .ok(
            case,
            "ModifyDBClusterParameterGroup",
            &[
                ("DBClusterParameterGroupName", "plaintext-group"),
                ("Parameters.Parameter.1.ParameterName", "tls"),
                ("Parameters.Parameter.1.ParameterValue", "disabled"),
                ("Parameters.Parameter.1.ApplyMethod", "pending-reboot"),
            ],
        )
        .await;
    cluster_with_member(
        &docdb,
        case,
        "plain-cluster",
        "plain-instance",
        true,
        Some("plaintext-group"),
    )
    .await;
    let (address, port) = published_endpoint(&docdb, case, "plain-instance").await;
    let mut session = Session::open(&docdb, case, &address, &port).await;
    let pinged = session.command(case, "admin", doc! { "ping": 1_i32 }).await;
    assert_eq!(
        ok(&pinged),
        1.0,
        "case {case}: the endpoint must answer a ping in the clear, got {pinged:?}"
    );
    let unauthenticated = session
        .command(
            case,
            "shop",
            doc! { "insert": "widgets", "documents": [{ "_id": 1_i32, "name": "only" }] },
        )
        .await;
    assert_eq!(
        (ok(&unauthenticated), code(&unauthenticated)),
        (0.0, 13),
        "case {case}: a wire in the clear admits no unauthenticated write either, got \
         {unauthenticated:?}"
    );
    assert_eq!(
        ok(&session
            .authenticate(case, MASTER_USERNAME, MASTER_PASSWORD)
            .await),
        1.0,
        "case {case}: the master user must authenticate a session on the wire in the clear"
    );
    let inserted = session
        .command(
            case,
            "shop",
            doc! { "insert": "widgets", "documents": [{ "_id": 1_i32, "name": "only" }] },
        )
        .await;
    assert_eq!(
        (ok(&inserted), inserted.get_i32("n").ok()),
        (1.0, Some(1)),
        "case {case}: the authenticated session is served in the clear, got {inserted:?}"
    );
    let found = session
        .command(case, "shop", doc! { "find": "widgets", "filter": { "name": "only" } })
        .await;
    let batch = found
        .get_document("cursor")
        .and_then(|cursor| cursor.get_array("firstBatch").cloned())
        .expect("the find answers a cursor");
    assert_eq!(
        batch.len(),
        1,
        "case {case}: the session must read back what it wrote, got {batch:?}"
    );
}

#[tokio::test]
async fn an_endpoint_whose_cluster_recorded_no_credential_admits_no_session() {
    // RULE: a command outside the handshake is served only to a session that has
    // AUTHENTICATED on this endpoint. A cluster that recorded no master credential has nothing
    // a client could authenticate against, so its endpoint answers the handshake and serves
    // nothing else — a socket this emulator opened never hands a cluster's documents to a
    // client that presented nothing.
    let docdb = Docdb::new();
    let case = "a cluster that recorded no master credential";
    docdb
        .ok(
            case,
            "CreateDBClusterParameterGroup",
            &[
                ("DBClusterParameterGroupName", "anonymous-group"),
                ("DBParameterGroupFamily", "docdb5.0"),
                ("Description", "tls off"),
            ],
        )
        .await;
    docdb
        .ok(
            case,
            "ModifyDBClusterParameterGroup",
            &[
                ("DBClusterParameterGroupName", "anonymous-group"),
                ("Parameters.Parameter.1.ParameterName", "tls"),
                ("Parameters.Parameter.1.ParameterValue", "disabled"),
                ("Parameters.Parameter.1.ApplyMethod", "pending-reboot"),
            ],
        )
        .await;
    cluster_with_member(
        &docdb,
        case,
        "anonymous-cluster",
        "anonymous-instance",
        false,
        Some("anonymous-group"),
    )
    .await;
    let (address, port) = published_endpoint(&docdb, case, "anonymous-instance").await;
    let mut session = Session::open(&docdb, case, &address, &port).await;
    // The handshake and the liveness check reach the endpoint as itself: a driver sends them
    // before it has authenticated anything.
    assert_eq!(
        ok(&session.command(case, "admin", doc! { "ping": 1_i32 }).await),
        1.0,
        "case {case}: the handshake must reach the endpoint"
    );
    // Every command past it is refused, whichever kind it is.
    for (which, command) in [
        (
            "a write",
            doc! { "insert": "widgets", "documents": [{ "_id": 1_i32 }] },
        ),
        ("a read", doc! { "find": "widgets", "filter": {} }),
        ("a drop", doc! { "drop": "widgets" }),
    ] {
        let refused = session.command(case, "shop", command).await;
        assert_eq!(
            (ok(&refused), code(&refused)),
            (0.0, 13),
            "case {case}: {which} on a session that authenticated against nothing must be \
             refused, got {refused:?}"
        );
    }
}

#[tokio::test]
async fn a_wrong_password_does_not_authenticate_a_session() {
    // The exchange proves the client holds the cluster's master password, so a proof computed
    // from any other password does not hold — and the session stays nobody.
    let docdb = Docdb::new();
    let case = "the wrong password";
    cluster_with_member(&docdb, case, "auth-cluster", "auth-instance", true, None).await;
    let (address, port) = published_endpoint(&docdb, case, "auth-instance").await;
    for (row, username, password) in [
        ("another password", MASTER_USERNAME, "another-Secret-99"),
        ("another user name", "someoneelse", MASTER_PASSWORD),
    ] {
        let mut session = Session::open_tls(&docdb, row, &address, &port).await;
        let refused = session.authenticate(row, username, password).await;
        assert_eq!(
            (ok(&refused), code(&refused)),
            (0.0, 18),
            "case {row}: the session must not be authenticated, got {refused:?}"
        );
        let written = session
            .command(row, "shop", doc! { "insert": "widgets", "documents": [{ "_id": 1_i32 }] })
            .await;
        assert_eq!(
            (ok(&written), code(&written)),
            (0.0, 13),
            "case {row}: a session that did not authenticate may write nothing, got {written:?}"
        );
    }
}

#[tokio::test]
async fn every_member_is_an_address_onto_its_clusters_documents() {
    // "Each Amazon DocumentDB cluster hosts copies of its data": a member is a second address
    // onto its cluster's storage, so what the writer wrote is what a replica reads — and a
    // replica serves reads only, which is what the engine's own `NotWritablePrimary` says.
    let docdb = Docdb::new();
    let case = "two members of one cluster";
    cluster_with_member(&docdb, case, "members-cluster", "members-writer", true, None).await;
    docdb
        .ok(
            case,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "members-replica"),
                ("DBInstanceClass", "db.r5.large"),
                ("Engine", "docdb"),
                ("DBClusterIdentifier", "members-cluster"),
            ],
        )
        .await;
    let_the_clock_catch_up();
    let (writer_address, writer_port) = published_endpoint(&docdb, case, "members-writer").await;
    let (replica_address, replica_port) = published_endpoint(&docdb, case, "members-replica").await;
    assert_ne!(
        (writer_address.as_str(), writer_port.as_str()),
        (replica_address.as_str(), replica_port.as_str()),
        "case {case}: each member is reached at an endpoint of its own"
    );

    let mut writer = Session::open_tls(&docdb, case, &writer_address, &writer_port).await;
    assert_eq!(
        ok(&writer
            .authenticate(case, MASTER_USERNAME, MASTER_PASSWORD)
            .await),
        1.0,
        "case {case}: the writer must authenticate the cluster's master user"
    );
    let inserted = writer
        .command(
            case,
            "shop",
            doc! { "insert": "widgets", "documents": [{ "_id": 1_i32, "name": "shared" }] },
        )
        .await;
    assert_eq!(
        ok(&inserted),
        1.0,
        "case {case}: the writer must take the write, got {inserted:?}"
    );

    let mut replica = Session::open_tls(&docdb, case, &replica_address, &replica_port).await;
    let hello = replica.command(case, "admin", doc! { "hello": 1_i32 }).await;
    assert_eq!(
        hello.get_bool("isWritablePrimary").ok(),
        Some(false),
        "case {case}: a replica must report itself as no writer, got {hello:?}"
    );
    assert_eq!(
        ok(&replica
            .authenticate(case, MASTER_USERNAME, MASTER_PASSWORD)
            .await),
        1.0,
        "case {case}: a replica authenticates the same master user as the writer"
    );
    let read = replica
        .command(case, "shop", doc! { "find": "widgets", "filter": { "name": "shared" } })
        .await;
    let batch = read
        .get_document("cursor")
        .and_then(|cursor| cursor.get_array("firstBatch").cloned())
        .expect("the find answers a cursor");
    assert_eq!(
        batch.len(),
        1,
        "case {case}: the replica must read what the writer wrote, got {batch:?}"
    );
    let refused = replica
        .command(case, "shop", doc! { "insert": "widgets", "documents": [{ "_id": 2_i32 }] })
        .await;
    assert_eq!(
        (ok(&refused), code(&refused)),
        (0.0, 10107),
        "case {case}: a replica serves reads only, got {refused:?}"
    );
}

#[tokio::test]
async fn a_unique_index_refuses_a_duplicate_and_a_cursor_pages_a_result() {
    // Two rules of the data itself: a unique index refuses a second document with the same
    // key, and a result bigger than one batch is handed over through a cursor.
    let docdb = Docdb::new();
    let case = "indexes and cursors";
    cluster_with_member(&docdb, case, "index-cluster", "index-instance", true, None).await;
    let (address, port) = published_endpoint(&docdb, case, "index-instance").await;
    let mut session = Session::open_tls(&docdb, case, &address, &port).await;
    assert_eq!(
        ok(&session
            .authenticate(case, MASTER_USERNAME, MASTER_PASSWORD)
            .await),
        1.0,
        "case {case}: the session must authenticate before the engine serves it"
    );

    let built = session
        .command(
            case,
            "shop",
            doc! {
                "createIndexes": "widgets",
                "indexes": [{ "key": { "sku": 1_i32 }, "name": "sku_1", "unique": true }],
            },
        )
        .await;
    assert_eq!(
        ok(&built),
        1.0,
        "case {case}: the index must be built, got {built:?}"
    );
    for (row, document, expected_errors) in [
        ("the first document", doc! { "_id": 1_i32, "sku": "a" }, 0),
        ("a second document with the same key", doc! { "_id": 2_i32, "sku": "a" }, 1),
        ("a document with another key", doc! { "_id": 3_i32, "sku": "b" }, 0),
    ] {
        let inserted = session
            .command(case, "shop", doc! { "insert": "widgets", "documents": [document] })
            .await;
        let errors = inserted
            .get_array("writeErrors")
            .map(|errors| errors.len())
            .unwrap_or_default();
        assert_eq!(
            errors, expected_errors,
            "case {case} / {row}: expected {expected_errors} write errors, got {inserted:?}"
        );
        if expected_errors > 0 {
            let refusal = inserted
                .get_array("writeErrors")
                .expect("the refusal is reported")
                .first()
                .and_then(Bson::as_document)
                .cloned()
                .expect("the refusal is a document");
            assert_eq!(
                refusal.get_i32("code").ok(),
                Some(11000),
                "case {case} / {row}: expected the duplicate-key code, got {refusal:?}"
            );
        }
    }

    // A result bigger than the batch the client asked for is paged: the cursor the find
    // answers with is the one `getMore` reads the rest from.
    for number in 10..20_i32 {
        session
            .command(
                case,
                "shop",
                doc! { "insert": "pages", "documents": [{ "_id": number }] },
            )
            .await;
    }
    let first = session
        .command(
            case,
            "shop",
            doc! { "find": "pages", "filter": {}, "batchSize": 4_i32 },
        )
        .await;
    let cursor = first
        .get_document("cursor")
        .cloned()
        .unwrap_or_else(|_| panic!("case {case}: the find answers a cursor, got {first:?}"));
    assert_eq!(
        cursor.get_array("firstBatch").map(Vec::len).ok(),
        Some(4),
        "case {case}: the first batch must be the size the client asked for, got {cursor:?}"
    );
    let cursor_id = cursor
        .get_i64("id")
        .unwrap_or_else(|_| panic!("case {case}: the cursor carries an id, got {cursor:?}"));
    assert_ne!(
        cursor_id, 0,
        "case {case}: a result with more to come must leave the cursor open, got {cursor:?}"
    );
    let mut read = 4_usize;
    let mut open = cursor_id;
    while open != 0 {
        let more = session
            .command(
                case,
                "shop",
                doc! { "getMore": open, "collection": "pages", "batchSize": 4_i32 },
            )
            .await;
        let cursor = more
            .get_document("cursor")
            .cloned()
            .unwrap_or_else(|_| panic!("case {case}: getMore answers a cursor, got {more:?}"));
        read += cursor
            .get_array("nextBatch")
            .map(Vec::len)
            .unwrap_or_default();
        open = cursor.get_i64("id").unwrap_or_default();
    }
    assert_eq!(
        read, 10,
        "case {case}: the cursor must hand over every document the find selected, got {read}"
    );
}

#[tokio::test]
async fn the_socket_closes_with_the_instance() {
    // The endpoint is the resource: deleting the instance closes the socket it published, so
    // the address a deleted instance published is one nothing answers at any more — exactly
    // as a client of an address nothing listens on is answered.
    let docdb = Docdb::new();
    let case = "a deleted instance";
    cluster_with_member(&docdb, case, "closing-cluster", "closing-instance", false, None).await;
    let (address, port) = published_endpoint(&docdb, case, "closing-instance").await;
    // It answers while the instance is there.
    let mut session = Session::open_tls(&docdb, case, &address, &port).await;
    let pinged = session.command(case, "admin", doc! { "ping": 1_i32 }).await;
    assert_eq!(
        ok(&pinged),
        1.0,
        "case {case}: the endpoint must answer while the instance is there, got {pinged:?}"
    );
    docdb
        .ok(
            case,
            "DeleteDBInstance",
            &[("DBInstanceIdentifier", "closing-instance")],
        )
        .await;
    // The socket and the address it answered at go with the handle the delete dropped: the
    // register no longer serves it, and a client dialling it is answered as one of an address
    // nothing listens on.
    let dialable: u16 = port.parse().expect("the published port is a port");
    let planes = DialablePlanes::of(&**docdb.engine())
        .expect("the engine vends the register of served data planes");
    let authority = ::service_common::data_plane_addressing::host_port_authority(&address, dialable);
    assert_eq!(
        planes.served_by(&authority),
        None,
        "case {case}: {authority} must answer nothing once the instance is deleted"
    );
    // The port goes with the listener the delete dropped. Polled for, rather than waited out:
    // the accept loop is torn down asynchronously, so the test looks again until the dial is
    // refused or a deadline far longer than that teardown takes has passed.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut refused = false;
    while std::time::Instant::now() < deadline {
        // `127.0.0.1`: this machine's own loopback address, which is the host the endpoint
        // published (asserted when the session above was opened on it).
        if TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, dialable)).await.is_err() {
            refused = true;
            break;
        }
        ::tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(
        refused,
        "case {case}: {authority} must stop accepting connections once the instance is deleted"
    );
}

#[tokio::test]
async fn an_aggregate_is_bounded_by_the_memory_it_would_materialise() {
    // RULE: a pipeline is bounded by the MEMORY it would build as well as by how many
    // documents it visits. An accumulator's argument is a client expression resolved once per
    // member of every group, so the bytes one command materialises are the product of the
    // documents it visits, the accumulators over each and the SIZE of that expression — and a
    // command past that product is refused instead of being built.
    //
    // The rows differ in that one factor, the expression's size: the same pipeline over the
    // same documents is served with a small expression and refused with one large enough that
    // resolving it per document would outgrow the ceiling.
    let docdb = Docdb::new();
    let case = "the memory a pipeline would materialise";
    cluster_with_member(&docdb, case, "budget-cluster", "budget-instance", true, None).await;
    let (address, port) = published_endpoint(&docdb, case, "budget-instance").await;
    let mut session = Session::open_tls(&docdb, case, &address, &port).await;
    assert_eq!(
        ok(&session
            .authenticate(case, MASTER_USERNAME, MASTER_PASSWORD)
            .await),
        1.0,
        "case {case}: the session must authenticate before the engine serves it"
    );
    // The documents the pipeline visits, each tiny: the size under test is the EXPRESSION's.
    let documents: Vec<Document> = (0..50)
        .map(|index| doc! { "_id": index, "group": "one" })
        .collect();
    let inserted = session
        .command(
            case,
            "shop",
            doc! { "insert": "ledger", "documents": documents },
        )
        .await;
    assert_eq!(
        (ok(&inserted), inserted.get_i32("n").ok()),
        (1.0, Some(50)),
        "case {case}: the documents the pipeline reads must be stored, got {inserted:?}"
    );
    for (row, literal_bytes, served) in [
        ("an expression of a few bytes", 8_usize, true),
        // 50 documents x one accumulator x ~400 KB resolves to ~20 MB, past the ceiling one
        // command may materialise.
        ("an expression of 400 KB", 400 * 1024, false),
    ] {
        let answered = session
            .command(
                case,
                "shop",
                doc! {
                    "aggregate": "ledger",
                    "pipeline": [
                        {
                            "$group": {
                                "_id": "$group",
                                "collected": { "$push": "x".repeat(literal_bytes) },
                            }
                        }
                    ],
                    "cursor": {},
                },
            )
            .await;
        match served {
            true => assert_eq!(
                ok(&answered),
                1.0,
                "case {case} ({row}): a pipeline inside the budget must be served, got \
                 {answered:?}"
            ),
            false => assert_eq!(
                (ok(&answered), code(&answered)),
                (0.0, 8),
                "case {case} ({row}): a pipeline that would outgrow the budget must be \
                 refused, got the reply's own code"
            ),
        }
    }

    // The other half of the same rule, which the SIZE of the expression cannot show: a field
    // path of a few bytes resolves to whatever the document holds, so the budget is charged
    // for the VALUE and not for the expression. Twenty documents each carrying ~900 KB are
    // stored in batches, and the same `$push` over the field they carry is refused.
    let payload = "y".repeat(900 * 1024);
    for batch in 0..5 {
        let documents: Vec<Document> = (0..4)
            .map(|index| {
                doc! { "_id": batch * 4 + index, "group": "one", "payload": payload.clone() }
            })
            .collect();
        let stored = session
            .command(
                case,
                "shop",
                doc! { "insert": "payloads", "documents": documents },
            )
            .await;
        assert_eq!(
            (ok(&stored), stored.get_i32("n").ok()),
            (1.0, Some(4)),
            "case {case}: the documents the field path reads must be stored, got {stored:?}"
        );
    }
    let answered = session
        .command(
            case,
            "shop",
            doc! {
                "aggregate": "payloads",
                "pipeline": [
                    { "$group": { "_id": "$group", "collected": { "$push": "$payload" } } }
                ],
                "cursor": {},
            },
        )
        .await;
    assert_eq!(
        (ok(&answered), code(&answered)),
        (0.0, 8),
        "case {case} (a field path over large documents): the budget is charged for the value \
         the expression resolves to, so this must be refused, got the reply's own code"
    );
}
