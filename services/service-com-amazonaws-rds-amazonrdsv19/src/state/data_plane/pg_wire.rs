// SPDX-License-Identifier: CC0-1.0

//! The server half of the PostgreSQL frontend/backend protocol, version 3.0, for a DB
//! instance's engine endpoint.
//!
//! A stock client (libpq, psycopg, a JDBC driver) connects to the published
//! `Endpoint{Address,Port}` and speaks this. What it gets is this emulator's own state:
//! the login is admitted against the stored DB instance ([`wire_session::admit`]), the
//! statements run against the instance's SQL engine, and the session values
//! (`current_user`, `pg_backend_pid()`, `pg_stat_ssl`) are the facts of the connection it
//! is asking on.
//!
//! The order of a connection, which is the protocol's and not this emulator's:
//!
//! 1. the client may send `SSLRequest` (byte 0, in the clear). The server answers `S` and
//!    the core's TLS upgrader runs the handshake, or `N` for a request this server does
//!    not serve (`GSSENCRequest`);
//! 2. the `StartupMessage` names the user, the database and the client's parameters;
//! 3. the server asks for the password, checks it, and answers `AuthenticationOk` with
//!    the parameters a client reads its session's shape from — or one `ErrorResponse` with
//!    severity `FATAL` and the SQLSTATE of the refusal, and closes;
//! 4. the simple (`Q`) and extended (`P`/`B`/`D`/`E`/`S`) query protocols, until the
//!    client terminates or the engine's run ends under it (a reboot, a stop), which this
//!    server reports as PostgreSQL reports a restart: `57P01`, and the connection closes.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::sql_engine::{Cell, ColumnInfo, EngineError, EngineErrorKind, InstanceDatabase};
use super::tls_probe::{HandshakeRecord, HandshakeTap};
use super::wire_session::{
    self, LoginRefusal, PasswordProof, SessionContext, SessionFacts, TlsFacts,
};
use super::EngineWire;

/// The protocol version a 3.0 client sends in its startup message.
const PROTOCOL_VERSION_3: i32 = 196_608;

/// The startup codes that are requests rather than a startup message.
const SSL_REQUEST_CODE: i32 = 80_877_103;
const GSSENC_REQUEST_CODE: i32 = 80_877_104;
const CANCEL_REQUEST_CODE: i32 = 80_877_102;

/// How many bytes one protocol message may carry. The protocol allows far more; this is
/// what this endpoint serves, so a length prefix cannot make it allocate without bound.
const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

/// `superuser_reserved_connections`'s own default: "the number of connection slots
/// reserved for connections by roles with SUPERUSER", which PostgreSQL ships as 3. Those
/// slots come out of `max_connections`, so an ordinary role is refused once only they are
/// left.
const DEFAULT_SUPERUSER_RESERVED_CONNECTIONS: u64 = 3;

/// The PostgreSQL wire handler for one DB instance's listener.
pub struct PostgresWire {
    context: SessionContext,
}

impl PostgresWire {
    /// The handler for the instance `context` names.
    #[must_use]
    pub fn new(context: SessionContext) -> Self {
        Self { context }
    }
}

impl ::emulator_server::NativeHandler for PostgresWire {
    fn serve(
        &self,
        conn: ::emulator_server::ListenerConn,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let context = self.context.clone();
        Box::pin(async move {
            // An I/O error is the client's connection going away, which ends the session
            // and nothing else.
            let _ = serve_session(context, conn).await;
        })
    }
}

/// Everything this session is: who is connected and what the engine's run was when it
/// started. Dropping it ends the session's row in `pg_stat_ssl` and rolls back whatever
/// it had not committed — on every path, including a severed connection.
///
/// The session is taken out of the endpoint's registry on the dropping thread, so the next
/// login is counted against `max_connections` correctly at once; the ENGINE work — the
/// catalog row and the rollback of what this session never committed — goes to the blocking
/// pool ([`wire_session::off_the_async_worker`]), because a drop runs on an async worker and
/// `Drop` cannot await.
struct Session {
    database: Arc<InstanceDatabase>,
    sessions: super::pg_sessions::SessionRegistry,
    /// The database this session is connected to, which is the one its row and its
    /// uncommitted work are in.
    name: String,
    pid: u32,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.sessions.ended(self.pid);
        let engine = Arc::clone(&self.database);
        let name = self.name.clone();
        let pid = self.pid;
        wire_session::off_the_async_worker(move || {
            let _ = engine.execute(
                &name,
                "DELETE FROM pg_stat_ssl WHERE pid = ?",
                &[Some(pid.to_string())],
            );
            let _ = engine.rollback_transaction(pid);
        });
    }
}

