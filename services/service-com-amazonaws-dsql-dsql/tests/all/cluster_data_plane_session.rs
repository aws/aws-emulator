// SPDX-License-Identifier: CC0-1.0

//! The cluster's DATA plane: the PostgreSQL endpoint a cluster that reports
//! `ACTIVE` answers on, and the auth token a session is admitted with.
//!
//! The RULE under test: once a cluster's status says it is up, the address this
//! emulator published for it answers the PostgreSQL frontend/backend protocol
//! for THAT cluster — a client completes the startup as a database role with an
//! auth token as its password, over a connection it may upgrade in band, and
//! runs SQL whose answers come from that cluster's own storage. The token
//! authenticates the CONNECTION: one presented after its lifetime has run out is
//! refused at connect, a session opened before it ran out keeps serving
//! statements, and a freshly minted token opens a working session. A cluster that
//! has not finished coming up admits no session, and a cluster the clock has
//! finished deleting answers nothing at all.
//!
//! Everything here runs in ONE process: the listener is opened by the
//! `CreateCluster` call below, on the core's own loopback interface, and the
//! client is written against the wire itself rather than against an in-process
//! shortcut — so what is driven is the protocol a stock libpq client speaks.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::harness::Dsql;

/// The database role a cluster has from the moment it is created, and the one
/// database it holds.
const ADMIN_USER: &str = "admin";
const CLUSTER_DATABASE: &str = "postgres";

/// How long a case waits for a cluster to reach a status before it gives up:
/// far longer than the span this service owes a transition in, so the wait is on
/// the CONDITION and not on a clock.
const SETTLE_DEADLINE: Duration = Duration::from_secs(120);

/// How often a wait looks again.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The sentence a connection this cluster does not accept is refused with, and
/// the `SQLSTATE` it carries.
const ACCESS_DENIED: &str = "unable to accept connection, access denied";
const ACCESS_DENIED_STATE: &str = "28000";

/// One emulator, the engine its store lives in, and one cluster on it.
struct Cluster {
    dsql: Dsql,
    engine: Arc<engine::InMemoryEngine>,
    identifier: String,
    /// The host the control plane published as the cluster's `endpoint`, which
    /// is what an auth token for this cluster is presigned against.
    endpoint: String,
}