/// One connection, from its first byte to its last.
async fn serve_session(
    context: SessionContext,
    conn: ::emulator_server::ListenerConn,
) -> std::io::Result<()> {
    let mut conn = conn;
    let mut tls: Option<TlsFacts> = None;

    // 1. The requests that come before a startup message.
    let startup = loop {
        let body = read_startup(&mut conn).await?;
        let code = i32::from_be_bytes([
            *body.first().unwrap_or(&0),
            *body.get(1).unwrap_or(&0),
            *body.get(2).unwrap_or(&0),
            *body.get(3).unwrap_or(&0),
        ]);
        if body.len() == 4 {
            match code {
                SSL_REQUEST_CODE if tls.is_none() => {
                    conn.write_all(b"S").await?;
                    conn.flush().await?;
                    let record = HandshakeRecord::default();
                    let tapped: ::emulator_server::ListenerConn =
                        Box::new(HandshakeTap::new(conn, record.clone()));
                    let (upgraded, _alpn) = context.upgrader.accept(tapped, None).await?;
                    conn = upgraded;
                    tls = Some(record.negotiated());
                    continue;
                }
                // This server does not serve GSSAPI encryption; `N` is the protocol's
                // "not this one", and the client carries on in the clear or with TLS.
                GSSENC_REQUEST_CODE => {
                    conn.write_all(b"N").await?;
                    conn.flush().await?;
                    continue;
                }
                // A cancel request is sent on its own connection and names a backend to
                // cancel; this server runs each statement to completion under its own
                // deadline, so there is nothing to cancel and the connection closes, as
                // the protocol says it does.
                CANCEL_REQUEST_CODE => return Ok(()),
                _ => {
                    return fatal(
                        &mut conn,
                        "0A000",
                        &format!("unsupported startup request {code}"),
                    )
                    .await
                }
            }
        }
        if code != PROTOCOL_VERSION_3 {
            let major = code >> 16;
            let minor = code & 0xffff;
            return fatal(
                &mut conn,
                "0A000",
                &format!(
                    "unsupported frontend protocol {major}.{minor}: server supports 3.0"
                ),
            )
            .await;
        }
        break startup_parameters(body.get(4..).unwrap_or_default());
    };

    // 2. The instance this listener belongs to, as the store holds it now.
    let Some(facts) = context.endpoint_facts() else {
        return fatal(
            &mut conn,
            "57P03",
            "the database system is starting up",
        )
        .await;
    };
    let user = startup
        .iter()
        .find(|(key, _)| key == "user")
        .map(|(_, value)| value.clone())
        .unwrap_or_default();
    let database = startup
        .iter()
        .find(|(key, _)| key == "database")
        .map(|(_, value)| value.clone());

    // 3. The refusals that come before any credential is read, then the password.
    if let Err(refusal) = wire_session::precheck(&facts, tls.is_some()) {
        wire_session::record_refused_login(&facts, &refusal);
        let (code, message) = refusal_sentence(&refusal, &facts, &user, database.as_deref());
        return fatal(&mut conn, code, &message).await;
    }
    // AuthenticationCleartextPassword: the password travels on the session, which the
    // instance's own parameter group is what decides may be in the clear.
    send(&mut conn, b'R', &3i32.to_be_bytes()).await?;
    conn.flush().await?;
    let password = match read_message(&mut conn).await? {
        Some((b'p', body)) => cstring(&body, 0).0,
        _ => {
            return fatal(
                &mut conn,
                "28P01",
                &format!("password authentication failed for user \"{user}\""),
            )
            .await
        }
    };
    // Which credentials admit this login: the master user's password, and — on a resource
    // with IAM database authentication turned on — an authentication token presented by a
    // database user enabled for it. What this resolves is the resource's own setting and
    // the database the user would be resolved against, both read from the facts above; the
    // engine is read only inside the decision, and only for a login whose token claims
    // already hold.
    let iam = context.iam_login(&facts, &user, database.as_deref());
    let database = match wire_session::admit(
        &facts,
        &user,
        database.as_deref(),
        tls.is_some(),
        &PasswordProof::Cleartext(&password),
        Some(&iam),
    ) {
        Ok(database) => database,
        Err(refusal) => {
            wire_session::record_refused_login(&facts, &refusal);
            let (code, message) = refusal_sentence(&refusal, &facts, &user, database.as_deref());
            return fatal(&mut conn, code, &message).await;
        }
    };

    // 4. The session is open.
    let mut session_facts = SessionFacts {
        wire: EngineWire::Postgres,
        identifier: facts.identifier.clone(),
        user: user.clone(),
        database: database.clone(),
        connection_id: context.take_connection_id(),
        tls: tls.clone(),
        server_version: facts.server_version.clone(),
        engine: facts.engine.clone(),
        requires_secure_transport: facts.requires_secure_transport,
        settings: std::collections::BTreeMap::new(),
        databases: facts.databases.clone(),
        endpoint_host: facts.endpoint_host.clone(),
        read_only: facts.read_only,
        runtime: facts.runtime.clone(),
        sessions: context.sessions.clone(),
    };
    // `max_connections` — "the maximum number of concurrent connections to the database
    // server" — is what a PostgreSQL server bounds its session table by, and it refuses the
    // connection that would exceed it before the session is opened. The value is the
    // instance's own effective parameter, so a parameter group that lowers it lowers what
    // this endpoint admits. The last `superuser_reserved_connections` slots are not
    // available to an ordinary role at all, and the master user of an RDS instance is one
    // ([`wire_session::EngineRuntime`] reports it as no superuser), so it is refused for
    // the reserved slots before the table is full.
    if let Some(limit) = session_facts.runtime.numeric_parameter("max_connections") {
        let open = context.sessions.open_count() as u64;
        let reserved = session_facts
            .runtime
            .numeric_parameter("superuser_reserved_connections")
            .unwrap_or(DEFAULT_SUPERUSER_RESERVED_CONNECTIONS);
        if open >= limit {
            return fatal(&mut conn, "53300", "sorry, too many clients already").await;
        }
        if open + reserved >= limit {
            return fatal(
                &mut conn,
                "53300",
                "remaining connection slots are reserved for non-replication superuser \
                 connections",
            )
            .await;
        }
    }
    let database_handle = Arc::clone(&context.database);
    let _session = Session {
        database: Arc::clone(&database_handle),
        sessions: context.sessions.clone(),
        name: session_facts.database.clone(),
        pid: session_facts.connection_id,
    };
    context.sessions.admitted(super::pg_sessions::LiveSession {
        pid: session_facts.connection_id,
        user: user.clone(),
        database: database.clone(),
        application_name: startup
            .iter()
            .find(|(key, _)| key == "application_name")
            .map(|(_, value)| value.clone())
            .unwrap_or_default(),
        client_address: facts.endpoint_host.clone(),
        backend_start: super::pg_server::timestamptz_text(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64(),
        ),
        state: super::pg_sessions::STATE_IDLE.to_string(),
        query: String::new(),
        query_start: None,
    });
    register_session(&database_handle, &session_facts).await;

    send(&mut conn, b'R', &0i32.to_be_bytes()).await?;
    for (name, value) in [
        ("server_version", session_facts.server_version.clone()),
        ("server_encoding", "UTF8".to_string()),
        ("client_encoding", "UTF8".to_string()),
        ("DateStyle", "ISO, MDY".to_string()),
        ("TimeZone", "UTC".to_string()),
        ("integer_datetimes", "on".to_string()),
        ("standard_conforming_strings", "on".to_string()),
        ("is_superuser", "off".to_string()),
        ("session_authorization", user.clone()),
    ] {
        let mut body = Vec::new();
        put_cstring(&mut body, name);
        put_cstring(&mut body, &value);
        send(&mut conn, b'S', &body).await?;
    }
    let mut key_data = Vec::new();
    let session_pid = i32::try_from(session_facts.connection_id).unwrap_or(i32::MAX);
    key_data.extend_from_slice(&session_pid.to_be_bytes());
    // The cancel key: a session's own id, which is all this server has to identify it
    // by, since it cancels nothing (see the cancel request above).
    key_data.extend_from_slice(&session_pid.to_be_bytes());
    send(&mut conn, b'K', &key_data).await?;
    ready(&mut conn, &database_handle, session_facts.connection_id, false).await?;

    // 5. The query protocols.
    let mut generation = context.generation.clone();
    // The engine's run as it is NOW is what this session serves under: marking the
    // current value seen is what makes the wait below fire on the NEXT change (the
    // reboot or stop that ends this session) rather than on one that happened before it
    // opened.
    let _ = generation.borrow_and_update();
    let mut prepared: std::collections::BTreeMap<String, PreparedStatement> =
        std::collections::BTreeMap::new();
    let mut portals: std::collections::BTreeMap<String, Portal> =
        std::collections::BTreeMap::new();
    let mut failed_in_transaction = false;
    loop {
        let message = tokio::select! {
            // The engine's run ended under this session: a reboot or a stop severs it,
            // which PostgreSQL reports exactly this way before closing the socket.
            _ = generation.changed() => {
                let _ = fatal(
                    &mut conn,
                    "57P01",
                    "terminating connection due to administrator command",
                )
                .await;
                return Ok(());
            }
            message = read_message(&mut conn) => message?,
        };
        let Some((kind, body)) = message else {
            return Ok(());
        };
        match kind {
            // Terminate.
            b'X' => return Ok(()),
            // Simple query.
            b'Q' => {
                let (sql, _) = cstring(&body, 0);
                failed_in_transaction = run_simple_query(
                    &mut conn,
                    &context,
                    &database_handle,
                    &mut session_facts,
                    &sql,
                )
                .await?;
                ready(
                    &mut conn,
                    &database_handle,
                    session_facts.connection_id,
                    failed_in_transaction,
                )
                .await?;
            }
            // Parse.
            b'P' => {
                let (name, at) = cstring(&body, 0);
                let (sql, _) = cstring(&body, at);
                if prepared.len() >= MAX_PREPARED_STATEMENTS {
                    error_response(
                        &mut conn,
                        "ERROR",
                        "53400",
                        &format!(
                            "this endpoint holds at most {MAX_PREPARED_STATEMENTS} prepared statements per session"
                        ),
                    )
                    .await?;
                    continue;
                }
                prepared.insert(name, PreparedStatement { sql });
                send(&mut conn, b'1', &[]).await?;
            }
            // Bind.
            b'B' => {
                let (portal_name, at) = cstring(&body, 0);
                let (statement_name, at) = cstring(&body, at);
                match bind_portal(&body, at, prepared.get(&statement_name)) {
                    Ok(portal) => {
                        if portals.len() >= MAX_PREPARED_STATEMENTS {
                            error_response(
                                &mut conn,
                                "ERROR",
                                "53400",
                                &format!(
                                    "this endpoint holds at most {MAX_PREPARED_STATEMENTS} portals per session"
                                ),
                            )
                            .await?;
                            continue;
                        }
                        portals.insert(portal_name, portal);
                        send(&mut conn, b'2', &[]).await?;
                    }
                    Err(error) => {
                        failed_in_transaction = true;
                        engine_error_response(&mut conn, &error).await?;
                    }
                }
            }
            // Describe.
            b'D' => {
                let what = *body.first().unwrap_or(&b'S');
                let (name, _) = cstring(&body, 1);
                if what == b'S' {
                    // The parameter types of a statement this server did not plan: it
                    // reports none, which tells the client to send them as text.
                    let mut body = Vec::new();
                    body.extend_from_slice(&0i16.to_be_bytes());
                    send(&mut conn, b't', &body).await?;
                }
                let sql = match what {
                    b'P' => portals
                        .get(&name)
                        .map(|portal| (portal.sql.clone(), portal.params.clone())),
                    _ => prepared
                        .get(&name)
                        .map(|statement| (statement.sql.clone(), Vec::new())),
                };
                match sql {
                    // The columns the statement would produce, as the ENGINE reports
                    // them for it; a statement it cannot plan (a server command) is
                    // described as returning none, and what it does return is described
                    // when it runs.
                    Some((sql, params)) => {
                        let columns = describe_columns(
                            &database_handle,
                            &session_facts,
                            sql,
                            params,
                        )
                        .await;
                        if columns.is_empty() {
                            send(&mut conn, b'n', &[]).await?;
                        } else {
                            row_description(&mut conn, &columns, &[]).await?;
                            // The row description belongs to one query cycle: having
                            // answered it here, the execute below does not repeat it.
                            if let Some(portal) = portals.get_mut(&name) {
                                portal.described = true;
                            }
                        }
                    }
                    None => {
                        error_response(
                            &mut conn,
                            "ERROR",
                            "34000",
                            &format!("portal \"{name}\" does not exist"),
                        )
                        .await?;
                    }
                }
            }
            // Execute.
            b'E' => {
                let (name, _) = cstring(&body, 0);
                match portals.get(&name) {
                    Some(portal) => {
                        let described = portal.described;
                        let statement = portal.sql.clone();
                        // A `COPY … FROM STDIN` a client parsed and executed through the
                        // extended protocol enters copy-in mode here, exactly as it does
                        // in a simple query.
                        if let Some(target) = copy_target(&session_facts, &statement) {
                            if !serve_copy_in(
                                &mut conn,
                                &context,
                                &database_handle,
                                &session_facts,
                                &target,
                            )
                            .await?
                            {
                                failed_in_transaction = true;
                            }
                            continue;
                        }
                        let outcome = run_statement(
                            &context,
                            &database_handle,
                            &mut session_facts,
                            statement,
                            portal.params.clone(),
                        )
                        .await;
                        match outcome {
                            Ok(answer) => {
                                write_answer(&mut conn, &answer, !described).await?;
                            }
                            Err(error) => {
                                failed_in_transaction = true;
                                engine_error_response(&mut conn, &error).await?;
                            }
                        }
                    }
                    None => {
                        failed_in_transaction = true;
                        error_response(
                            &mut conn,
                            "ERROR",
                            "34000",
                            &format!("portal \"{name}\" does not exist"),
                        )
                        .await?;
                    }
                }
            }
            // Close.
            b'C' => {
                let what = *body.first().unwrap_or(&b'S');
                let (name, _) = cstring(&body, 1);
                if what == b'P' {
                    portals.remove(&name);
                } else {
                    prepared.remove(&name);
                }
                send(&mut conn, b'3', &[]).await?;
            }
            // Sync.
            b'S' => {
                if !database_handle.in_transaction(session_facts.connection_id) {
                    failed_in_transaction = false;
                }
                ready(
                    &mut conn,
                    &database_handle,
                    session_facts.connection_id,
                    failed_in_transaction,
                )
                .await?;
            }
            // Flush: everything this server writes is already flushed.
            b'H' => {
                conn.flush().await?;
            }
            other => {
                return fatal(
                    &mut conn,
                    "08P01",
                    &format!("unsupported frontend message type '{}'", other as char),
                )
                .await
            }
        }
    }
}