impl Cluster {
    /// Create a cluster and read back the endpoint it published.
    async fn created() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let dsql = Dsql::sharing(&engine, "us-east-1");
        let created = dsql.create(json!({"deletionProtectionEnabled": false})).await;
        assert_eq!(
            created.status, 200,
            "CreateCluster must answer 200, got {} {:?}",
            created.status, created.body
        );
        let endpoint = created.str("endpoint");
        assert!(
            !endpoint.is_empty(),
            "CreateCluster must publish a connection endpoint, got {:?}",
            created.body
        );
        Self {
            dsql,
            engine,
            identifier: created.str("identifier"),
            endpoint,
        }
    }

    /// Wait until `GetCluster` reports `status`, then answer the endpoint the
    /// cluster publishes. The wait is on the status itself, bounded by a
    /// deadline far past the span the service owes the transition in.
    async fn waited_for(&self, status: &str) -> String {
        let deadline = SystemTime::now() + SETTLE_DEADLINE;
        loop {
            let read = self.dsql.get(&self.identifier).await;
            if read.status == 200 && read.str("status") == status {
                return read.str("endpoint");
            }
            assert!(
                SystemTime::now() < deadline,
                "the cluster {:?} must reach {status}, last read {} {:?}",
                self.identifier,
                read.status,
                read.body
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// The port the core assigned this cluster's listener — the port a client on
    /// this machine reaches the published endpoint's PostgreSQL wire at.
    ///
    /// Read out of the store the control plane holds the plane on, which is the
    /// only place this emulator can report it: the model's `endpoint` member
    /// carries a host and no port (`^[a-zA-Z0-9.-]+$`), and the core assigns
    /// every listener's port itself.
    fn served_port(&self) -> u16 {
        let state = service_com_amazonaws_dsql_dsql::state::service_state(&*self.engine)
            .expect("the shared engine vends this service's store");
        let held = state.lock();
        let plane = held
            .cluster_planes
            .get(&self.identifier)
            .unwrap_or_else(|| panic!("the cluster {:?} must have an open data plane", self.identifier));
        assert_eq!(
            plane.endpoint_host(),
            self.endpoint,
            "the plane must be opened for the host the control plane published"
        );
        u16::try_from(plane.served_endpoint().port).expect("a listener port fits")
    }
}

/// RULE: an `ACTIVE` cluster's published endpoint serves the PostgreSQL wire for
/// that cluster — in the clear and over the in-band TLS upgrade alike — and the
/// statements of a session run on that cluster's own storage.
///
/// The rows span how a session is opened: in the clear, and with the client's
/// `SSLRequest` answered and the handshake run on the same port.
#[tokio::test]
async fn an_active_clusters_endpoint_serves_a_postgresql_session() {
    let cluster = Cluster::created().await;
    let endpoint = cluster.waited_for("ACTIVE").await;
    let port = cluster.served_port();

    for (case, transport) in [
        ("a session in the clear", Transport::Plaintext),
        ("TLS negotiated in band", Transport::Tls),
    ] {
        let token = auth_token(
            "DbConnectAdmin",
            &endpoint,
            SystemTime::now(),
            Duration::from_secs(900),
        );
        let mut session = login(
            case,
            port,
            transport,
            ADMIN_USER,
            &token,
            Some(CLUSTER_DATABASE),
        )
        .await
        .unwrap_or_else(|refusal| {
            panic!("case {case}: the endpoint must open a session, got {refusal:?}")
        });

        // The session runs as the role the token was minted for, on the one
        // database a cluster holds.
        for (statement, expected) in [
            ("SELECT current_user", ADMIN_USER),
            ("SELECT current_database()", CLUSTER_DATABASE),
            ("SELECT 1", "1"),
        ] {
            let answered = session
                .query(statement)
                .await
                .unwrap_or_else(|refusal| panic!("case {case}: {statement} must run, got {refusal:?}"));
            assert_eq!(
                answered.only(),
                Some(expected),
                "case {case}: {statement} must answer {expected:?}, got {:?}",
                answered.only()
            );
        }

        // The backend reports a PostgreSQL version, which is what a driver reads
        // to decide how to talk to it.
        let version = session
            .parameter("server_version")
            .unwrap_or_else(|| panic!("case {case}: the backend must report server_version"));
        assert!(
            version.starts_with("16"),
            "case {case}: the cluster must report the PostgreSQL version it is compatible \
             with, got {version:?}"
        );

        // The statements of a session are served by the cluster's OWN storage: a
        // table written here is read back here.
        for statement in [
            "CREATE TABLE orders (id INTEGER PRIMARY KEY, note TEXT)",
            "INSERT INTO orders (id, note) VALUES (1, 'first')",
        ] {
            session
                .query(statement)
                .await
                .unwrap_or_else(|refusal| panic!("case {case}: {statement} must run, got {refusal:?}"));
        }
        let read_back = session
            .query("SELECT note FROM orders WHERE id = 1")
            .await
            .unwrap_or_else(|refusal| panic!("case {case}: the row must read back, got {refusal:?}"));
        assert_eq!(
            read_back.only(),
            Some("first"),
            "case {case}: the row written over this session must read back, got {:?}",
            read_back.only()
        );
        session
            .query("DROP TABLE orders")
            .await
            .unwrap_or_else(|refusal| panic!("case {case}: DROP TABLE must run, got {refusal:?}"));
    }
}

/// RULE: the auth token authenticates the CONNECTION. A token presented after
/// its lifetime has run out is refused at connect, naming the instant it ran out;
/// the session opened before it ran out keeps serving statements; and a freshly
/// minted token opens a working session.
#[tokio::test]
async fn an_expired_token_is_refused_at_connect_while_an_open_session_survives() {
    let cluster = Cluster::created().await;
    let endpoint = cluster.waited_for("ACTIVE").await;
    let port = cluster.served_port();

    // A session opened with a token that is good now. Its lifetime is short, so
    // it has run out by the time the refusals below are attempted — and the
    // session is not touched by that.
    let lifetime = Duration::from_secs(20);
    let minted_at = SystemTime::now();
    let token = auth_token("DbConnectAdmin", &endpoint, minted_at, lifetime);
    let mut open = login(
        "a token inside its lifetime",
        port,
        Transport::Tls,
        ADMIN_USER,
        &token,
        Some(CLUSTER_DATABASE),
    )
    .await
    .expect("a token inside its lifetime opens a session");

    // The same token, presented past its lifetime: the clock is not waited for —
    // the token is minted as one signed that long ago, which is the same
    // credential a client would present late.
    let spent = auth_token(
        "DbConnectAdmin",
        &endpoint,
        minted_at - (lifetime + Duration::from_secs(15)),
        lifetime,
    );
    let refusal = login(
        "a token past its lifetime",
        port,
        Transport::Tls,
        ADMIN_USER,
        &spent,
        Some(CLUSTER_DATABASE),
    )
    .await
    .expect_err("a token past its lifetime opens no session");
    assert_eq!(
        refusal.message, ACCESS_DENIED,
        "case expired: the refusal must be {ACCESS_DENIED:?}, got {:?}",
        refusal.message
    );
    assert_eq!(
        refusal.severity, "FATAL",
        "case expired: the refusal must end the connection, got {:?}",
        refusal.severity
    );
    assert_eq!(
        refusal.code, ACCESS_DENIED_STATE,
        "case expired: expected SQLSTATE {ACCESS_DENIED_STATE}, got {:?}",
        refusal.code
    );
    assert!(
        refusal.hint.starts_with("Signature expired: "),
        "case expired: the refusal must say the signature expired, got {:?}",
        refusal.hint
    );

    // The session opened before the lifetime ran out is untouched.
    let still_serving = open
        .query("SELECT current_user")
        .await
        .expect("the session opened before expiry keeps serving statements");
    assert_eq!(
        still_serving.only(),
        Some(ADMIN_USER),
        "case open session: it must still answer as {ADMIN_USER:?}, got {:?}",
        still_serving.only()
    );

    // And a freshly minted token opens a working session again.
    let fresh = auth_token(
        "DbConnectAdmin",
        &endpoint,
        SystemTime::now(),
        Duration::from_secs(900),
    );
    let mut reopened = login(
        "a freshly minted token",
        port,
        Transport::Tls,
        ADMIN_USER,
        &fresh,
        Some(CLUSTER_DATABASE),
    )
    .await
    .expect("a freshly minted token opens a session");
    let answered = reopened
        .query("SELECT 1")
        .await
        .expect("the reopened session serves statements");
    assert_eq!(
        answered.only(),
        Some("1"),
        "case a freshly minted token: SELECT 1 must answer 1, got {:?}",
        answered.only()
    );
}

/// RULE: only a credential minted FOR this cluster and for a role the cluster
/// HOLDS opens a session; everything else is refused, and the hint says which of
/// the token's facts failed.
///
/// The rows span the class of logins that are not this cluster's: no credential at
/// all, an ordinary password, a token minted for the other action, a token minted
/// against another cluster's host, and a well-formed connect token naming a role the
/// cluster does not hold.
#[tokio::test]
async fn a_credential_that_is_not_this_clusters_token_opens_no_session() {
    let cluster = Cluster::created().await;
    let endpoint = cluster.waited_for("ACTIVE").await;
    let port = cluster.served_port();
    let at = SystemTime::now();
    let lifetime = Duration::from_secs(900);

    // (case, the user, the password presented, the message expected, the hint expected)
    let rows: Vec<(&str, &str, String, &str, Option<String>)> = vec![
        (
            // The cluster holds no password of its own, so a startup that presents
            // no credential at all is refused exactly as any other non-token is:
            // nothing on this endpoint is served to an unauthenticated caller.
            "no credential at all",
            ADMIN_USER,
            String::new(),
            ACCESS_DENIED,
            None,
        ),
        (
            "an ordinary password",
            ADMIN_USER,
            "Pw9#xZq!notatoken".to_string(),
            ACCESS_DENIED,
            None,
        ),
        (
            "a connect token presented for the admin role",
            ADMIN_USER,
            auth_token("DbConnect", &endpoint, at, lifetime),
            ACCESS_DENIED,
            Some("Wrong user to action mapping. user: admin, action: DbConnect".to_string()),
        ),
        (
            "an admin token presented for another role",
            "appuser",
            auth_token("DbConnectAdmin", &endpoint, at, lifetime),
            ACCESS_DENIED,
            Some("Wrong user to action mapping. user: appuser, action: DbConnectAdmin".to_string()),
        ),
        (
            "a token minted against another cluster's host",
            ADMIN_USER,
            auth_token("DbConnectAdmin", "other-cluster.example", at, lifetime),
            ACCESS_DENIED,
            None,
        ),
        (
            // The token is this cluster's and names the connect action its role
            // is bound to; the cluster simply holds no such role, which is what
            // the refusal says.
            "a connect token naming a role the cluster does not hold",
            "appuser",
            auth_token("DbConnect", &endpoint, at, lifetime),
            "role \"appuser\" does not exist",
            None,
        ),
    ];
    for (case, user, password, message, hint) in rows {
        let refusal = match login(
            case,
            port,
            Transport::Tls,
            user,
            &password,
            Some(CLUSTER_DATABASE),
        )
        .await
        {
            Ok(_session) => {
                panic!("case {case}: the login must be refused, but a session was opened")
            }
            Err(refusal) => refusal,
        };
        assert_eq!(
            refusal.message, message,
            "case {case}: the refusal must be {message:?}, got {:?}",
            refusal.message
        );
        assert_eq!(
            refusal.severity, "FATAL",
            "case {case}: the refusal must end the connection, got {:?}",
            refusal.severity
        );
        assert_eq!(
            refusal.code, ACCESS_DENIED_STATE,
            "case {case}: expected SQLSTATE {ACCESS_DENIED_STATE}, got {:?}",
            refusal.code
        );
        let reported = (!refusal.hint.is_empty()).then(|| refusal.hint.clone());
        assert_eq!(
            reported, hint,
            "case {case}: expected the hint {hint:?}, got {reported:?}"
        );
    }
}

/// RULE: the endpoint follows the CLUSTER. A cluster that has not finished coming
/// up admits no session even with a good token, and a cluster the clock has
/// finished deleting answers nothing at all — its port is closed.
#[tokio::test]
async fn the_endpoint_admits_a_session_only_while_the_cluster_is_up() {
    let cluster = Cluster::created().await;
    let port = cluster.served_port();
    // Straight after the create the cluster is still coming up, and the wire says
    // so rather than opening a session: `cannot_connect_now`, whose sentence is
    // the one PostgreSQL uses for a server that has not finished starting.
    let creating = cluster.dsql.get(&cluster.identifier).await;
    assert_eq!(
        creating.str("status"),
        "CREATING",
        "the cluster must still be coming up straight after the create, got {:?}",
        creating.body
    );
    let token = auth_token(
        "DbConnectAdmin",
        &cluster.endpoint,
        SystemTime::now(),
        Duration::from_secs(900),
    );
    let refusal = login(
        "a cluster still coming up",
        port,
        Transport::Tls,
        ADMIN_USER,
        &token,
        Some(CLUSTER_DATABASE),
    )
    .await
    .expect_err("a cluster that has not finished coming up opens no session");
    assert_eq!(
        refusal.code, "57P03",
        "case coming up: expected SQLSTATE 57P03 (cannot_connect_now), got {:?} {:?}",
        refusal.code, refusal.message
    );

    // Once it is up the same credential opens a session, and once the clock has
    // finished the deletion the port is closed.
    let endpoint = cluster.waited_for("ACTIVE").await;
    let token = auth_token(
        "DbConnectAdmin",
        &endpoint,
        SystemTime::now(),
        Duration::from_secs(900),
    );
    login(
        "an active cluster",
        port,
        Transport::Tls,
        ADMIN_USER,
        &token,
        Some(CLUSTER_DATABASE),
    )
    .await
    .expect("an active cluster opens a session");

    let deleted = cluster.dsql.delete(&cluster.identifier).await;
    assert_eq!(
        deleted.status, 200,
        "DeleteCluster must answer 200, got {} {:?}",
        deleted.status, deleted.body
    );
    let deadline = SystemTime::now() + SETTLE_DEADLINE;
    loop {
        let gone = cluster.dsql.get(&cluster.identifier).await.status == 404;
        let closed = TcpStream::connect(("127.0.0.1", port)).await.is_err();
        if gone && closed {
            break;
        }
        assert!(
            SystemTime::now() < deadline,
            "the deleted cluster must stop answering: gone={gone}, port closed={closed}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

// --- the auth token --------------------------------------------------------

/// An auth token for `action` against `host`, signed at `at` for `lifetime` —
/// the presigned request the client-side presigner
/// (`GenerateDbConnectAdminAuthToken` / `GenerateDbConnectAuthToken`) answers,
/// which a client sends as the PostgreSQL password.
fn auth_token(action: &str, host: &str, at: SystemTime, lifetime: Duration) -> String {
    format!(
        "{host}/?Action={action}&X-Amz-Algorithm=AWS4-HMAC-SHA256\
         &X-Amz-Credential=AKIDTESTTESTTESTTEST%2F20260101%2Fus-east-1%2Fdsql%2Faws4_request\
         &X-Amz-Date={}&X-Amz-Expires={}&X-Amz-SignedHeaders=host\
         &X-Amz-Signature=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        amz_date(at),
        lifetime.as_secs()
    )
}

/// `at` in the ISO 8601 basic form a presigned AWS request writes an instant in.
fn amz_date(at: SystemTime) -> String {
    let seconds = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("an instant after the epoch")
        .as_secs();
    ::chrono::DateTime::from_timestamp(i64::try_from(seconds).expect("the instant fits"), 0)
        .expect("the instant is a date")
        .format("%Y%m%dT%H%M%SZ")
        .to_string()
}

// --- the PostgreSQL client -------------------------------------------------

/// How a session is opened: in the clear, or with the wire's own in-band TLS
/// upgrade on the same port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transport {
    Plaintext,
    Tls,
}

/// A connection to the cluster endpoint, in the clear or upgraded to TLS.
trait Connection: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> Connection for T {}

/// What a client holds: the stream, whatever it is now.
type Client = Box<dyn Connection>;

/// A refusal the server sent: the severity, the SQLSTATE a client branches on,
/// the sentence it reports and the hint beside it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Refusal {
    severity: String,
    code: String,
    message: String,
    hint: String,
}

/// One result set a client read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Rows {
    columns: Vec<String>,
    rows: Vec<Vec<Option<String>>>,
}

impl Rows {
    /// The one value a single-row, single-column answer carries.
    fn only(&self) -> Option<&str> {
        self.rows
            .first()
            .and_then(|row| row.first())
            .and_then(Option::as_deref)
    }
}

/// A session a client has opened, and the parameters the backend reported while
/// it opened.
struct PostgresSession {
    conn: Client,
    parameters: Vec<(String, String)>,
}

/// Written by hand rather than derived: the stream carries no `Debug`, and what
/// a failing assertion has to say about an open session is what the backend
/// reported while it opened.
impl std::fmt::Debug for PostgresSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PostgresSession")
            .field("parameters", &self.parameters)
            .finish_non_exhaustive()
    }
}