/// How many prepared statements and portals one session may hold at once, so a client
/// cannot grow this server's per-session state without bound.
const MAX_PREPARED_STATEMENTS: usize = 64;

/// A statement the client parsed under a name.
struct PreparedStatement {
    sql: String,
}

/// A statement bound to its parameter values, ready to execute.
struct Portal {
    sql: String,
    params: Vec<Option<String>>,
    /// Whether a `Describe` has already answered this portal's row description, which
    /// belongs to the query cycle once and not twice.
    described: bool,
}

/// Read the `Bind` message's parameter values for the statement it names.
fn bind_portal(
    body: &[u8],
    at: usize,
    statement: Option<&PreparedStatement>,
) -> Result<Portal, EngineError> {
    let Some(statement) = statement else {
        return Err(EngineError::new(
            EngineErrorKind::UndefinedObject,
            "prepared statement does not exist".to_string(),
        ));
    };
    let mut at = at;
    let format_count = usize::from(read_i16(body, at)?.max(0).unsigned_abs());
    at += 2;
    // Not sized from the count the message declares: each format is pushed as it is read,
    // so a count a client overstates cannot make this allocate for it.
    let mut formats = Vec::new();
    for _ in 0..format_count {
        formats.push(read_i16(body, at)?);
        at += 2;
    }
    if formats.iter().any(|format| *format != 0) {
        return Err(EngineError::new(
            EngineErrorKind::InsufficientPrivilege,
            "this endpoint reads parameters in the text format only".to_string(),
        ));
    }
    let value_count = usize::from(read_i16(body, at)?.max(0).unsigned_abs());
    at += 2;
    // Not sized from the count the message declares: the values are pushed as they are
    // read, so a count a client overstates cannot make this allocate for it.
    let mut params = Vec::new();
    for _ in 0..value_count {
        let length = read_i32(body, at)?;
        at += 4;
        if length < 0 {
            params.push(None);
            continue;
        }
        let length = usize::try_from(length).unwrap_or_default();
        let bytes = body.get(at..at + length).ok_or_else(truncated)?;
        at += length;
        params.push(Some(String::from_utf8_lossy(bytes).into_owned()));
    }
    let result_formats = usize::from(read_i16(body, at)?.max(0).unsigned_abs());
    at += 2;
    for _ in 0..result_formats {
        if read_i16(body, at)? != 0 {
            return Err(EngineError::new(
                EngineErrorKind::InsufficientPrivilege,
                "this endpoint returns results in the text format only".to_string(),
            ));
        }
        at += 2;
    }
    Ok(Portal {
        sql: statement.sql.clone(),
        params,
        described: false,
    })
}