impl PostgresSession {
    /// The value the backend reported for the parameter `name`.
    fn parameter(&self, name: &str) -> Option<String> {
        self.parameters
            .iter()
            .find(|(held, _)| held == name)
            .map(|(_, value)| value.clone())
    }

    /// Run one simple query and read everything the server answers for it.
    async fn query(&mut self, sql: &str) -> Result<Rows, Refusal> {
        let mut message = Vec::new();
        put_cstring(&mut message, sql);
        send(&mut self.conn, b'Q', &message).await;
        let mut answer = Rows::default();
        let mut refusal = None;
        loop {
            let Some((kind, body)) = try_read(&mut self.conn).await else {
                return Err(refusal.unwrap_or(Refusal {
                    severity: "FATAL".to_string(),
                    code: "08006".to_string(),
                    message: "the server closed the connection".to_string(),
                    hint: String::new(),
                }));
            };
            match kind {
                // RowDescription.
                b'T' => {
                    let count = usize::from(u16::from_be_bytes([body[0], body[1]]));
                    let mut at = 2usize;
                    answer.columns.clear();
                    for _ in 0..count {
                        let (name, next) = read_cstring(&body, at);
                        answer.columns.push(name);
                        // name, table oid, column number, type oid, size,
                        // modifier, format code.
                        at = next + 18;
                    }
                }
                // DataRow.
                b'D' => {
                    let count = usize::from(u16::from_be_bytes([body[0], body[1]]));
                    let mut at = 2usize;
                    let mut row = Vec::with_capacity(count);
                    for _ in 0..count {
                        let length =
                            i32::from_be_bytes([body[at], body[at + 1], body[at + 2], body[at + 3]]);
                        at += 4;
                        if length < 0 {
                            row.push(None);
                            continue;
                        }
                        let length = usize::try_from(length).expect("a value length fits");
                        row.push(Some(
                            String::from_utf8_lossy(&body[at..at + length]).into_owned(),
                        ));
                        at += length;
                    }
                    answer.rows.push(row);
                }
                b'E' => refusal = Some(read_refusal(&body)),
                // ReadyForQuery ends the exchange.
                b'Z' => break,
                // CommandComplete, EmptyQueryResponse, NoticeResponse.
                _ => {}
            }
        }
        match refusal {
            Some(refusal) => Err(refusal),
            None => Ok(answer),
        }
    }
}

/// Open a session on the cluster endpoint at `port`, as libpq does: the optional
/// `SSLRequest` first, then the startup message, then the password.
async fn login(
    case: &str,
    port: u16,
    transport: Transport,
    user: &str,
    password: &str,
    database: Option<&str>,
) -> Result<PostgresSession, Refusal> {
    let stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap_or_else(|error| {
            panic!("case {case}: the published endpoint's port {port} must accept a connection, got {error}")
        });
    let mut conn: Client = Box::new(stream);
    if transport == Transport::Tls {
        let mut request = Vec::new();
        request.extend_from_slice(&8i32.to_be_bytes());
        // The `SSLRequest` code, sent in the clear as the connection's first
        // bytes.
        request.extend_from_slice(&80_877_103i32.to_be_bytes());
        conn.write_all(&request)
            .await
            .unwrap_or_else(|error| panic!("case {case}: the request is sent, got {error}"));
        let mut answer = [0u8; 1];
        conn.read_exact(&mut answer)
            .await
            .unwrap_or_else(|error| panic!("case {case}: the server answers, got {error}"));
        assert_eq!(
            answer[0], b'S',
            "case {case}: the server must answer an SSLRequest with 'S' before any TLS"
        );
        conn = upgrade_to_tls(case, conn).await;
    }
    let mut body = Vec::new();
    // Protocol version 3.0.
    body.extend_from_slice(&196_608i32.to_be_bytes());
    put_cstring(&mut body, "user");
    put_cstring(&mut body, user);
    if let Some(database) = database {
        put_cstring(&mut body, "database");
        put_cstring(&mut body, database);
    }
    body.push(0);
    let mut startup = Vec::new();
    startup.extend_from_slice(
        &i32::try_from(body.len() + 4)
            .expect("the startup message fits")
            .to_be_bytes(),
    );
    startup.extend_from_slice(&body);
    conn.write_all(&startup)
        .await
        .unwrap_or_else(|error| panic!("case {case}: the startup is sent, got {error}"));

    let mut parameters = Vec::new();
    loop {
        let Some((kind, body)) = try_read(&mut conn).await else {
            panic!("case {case}: the server must answer the startup");
        };
        match kind {
            // AuthenticationRequest.
            b'R' => {
                let code = i32::from_be_bytes([body[0], body[1], body[2], body[3]]);
                if code == 0 {
                    continue;
                }
                assert_eq!(
                    code, 3,
                    "case {case}: this endpoint asks for the token as a cleartext password, got \
                     the authentication request {code}"
                );
                let mut message = Vec::new();
                put_cstring(&mut message, password);
                send(&mut conn, b'p', &message).await;
            }
            b'E' => return Err(read_refusal(&body)),
            // ParameterStatus.
            b'S' => {
                let (name, next) = read_cstring(&body, 0);
                let (value, _) = read_cstring(&body, next);
                parameters.push((name, value));
            }
            // ReadyForQuery: the session is open.
            b'Z' => return Ok(PostgresSession { conn, parameters }),
            // BackendKeyData, NoticeResponse.
            _ => {}
        }
    }
}