/// A message that ended before it said everything it declared.
fn truncated() -> EngineError {
    EngineError::new(
        EngineErrorKind::Syntax,
        "the message ended before its declared length".to_string(),
    )
}

fn read_i16(body: &[u8], at: usize) -> Result<i16, EngineError> {
    let bytes = body.get(at..at + 2).ok_or_else(truncated)?;
    Ok(i16::from_be_bytes([
        *bytes.first().ok_or_else(truncated)?,
        *bytes.get(1).ok_or_else(truncated)?,
    ]))
}

fn read_i32(body: &[u8], at: usize) -> Result<i32, EngineError> {
    let bytes = body.get(at..at + 4).ok_or_else(truncated)?;
    Ok(i32::from_be_bytes([
        *bytes.first().ok_or_else(truncated)?,
        *bytes.get(1).ok_or_else(truncated)?,
        *bytes.get(2).ok_or_else(truncated)?,
        *bytes.get(3).ok_or_else(truncated)?,
    ]))
}

/// Run every statement a simple query carries, answering for each in turn. Returns
/// whether a statement failed, which is what the following `ReadyForQuery` reports when
/// the session is inside a transaction.
async fn run_simple_query(
    conn: &mut ::emulator_server::ListenerConn,
    context: &wire_session::SessionContext,
    database: &Arc<InstanceDatabase>,
    facts: &mut SessionFacts,
    sql: &str,
) -> std::io::Result<bool> {
    let statements = match wire_session::split_statements(facts.wire, sql) {
        Ok(statements) => statements,
        Err(error) => {
            engine_error_response(conn, &error).await?;
            return Ok(true);
        }
    };
    if statements.is_empty() {
        send(conn, b'I', &[]).await?;
        return Ok(false);
    }
    for statement in statements {
        // `COPY … FROM STDIN` is not a statement the engine runs on its own: it puts this
        // session into the protocol's copy-in mode, where the client streams the rows.
        if let Some(target) = copy_target(facts, &statement) {
            if !serve_copy_in(conn, context, database, facts, &target).await? {
                return Ok(true);
            }
            continue;
        }
        match run_statement(context, database, facts, statement, Vec::new()).await {
            Ok(answer) => write_answer(conn, &answer, true).await?,
            Err(error) => {
                engine_error_response(conn, &error).await?;
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// The copy-in target `sql` names, for the statements that are one.
///
/// A session that serves no writes enters copy-in mode for nothing: the statement is handed
/// to the engine instead, which refuses it as the write it is
/// (`super::read_only::refused_command`), so the client is told why rather than being asked
/// for rows nothing will load.
fn copy_target(facts: &SessionFacts, sql: &str) -> Option<super::pg_copy::CopyTarget> {
    if facts.read_only {
        return None;
    }
    let tokens = wire_session::significant_tokens(facts.wire, sql).ok()?;
    super::pg_copy::copy_in_target(facts, &tokens)
}

/// Serve one `COPY <table> FROM STDIN`: ask the client for the data, read what it streams,
/// load it, and answer with what was loaded — or with the error the load stopped on.
///
/// Returns whether the copy succeeded. A copy that failed leaves the session the way any
/// failed statement does: the error is reported and the caller answers `ReadyForQuery`.
///
/// The rows it loaded are a change of the instance's data like any other, so a copy that
/// committed is carried over to this instance's read replicas
/// ([`super::replication`]) exactly as a statement's write is.
async fn serve_copy_in(
    conn: &mut ::emulator_server::ListenerConn,
    context: &wire_session::SessionContext,
    database: &Arc<InstanceDatabase>,
    facts: &SessionFacts,
    target: &super::pg_copy::CopyTarget,
) -> std::io::Result<bool> {
    // CopyInResponse: the text format (0), and no per-column format codes, which is what
    // the text format carries.
    let mut response = vec![0u8];
    response.extend_from_slice(&0i16.to_be_bytes());
    send(conn, b'G', &response).await?;
    conn.flush().await?;

    let mut data: Vec<u8> = Vec::new();
    let mut failed: Option<EngineError> = None;
    loop {
        let Some((kind, body)) = read_message(conn).await? else {
            // The client went away mid-copy: nothing is loaded.
            return Ok(false);
        };
        match kind {
            // CopyData.
            b'd' => {
                if data.len() + body.len() > super::pg_copy::MAX_COPY_BYTES {
                    failed = Some(EngineError::new(
                        EngineErrorKind::LimitExceeded,
                        format!(
                            "the copy data is larger than this endpoint serves ({} bytes)",
                            super::pg_copy::MAX_COPY_BYTES
                        ),
                    ));
                    data.clear();
                    continue;
                }
                data.extend_from_slice(&body);
            }
            // CopyDone.
            b'c' => break,
            // CopyFail: the client gave up on its own load, and says why.
            b'f' => {
                let (reason, _) = cstring(&body, 0);
                engine_error_response(
                    conn,
                    &EngineError::new(
                        EngineErrorKind::Cancelled,
                        format!("COPY from stdin failed: {reason}"),
                    ),
                )
                .await?;
                return Ok(false);
            }
            // The protocol allows nothing else between CopyInResponse and CopyDone.
            _ => {
                let _ = fatal(conn, "08P01", "unexpected message during COPY from stdin").await;
                return Ok(false);
            }
        }
    }
    if let Some(error) = failed {
        engine_error_response(conn, &error).await?;
        return Ok(false);
    }
    let loaded = {
        let database = Arc::clone(database);
        let facts = facts.clone();
        let target = target.clone();
        tokio::task::spawn_blocking(move || super::pg_copy::load(&database, &facts, &target, &data))
            .await
            .map_err(|error| std::io::Error::other(error.to_string()))?
    };
    match loaded {
        Ok(rows) => {
            if !database.in_transaction(facts.connection_id) {
                super::replication::apply_to_read_replicas(
                    &context.state,
                    &context.target,
                    database,
                )
                .await;
            }
            let mut tag = Vec::new();
            put_cstring(&mut tag, &format!("COPY {rows}"));
            send(conn, b'C', &tag).await?;
            Ok(true)
        }
        Err(error) => {
            engine_error_response(conn, &error).await?;
            Ok(false)
        }
    }
}

/// Run one statement on a blocking task — the engine's work is CPU-bound and must not run
/// on the async worker — and keep any setting it changed on the session.
///
/// The instance's engine runtime is re-read first, so the statement sees the parameters the
/// instance has NOW: a parameter group modified under a live session is visible to it from
/// the next statement on, which is what "applied immediately" means. The session's own row
/// in `pg_stat_activity` is marked with the statement for as long as it runs.
///
/// A statement that COMMITTED a change is then carried over to the read replicas of this
/// instance ([`super::replication`]): that is the replication a replica is kept up to date
/// by, and it happens after the client's answer is computed, as the engines' own
/// asynchronous replication does.
async fn run_statement(
    context: &wire_session::SessionContext,
    database: &Arc<InstanceDatabase>,
    facts: &mut SessionFacts,
    sql: String,
    params: Vec<Option<String>>,
) -> Result<wire_session::StatementAnswer, EngineError> {
    if let Some((runtime, read_only)) = context.engine_now() {
        facts.runtime = runtime;
        facts.read_only = read_only;
    }
    context.sessions.running(
        facts.connection_id,
        &sql,
        super::pg_server::timestamptz_text(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64(),
        ),
    );
    let engine = Arc::clone(database);
    let session = facts.clone();
    let statement = sql.clone();
    let answer = tokio::task::spawn_blocking(move || {
        wire_session::run_statement(&engine, &session, &statement, &params)
    })
    .await
    .map_err(|error| EngineError::new(EngineErrorKind::Internal, error.to_string()));
    context.sessions.idle(facts.connection_id);
    let answer = answer??;
    if let Some((name, value)) = answer.setting_changed.clone() {
        facts.settings.insert(name, value);
    }
    if let Some(database) = answer.database_changed.clone() {
        facts.database = database;
    }
    // A statement that created or dropped a database changed what this session may name.
    if answer.databases_changed {
        facts.databases = database.database_names();
    }
    // What the source has COMMITTED is what its read replicas hold: carried over once the
    // statement is through and the session has no transaction still open, so a write still
    // inside a transaction reaches no replica until that transaction commits.
    if super::replication::publishes_changes(&answer.verb)
        && !database.in_transaction(facts.connection_id)
    {
        super::replication::apply_to_read_replicas(&context.state, &context.target, database)
            .await;
    }
    Ok(answer)
}

/// Write a statement's rows and its `CommandComplete`.
///
/// `describe_rows` is false where a `Describe` has already answered this portal's row
/// description, which the extended query protocol asks for once per cycle.
async fn write_answer(
    conn: &mut ::emulator_server::ListenerConn,
    answer: &wire_session::StatementAnswer,
    describe_rows: bool,
) -> std::io::Result<()> {
    // What the server has to say about the statement comes before its answer, which is
    // the order a client reads them in.
    for notice in &answer.notices {
        notice_response(conn, notice).await?;
    }
    if answer.verb.is_empty() {
        send(conn, b'I', &[]).await?;
        return Ok(());
    }
    if !answer.result.columns.is_empty() {
        let empty: Vec<Cell> = Vec::new();
        let sample = answer.result.rows.first().unwrap_or(&empty);
        if describe_rows {
            row_description(conn, &answer.result.columns, sample).await?;
        }
        for row in &answer.result.rows {
            let mut body = Vec::new();
            let count = i16::try_from(row.len()).unwrap_or(i16::MAX);
            body.extend_from_slice(&count.to_be_bytes());
            for (index, cell) in row.iter().enumerate() {
                match render_cell(answer.result.columns.get(index), cell) {
                    None => body.extend_from_slice(&(-1i32).to_be_bytes()),
                    Some(text) => {
                        let length = i32::try_from(text.len()).unwrap_or(i32::MAX);
                        body.extend_from_slice(&length.to_be_bytes());
                        body.extend_from_slice(text.as_bytes());
                    }
                }
            }
            send(conn, b'D', &body).await?;
        }
    }
    let mut tag = Vec::new();
    put_cstring(&mut tag, &command_tag(answer));
    send(conn, b'C', &tag).await?;
    conn.flush().await
}

/// One `RowDescription`: the columns a statement produces, each under the type a client
/// decodes it with.
async fn row_description(
    conn: &mut ::emulator_server::ListenerConn,
    columns: &[ColumnInfo],
    sample: &[Cell],
) -> std::io::Result<()> {
    let mut body = Vec::new();
    let count = i16::try_from(columns.len()).unwrap_or(i16::MAX);
    body.extend_from_slice(&count.to_be_bytes());
    for (index, column) in columns.iter().enumerate() {
        put_cstring(&mut body, &column.name);
        // No table and no column number: the value is this server's, and the client
        // reads the column by name and position.
        body.extend_from_slice(&0i32.to_be_bytes());
        body.extend_from_slice(&0i16.to_be_bytes());
        body.extend_from_slice(&type_oid(column, sample.get(index)).to_be_bytes());
        body.extend_from_slice(&(-1i16).to_be_bytes());
        body.extend_from_slice(&(-1i32).to_be_bytes());
        body.extend_from_slice(&0i16.to_be_bytes());
    }
    send(conn, b'T', &body).await
}

/// The columns a statement would produce, asked of the engine on a blocking task.
async fn describe_columns(
    database: &Arc<InstanceDatabase>,
    facts: &SessionFacts,
    sql: String,
    params: Vec<Option<String>>,
) -> Vec<ColumnInfo> {
    let database = Arc::clone(database);
    let facts = facts.clone();
    tokio::task::spawn_blocking(move || {
        wire_session::describe_statement(&database, &facts, &sql, &params)
    })
    .await
    .unwrap_or_default()
}

/// The `CommandComplete` tag: the statement's verb and the count PostgreSQL reports with
/// it ("the number of rows retrieved" for a SELECT, the number changed for a write, and
/// an `oid` field of 0 for an INSERT).
fn command_tag(answer: &wire_session::StatementAnswer) -> String {
    match answer.verb.as_str() {
        "SELECT" | "SHOW" | "FETCH" => format!("SELECT {}", answer.result.rows.len()),
        "INSERT" => format!("INSERT 0 {}", answer.result.affected),
        "UPDATE" | "DELETE" | "MERGE" => {
            format!("{} {}", answer.verb, answer.result.affected)
        }
        other => other.to_string(),
    }
}

/// The PostgreSQL type OID a column is reported under: the type its DECLARATION names,
/// which is what a client decodes the value with, and the type of the value the engine
/// holds where the column has no declaration (a computed column).
///
/// Source: the type names PostgreSQL's "Data Types" chapter documents (with the aliases it
/// lists for each) and the OIDs of those types in `pg_type`, which is what the wire
/// carries.
fn type_oid(column: &ColumnInfo, sample: Option<&Cell>) -> i32 {
    if let Some(declared) = &column.declared_type {
        let lowered = declared.to_ascii_lowercase();
        let base = lowered
            .split('(')
            .next()
            .unwrap_or(&lowered)
            .trim()
            .to_string();
        let array = base.ends_with("[]");
        let base = base.trim_end_matches("[]").trim().to_string();
        let oid = match base.as_str() {
            "bool" | "boolean" => Some((16, 1000)),
            "bytea" | "blob" => Some((17, 1001)),
            "char" | "character" | "bpchar" => Some((1042, 1014)),
            "name" => Some((19, 1003)),
            "int8" | "bigint" | "bigserial" => Some((20, 1016)),
            "int2" | "smallint" | "smallserial" => Some((21, 1005)),
            "int4" | "int" | "integer" | "serial" => Some((23, 1007)),
            "text" | "citext" => Some((25, 1009)),
            "json" => Some((114, 199)),
            "float4" | "real" => Some((700, 1021)),
            "float8" | "double" | "double precision" => Some((701, 1022)),
            "varchar" | "character varying" => Some((1043, 1015)),
            "date" => Some((1082, 1182)),
            "time" | "time without time zone" => Some((1083, 1183)),
            "timestamp" | "timestamp without time zone" => Some((1114, 1115)),
            "timestamptz" | "timestamp with time zone" => Some((1184, 1185)),
            "numeric" | "decimal" => Some((1700, 1231)),
            "uuid" => Some((2950, 2951)),
            "jsonb" => Some((3802, 3807)),
            _ => None,
        };
        if let Some((scalar, array_oid)) = oid {
            return if array { array_oid } else { scalar };
        }
    }
    match sample {
        Some(Cell::Integer(_)) => 20,
        Some(Cell::Real(_)) => 701,
        Some(Cell::Blob(_)) => 17,
        _ => 25,
    }
}

/// One value in the protocol's text format, or `None` for SQL NULL.
///
/// A column declared boolean reports PostgreSQL's own `t`/`f`, and a byte string reports
/// the `\x` hex form PostgreSQL's text output for `bytea` uses; everything else is the
/// value as the engine holds it.
fn render_cell(column: Option<&ColumnInfo>, cell: &Cell) -> Option<String> {
    let boolean = column.and_then(|column| column.declared_type.as_ref()).is_some_and(
        |declared| {
            let lowered = declared.to_ascii_lowercase();
            lowered == "bool" || lowered == "boolean"
        },
    );
    match cell {
        Cell::Null => None,
        Cell::Integer(value) if boolean => {
            Some(if *value == 0 { "f" } else { "t" }.to_string())
        }
        Cell::Integer(value) => Some(value.to_string()),
        Cell::Real(value) => Some(value.to_string()),
        Cell::Text(value) if boolean => Some(
            match value.to_ascii_lowercase().as_str() {
                "0" | "f" | "false" | "n" | "no" | "off" => "f",
                _ => "t",
            }
            .to_string(),
        ),
        Cell::Text(value) => Some(value.clone()),
        Cell::Blob(bytes) => Some(format!("\\x{}", ::hex::encode(bytes))),
    }
}

/// Record this session in `pg_stat_ssl`, the view a client reads its own connection's TLS
/// from, and open the database's extension catalog.
///
/// The `pg_stat_ssl` row is the SESSION's own fact: whether its socket is encrypted and,
/// where the handshake could be observed, what it negotiated. The catalog is the
/// DATABASE's, and a client may read it before installing anything
/// ([`super::server_statements::open_extension_catalog`]).
async fn register_session(database: &Arc<InstanceDatabase>, facts: &SessionFacts) {
    let database = Arc::clone(database);
    let session = facts.clone();
    let name = facts.database.clone();
    let pid = facts.connection_id.to_string();
    let encrypted = i64::from(facts.encrypted());
    let version = facts
        .tls
        .as_ref()
        .and_then(|tls| tls.protocol.clone());
    let cipher = facts.tls.as_ref().and_then(|tls| tls.cipher.clone());
    let _ = tokio::task::spawn_blocking(move || {
        let _ = super::server_statements::open_extension_catalog(&database, &session);
        let _ = database.execute(&name, PG_STAT_SSL_SCHEMA, &[]);
        let _ = super::pg_server::ensure_catalogs(&database, &name);
        let _ = database.execute(
            &name,
            "INSERT OR REPLACE INTO pg_stat_ssl (pid, ssl, version, cipher) \
             VALUES (?, ?, ?, ?)",
            &[
                Some(pid),
                Some(encrypted.to_string()),
                version,
                cipher,
            ],
        );
    })
    .await;
}

/// `pg_stat_ssl`, as this server keeps it: one row per live session.
///
/// `version` and `cipher` are the handshake's own, read from it by
/// [`super::tls_probe`]; a handshake whose `ServerHello` could not be read leaves them
/// NULL rather than naming a version nobody observed.
const PG_STAT_SSL_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS pg_stat_ssl (\
     pid integer PRIMARY KEY, ssl boolean NOT NULL, version text, cipher text)";

/// The SQLSTATE and the sentence PostgreSQL refuses a login with.
///
/// Each one is the server's own wording for that refusal, with the values — the user, the
/// database, the host — taken from the request and from this instance's state.
fn refusal_sentence(
    refusal: &LoginRefusal,
    facts: &wire_session::InstanceFacts,
    user: &str,
    database: Option<&str>,
) -> (&'static str, String) {
    match refusal {
        // The statuses in which there is no engine to connect to. PostgreSQL has one
        // sentence for coming up and one for going down, and `57P03` for both.
        LoginRefusal::NotRunning { status } => (
            "57P03",
            match status.as_str() {
                "creating" | "starting" | "rebooting" | "modifying" | "backing-up" => {
                    "the database system is starting up".to_string()
                }
                _ => "the database system is shutting down".to_string(),
            },
        ),
        // What PostgreSQL answers a connection that no `pg_hba.conf` line admits, which is
        // what `rds.force_ssl` adds for a session with no encryption.
        LoginRefusal::PlaintextRefused => (
            "28000",
            format!(
                "no pg_hba.conf entry for host \"{}\", user \"{}\", database \"{}\", no encryption",
                facts.endpoint_host,
                user,
                database.unwrap_or(user)
            ),
        ),
        LoginRefusal::UnknownDatabase { database } => (
            "3D000",
            format!("database \"{database}\" does not exist"),
        ),
        LoginRefusal::AuthenticationFailed { user, .. } => (
            "28P01",
            format!("password authentication failed for user \"{user}\""),
        ),
    }
}

/// The SQLSTATE a failed statement is reported under.
///
/// Source: the class and condition names in PostgreSQL's "Appendix A. PostgreSQL Error
/// Codes", which is what a client branches on.
fn sqlstate(kind: EngineErrorKind) -> &'static str {
    match kind {
        EngineErrorKind::Syntax => "42601",
        EngineErrorKind::UndefinedObject => "42P01",
        EngineErrorKind::UndefinedDatabase => "3D000",
        EngineErrorKind::UniqueViolation => "23505",
        EngineErrorKind::NotNullViolation => "23502",
        EngineErrorKind::CheckViolation => "23514",
        EngineErrorKind::ForeignKeyViolation => "23503",
        // `lock_not_available` and `deadlock_detected`: a row another transaction holds,
        // which a `NOWAIT` statement is refused for and which a cycle of waiting
        // transactions is broken with.
        EngineErrorKind::LockNotAvailable => "55P03",
        EngineErrorKind::Deadlock => "40P01",
        // `numeric_value_out_of_range` and `division_by_zero`: the two conditions the exact
        // decimal arithmetic reports (`super::pg_numeric`).
        EngineErrorKind::NumericValueOutOfRange => "22003",
        EngineErrorKind::DivisionByZero => "22012",
        // `serialization_failure`: a SERIALIZABLE transaction whose reads and another
        // transaction's writes cannot be ordered.
        EngineErrorKind::SerializationFailure => "40001",
        EngineErrorKind::DataError => "22000",
        EngineErrorKind::Cancelled => "57014",
        // `program_limit_exceeded`: the statement asked for more than this endpoint
        // serves.
        EngineErrorKind::LimitExceeded => "54000",
        EngineErrorKind::InsufficientPrivilege => "42501",
        // `read_only_sql_transaction`: the statement would write on a session that serves
        // no writes.
        EngineErrorKind::ReadOnlySqlTransaction => super::read_only::POSTGRES_READ_ONLY_SQLSTATE,
        // `duplicate_object` and `object_in_use`: the general class of each, which a
        // statement whose own condition names a narrower one overrides
        // ([`EngineError::with_sqlstate`]).
        EngineErrorKind::DuplicateObject => "42710",
        EngineErrorKind::ObjectInUse => "55006",
        EngineErrorKind::FeatureNotSupported => "0A000",
        EngineErrorKind::Internal => "58000",
    }
}

/// Report a failed statement, keeping the engine's own message.
async fn engine_error_response(
    conn: &mut ::emulator_server::ListenerConn,
    error: &EngineError,
) -> std::io::Result<()> {
    let code = error.sqlstate.unwrap_or_else(|| sqlstate(error.kind));
    error_response(conn, "ERROR", code, &error.message).await
}

/// One `NoticeResponse`: what the server has to say about a statement it did not refuse.
async fn notice_response(
    conn: &mut ::emulator_server::ListenerConn,
    notice: &wire_session::WireNotice,
) -> std::io::Result<()> {
    message_response(conn, b'N', notice.severity, notice.code, &notice.message).await
}

/// One `ErrorResponse`.
async fn error_response(
    conn: &mut ::emulator_server::ListenerConn,
    severity: &str,
    code: &str,
    message: &str,
) -> std::io::Result<()> {
    message_response(conn, b'E', severity, code, message).await
}

/// One `ErrorResponse` or `NoticeResponse`, which carry the same fields.
async fn message_response(
    conn: &mut ::emulator_server::ListenerConn,
    kind: u8,
    severity: &str,
    code: &str,
    message: &str,
) -> std::io::Result<()> {
    let mut body = Vec::new();
    body.push(b'S');
    put_cstring(&mut body, severity);
    body.push(b'V');
    put_cstring(&mut body, severity);
    body.push(b'C');
    put_cstring(&mut body, code);
    body.push(b'M');
    put_cstring(&mut body, message);
    body.push(0);
    send(conn, kind, &body).await?;
    conn.flush().await
}

/// Report a refusal that ends the connection, which is what a `FATAL` is.
async fn fatal(
    conn: &mut ::emulator_server::ListenerConn,
    code: &str,
    message: &str,
) -> std::io::Result<()> {
    error_response(conn, "FATAL", code, message).await
}

/// `ReadyForQuery` with the transaction status THIS SESSION is in.
async fn ready(
    conn: &mut ::emulator_server::ListenerConn,
    database: &Arc<InstanceDatabase>,
    session: u32,
    failed: bool,
) -> std::io::Result<()> {
    let status = match (database.in_transaction(session), failed) {
        (true, true) => b'E',
        (true, false) => b'T',
        (false, _) => b'I',
    };
    send(conn, b'Z', &[status]).await?;
    conn.flush().await
}

/// Write one message: its type byte, its length (counting the length itself) and its body.
async fn send(
    conn: &mut ::emulator_server::ListenerConn,
    kind: u8,
    body: &[u8],
) -> std::io::Result<()> {
    let length = i32::try_from(body.len() + 4).unwrap_or(i32::MAX);
    conn.write_all(&[kind]).await?;
    conn.write_all(&length.to_be_bytes()).await?;
    conn.write_all(body).await
}

/// Read the next message: its type byte and its body, or `None` at end of stream.
async fn read_message(
    conn: &mut ::emulator_server::ListenerConn,
) -> std::io::Result<Option<(u8, Vec<u8>)>> {
    let mut kind = [0u8; 1];
    match conn.read_exact(&mut kind).await {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let mut length = [0u8; 4];
    conn.read_exact(&mut length).await?;
    let length = i32::from_be_bytes(length);
    let body_length = length
        .checked_sub(4)
        .and_then(|body| usize::try_from(body).ok())
        .ok_or_else(protocol_violation)?;
    if body_length > MAX_MESSAGE_BYTES {
        return Err(too_large());
    }
    let mut body = vec![0u8; body_length];
    conn.read_exact(&mut body).await?;
    Ok(Some((kind[0], body)))
}

/// Read a startup message or request: its length and the body that follows it. The
/// startup messages are the only ones with no type byte.
async fn read_startup(conn: &mut ::emulator_server::ListenerConn) -> std::io::Result<Vec<u8>> {
    let mut length = [0u8; 4];
    conn.read_exact(&mut length).await?;
    let length = i32::from_be_bytes(length);
    let body_length = length
        .checked_sub(4)
        .and_then(|body| usize::try_from(body).ok())
        .ok_or_else(protocol_violation)?;
    if body_length > MAX_MESSAGE_BYTES {
        return Err(too_large());
    }
    let mut body = vec![0u8; body_length];
    conn.read_exact(&mut body).await?;
    Ok(body)
}

fn protocol_violation() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "the message declared a length the protocol does not allow",
    )
}

fn too_large() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "the message is larger than this endpoint serves",
    )
}

/// The key/value pairs a startup message carries, each NUL-terminated and the list ended
/// by an empty key.
fn startup_parameters(body: &[u8]) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut at = 0usize;
    while at < body.len() {
        let (key, next) = cstring(body, at);
        if key.is_empty() {
            break;
        }
        let (value, next) = cstring(body, next);
        pairs.push((key, value));
        at = next;
    }
    pairs
}

/// The NUL-terminated string at `at`, and the index after it.
fn cstring(body: &[u8], at: usize) -> (String, usize) {
    let rest = body.get(at..).unwrap_or_default();
    let end = rest.iter().position(|byte| *byte == 0).unwrap_or(rest.len());
    let text = String::from_utf8_lossy(rest.get(..end).unwrap_or_default()).into_owned();
    (text, at + end + 1)
}

/// Append a NUL-terminated string.
fn put_cstring(body: &mut Vec<u8>, text: &str) {
    body.extend_from_slice(text.as_bytes());
    body.push(0);
}