/// Run the client side of a TLS handshake on `conn`.
///
/// The verifier accepts the certificate the EMULATOR'S OWN core generated for its
/// listeners, which is self-signed and names loopback: this client is the
/// emulator's own test, connecting to a port the same process opened, so there is
/// no authority to check it against.
async fn upgrade_to_tls(case: &str, conn: Client) -> Client {
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
            panic!("case {case}: the server must complete the in-band TLS handshake, got {error}")
        });
    Box::new(stream)
}

/// See [`upgrade_to_tls`].
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

// --- framing ---------------------------------------------------------------

/// Write one message: its type byte, its length and its body.
async fn send(conn: &mut Client, kind: u8, body: &[u8]) {
    let mut message = vec![kind];
    message.extend_from_slice(
        &i32::try_from(body.len() + 4)
            .expect("the message fits")
            .to_be_bytes(),
    );
    message.extend_from_slice(body);
    conn.write_all(&message)
        .await
        .unwrap_or_else(|error| panic!("the message is sent, got {error}"));
}

/// Read one message, or `None` where the server closed the connection.
async fn try_read(conn: &mut Client) -> Option<(u8, Vec<u8>)> {
    let mut header = [0u8; 5];
    conn.read_exact(&mut header).await.ok()?;
    let length = i32::from_be_bytes([header[1], header[2], header[3], header[4]]);
    let body_length = usize::try_from(length - 4).expect("a message length fits");
    let mut body = vec![0u8; body_length];
    conn.read_exact(&mut body).await.ok()?;
    Some((header[0], body))
}

/// The fields an `ErrorResponse` carries.
fn read_refusal(body: &[u8]) -> Refusal {
    let mut refusal = Refusal::default();
    let mut at = 0usize;
    while at < body.len() && body[at] != 0 {
        let field = body[at];
        let (value, next) = read_cstring(body, at + 1);
        match field {
            b'S' => refusal.severity = value,
            b'C' => refusal.code = value,
            b'M' => refusal.message = value,
            b'H' => refusal.hint = value,
            _ => {}
        }
        at = next;
    }
    refusal
}

/// The NUL-terminated string at `at`, and the index after it.
fn read_cstring(payload: &[u8], at: usize) -> (String, usize) {
    let rest = &payload[at..];
    let end = rest.iter().position(|byte| *byte == 0).unwrap_or(rest.len());
    (
        String::from_utf8_lossy(&rest[..end]).into_owned(),
        at + end + 1,
    )
}

/// Append a NUL-terminated string.
fn put_cstring(payload: &mut Vec<u8>, text: &str) {
    payload.extend_from_slice(text.as_bytes());
    payload.push(0);
}
