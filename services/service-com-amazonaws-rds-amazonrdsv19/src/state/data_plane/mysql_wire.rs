// SPDX-License-Identifier: CC0-1.0

//! The server half of the MySQL client/server protocol (protocol version 10) for a DB
//! instance's engine endpoint, which is what the MySQL-compatible engines publish.
//!
//! A stock client (libmysqlclient, PyMySQL, a JDBC driver) connects to the published
//! `Endpoint{Address,Port}` and speaks this. What it gets is this emulator's own state:
//! the greeting's server version is the instance's `EngineVersion`, the login is admitted
//! against the stored DB instance ([`wire_session::admit`]), and every statement runs
//! against the instance's SQL engine.
//!
//! The order of a connection, which is the protocol's and not this emulator's:
//!
//! 1. the server writes its greeting IN THE CLEAR — the version, the connection id, the
//!    nonce and the capabilities it offers, `CLIENT_SSL` among them;
//! 2. the client answers. A client that wants TLS answers with the first 32 bytes only
//!    and `CLIENT_SSL` set, the core's TLS upgrader runs the handshake, and the full
//!    handshake response arrives over the upgraded stream;
//! 3. the server checks the `mysql_native_password` proof against the stored master
//!    password and answers `OK`, or one `ERR` packet with the error number and SQLSTATE
//!    of the refusal, and closes;
//! 4. the command phase: `COM_QUERY`, `COM_PING`, `COM_INIT_DB`, `COM_QUIT`. A command
//!    this server does not serve is answered with the protocol's own "unknown command"
//!    error, never with a fabricated success. The session ends when the client quits or
//!    when the engine's run ends under it (a reboot, a stop).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::sql_engine::{Cell, ColumnInfo, EngineError, EngineErrorKind, InstanceDatabase};
use super::tls_probe::{HandshakeRecord, HandshakeTap};
use super::wire_session::{
    self, LoginRefusal, PasswordProof, SessionContext, SessionFacts, TlsFacts,
};
use super::{server_statements, EngineWire};

/// How many bytes one packet may carry. The protocol allows 16 MiB per packet; this is
/// what this endpoint serves, so a length prefix cannot make it allocate without bound.
const MAX_PACKET_BYTES: usize = 1024 * 1024;

/// The capability flags this server offers, from the protocol's own list.
const CLIENT_LONG_PASSWORD: u32 = 0x0000_0001;
const CLIENT_FOUND_ROWS: u32 = 0x0000_0002;
const CLIENT_LONG_FLAG: u32 = 0x0000_0004;
const CLIENT_CONNECT_WITH_DB: u32 = 0x0000_0008;
const CLIENT_PROTOCOL_41: u32 = 0x0000_0200;
const CLIENT_SSL: u32 = 0x0000_0800;
const CLIENT_TRANSACTIONS: u32 = 0x0000_2000;
const CLIENT_SECURE_CONNECTION: u32 = 0x0000_8000;
const CLIENT_PLUGIN_AUTH: u32 = 0x0008_0000;
const CLIENT_CONNECT_ATTRS: u32 = 0x0010_0000;
const CLIENT_PLUGIN_AUTH_LENENC_CLIENT_DATA: u32 = 0x0020_0000;
const CLIENT_DEPRECATE_EOF: u32 = 0x0100_0000;

/// What the greeting offers.
const SERVER_CAPABILITIES: u32 = CLIENT_LONG_PASSWORD
    | CLIENT_FOUND_ROWS
    | CLIENT_LONG_FLAG
    | CLIENT_CONNECT_WITH_DB
    | CLIENT_PROTOCOL_41
    | CLIENT_SSL
    | CLIENT_TRANSACTIONS
    | CLIENT_SECURE_CONNECTION
    | CLIENT_PLUGIN_AUTH
    | CLIENT_CONNECT_ATTRS
    | CLIENT_PLUGIN_AUTH_LENENC_CLIENT_DATA
    | CLIENT_DEPRECATE_EOF;

/// `SERVER_STATUS_AUTOCOMMIT`, which is the status a session starts in.
const SERVER_STATUS_AUTOCOMMIT: u16 = 0x0002;
/// `SERVER_STATUS_IN_TRANS`, set while a transaction is open.
const SERVER_STATUS_IN_TRANS: u16 = 0x0001;

/// The character set the greeting names: `utf8mb4_0900_ai_ci`, the default collation of
/// the MySQL 8 engines, whose character set is `utf8mb4`.
const COLLATION_UTF8MB4: u8 = 255;

/// The collation id a text column is reported under (`utf8mb4_0900_ai_ci`) and the one a
/// binary column is (`binary`).
const COLLATION_TEXT: u16 = 255;
const COLLATION_BINARY: u16 = 63;

/// The authentication plugin the greeting names.
const AUTH_PLUGIN: &str = "mysql_native_password";

/// How long the nonce in the greeting is.
const NONCE_BYTES: usize = 20;

/// The MySQL wire handler for one DB instance's listener.
pub struct MySqlWire {
    context: SessionContext,
}

impl MySqlWire {
    /// The handler for the instance `context` names.
    #[must_use]
    pub fn new(context: SessionContext) -> Self {
        Self { context }
    }
}

impl ::emulator_server::NativeHandler for MySqlWire {
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

/// Rolls back whatever a session had not committed when it ends — on every path,
/// including a severed connection — and takes the session out of the endpoint's registry.
///
/// The registry entry goes on the dropping thread, so the next login is counted against
/// `max_connections` correctly at once; the ROLLBACK is engine work and goes to the blocking
/// pool ([`wire_session::off_the_async_worker`]), because a drop runs on an async worker and
/// `Drop` cannot await.
struct Session {
    database: Arc<InstanceDatabase>,
    sessions: super::pg_sessions::SessionRegistry,
    connection_id: u32,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.sessions.ended(self.connection_id);
        let engine = Arc::clone(&self.database);
        let connection_id = self.connection_id;
        wire_session::off_the_async_worker(move || {
            let _ = engine.rollback_transaction(connection_id);
        });
    }
}

/// One connection, from the greeting to its last packet.
async fn serve_session(
    context: SessionContext,
    conn: ::emulator_server::ListenerConn,
) -> std::io::Result<()> {
    let mut conn = conn;
    let connection_id = context.take_connection_id();
    let Some(facts) = context.endpoint_facts() else {
        // No instance behind this listener any more: the protocol's own "server shutting
        // down" is the honest answer, sent as the first packet the client reads.
        return write_error(&mut conn, 0, 1053, "08S01", "Server shutdown in progress").await;
    };
    let nonce = session_nonce()?;

    // 1. The greeting, in the clear.
    let greeting = greeting_packet(&facts.server_version, connection_id, &nonce);
    write_packet(&mut conn, 0, &greeting).await?;

    // 2. The handshake response, with the TLS upgrade in the middle when the client asks
    //    for one.
    let (sequence, response) = read_packet(&mut conn).await?.ok_or_else(closed)?;
    let mut sequence = sequence;
    let mut client_capabilities = u32::from_le_bytes([
        *response.first().unwrap_or(&0),
        *response.get(1).unwrap_or(&0),
        *response.get(2).unwrap_or(&0),
        *response.get(3).unwrap_or(&0),
    ]);
    let mut tls: Option<TlsFacts> = None;
    let login = if client_capabilities & CLIENT_SSL != 0 && response.len() <= 36 {
        let record = HandshakeRecord::default();
        let tapped: ::emulator_server::ListenerConn =
            Box::new(HandshakeTap::new(conn, record.clone()));
        let (upgraded, _alpn) = context.upgrader.accept(tapped, None).await?;
        conn = upgraded;
        tls = Some(record.negotiated());
        let (next, response) = read_packet(&mut conn).await?.ok_or_else(closed)?;
        sequence = next;
        client_capabilities = u32::from_le_bytes([
            *response.first().unwrap_or(&0),
            *response.get(1).unwrap_or(&0),
            *response.get(2).unwrap_or(&0),
            *response.get(3).unwrap_or(&0),
        ]);
        handshake_response(&response, client_capabilities)
    } else {
        handshake_response(&response, client_capabilities)
    };

    // 3. The refusals, then the password proof.
    if let Err(refusal) = wire_session::precheck(&facts, tls.is_some()) {
        wire_session::record_refused_login(&facts, &refusal);
        let (number, state, message) = refusal_sentence(&refusal, &facts, &login.user, tls.is_some());
        return write_error(&mut conn, sequence.wrapping_add(1), number, state, &message).await;
    }
    let database = match wire_session::admit(
        &facts,
        &login.user,
        login.database.as_deref(),
        tls.is_some(),
        &PasswordProof::MysqlNative {
            nonce: &nonce,
            response: &login.auth_response,
        },
        // IAM database authentication on a MySQL instance is an AUTHENTICATION PLUGIN on
        // the account (`IDENTIFIED WITH AWSAuthenticationPlugin`), which this wire's
        // handshake does not negotiate and this endpoint's MySQL half keeps no account
        // table for: no login here presents an authentication token, so none is read.
        None,
    ) {
        Ok(database) => database,
        Err(refusal) => {
            wire_session::record_refused_login(&facts, &refusal);
            let (number, state, message) =
                refusal_sentence(&refusal, &facts, &login.user, tls.is_some());
            return write_error(&mut conn, sequence.wrapping_add(1), number, state, &message)
                .await;
        }
    };

    // 4. The session is open.
    let mut session_facts = SessionFacts {
        wire: EngineWire::MySql,
        identifier: facts.identifier.clone(),
        user: login.user.clone(),
        database,
        connection_id,
        tls,
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
    // `max_connections` bounds the sessions a MySQL server holds at once too, and the
    // server refuses the connection that would exceed it.
    if let Some(limit) = session_facts.runtime.numeric_parameter("max_connections") {
        if context.sessions.open_count() as u64 >= limit {
            return write_error(
                &mut conn,
                sequence.wrapping_add(1),
                1040,
                "08004",
                "Too many connections",
            )
            .await;
        }
    }
    let database_handle = Arc::clone(&context.database);
    let _session = Session {
        database: Arc::clone(&database_handle),
        sessions: context.sessions.clone(),
        connection_id,
    };
    context.sessions.admitted(super::pg_sessions::LiveSession {
        pid: connection_id,
        user: session_facts.user.clone(),
        database: session_facts.database.clone(),
        application_name: String::new(),
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
    let deprecate_eof = client_capabilities & CLIENT_DEPRECATE_EOF != 0;
    write_ok(
        &mut conn,
        sequence.wrapping_add(1),
        &database_handle,
        session_facts.connection_id,
        0,
        0,
    )
    .await?;

    // 5. The command phase.
    let mut generation = context.generation.clone();
    // The engine's run as it is NOW is what this session serves under: marking the
    // current value seen is what makes the wait below fire on the NEXT change (the reboot
    // or stop that ends this session) rather than on one that happened before it opened.
    let _ = generation.borrow_and_update();
    loop {
        let packet = tokio::select! {
            // The engine's run ended under this session: a reboot or a stop severs it,
            // which the protocol reports as the server going away before the socket
            // closes.
            _ = generation.changed() => {
                let _ = write_error(
                    &mut conn,
                    1,
                    1053,
                    "08S01",
                    "Server shutdown in progress",
                )
                .await;
                return Ok(());
            }
            packet = read_packet(&mut conn) => packet?,
        };
        let Some((sequence, payload)) = packet else {
            return Ok(());
        };
        let command = *payload.first().unwrap_or(&1);
        let argument = payload.get(1..).unwrap_or_default();
        match command {
            // COM_QUIT.
            0x01 => return Ok(()),
            // COM_INIT_DB.
            0x02 => {
                let named = String::from_utf8_lossy(argument).into_owned();
                if server_statements::database_exists(&database_handle, &session_facts, &named) {
                    session_facts.database = named;
                    write_ok(
                        &mut conn,
                        sequence.wrapping_add(1),
                        &database_handle,
                        session_facts.connection_id,
                        0,
                        0,
                    )
                    .await?;
                } else {
                    write_error(
                        &mut conn,
                        sequence.wrapping_add(1),
                        1049,
                        "42000",
                        &format!("Unknown database '{named}'"),
                    )
                    .await?;
                }
            }
            // COM_QUERY.
            0x03 => {
                let sql = String::from_utf8_lossy(argument).into_owned();
                serve_query(
                    &mut conn,
                    sequence.wrapping_add(1),
                    &context,
                    &database_handle,
                    &mut session_facts,
                    &sql,
                    deprecate_eof,
                )
                .await?;
            }
            // COM_PING.
            0x0e => {
                write_ok(
                    &mut conn,
                    sequence.wrapping_add(1),
                    &database_handle,
                    session_facts.connection_id,
                    0,
                    0,
                )
                .await?;
            }
            // Every other command — the prepared-statement protocol, the replication
            // commands, `COM_CHANGE_USER` — is one this endpoint does not serve, and the
            // protocol has an error for exactly that.
            other => {
                write_error(
                    &mut conn,
                    sequence.wrapping_add(1),
                    1047,
                    "08S01",
                    &format!("Unknown command {other}"),
                )
                .await?;
            }
        }
    }
}

/// What the client's handshake response said.
struct Login {
    user: String,
    database: Option<String>,
    auth_response: Vec<u8>,
}

/// Read a `HandshakeResponse41`: the capabilities, the user, the authentication response
/// and the database it asked for.
fn handshake_response(payload: &[u8], capabilities: u32) -> Login {
    // capability(4) max_packet(4) charset(1) filler(23) then the strings.
    let mut at = 32usize;
    let (user, next) = read_cstring(payload, at);
    at = next;
    let auth_response = if capabilities & CLIENT_PLUGIN_AUTH_LENENC_CLIENT_DATA != 0 {
        let (length, next) = read_length_encoded(payload, at);
        at = next;
        let length = usize::try_from(length).unwrap_or_default();
        let bytes = payload
            .get(at..at + length.min(payload.len().saturating_sub(at)))
            .unwrap_or_default()
            .to_vec();
        at += bytes.len();
        bytes
    } else {
        let length = usize::from(*payload.get(at).unwrap_or(&0));
        at += 1;
        let bytes = payload
            .get(at..at + length.min(payload.len().saturating_sub(at)))
            .unwrap_or_default()
            .to_vec();
        at += bytes.len();
        bytes
    };
    let database = if capabilities & CLIENT_CONNECT_WITH_DB != 0 {
        let (database, _) = read_cstring(payload, at);
        (!database.is_empty()).then_some(database)
    } else {
        None
    };
    Login {
        user,
        database,
        auth_response,
    }
}

/// The server greeting (`HandshakeV10`).
fn greeting_packet(server_version: &str, connection_id: u32, nonce: &[u8]) -> Vec<u8> {
    let mut packet = vec![10u8];
    packet.extend_from_slice(server_version.as_bytes());
    packet.push(0);
    packet.extend_from_slice(&connection_id.to_le_bytes());
    packet.extend_from_slice(nonce.get(..8).unwrap_or_default());
    packet.push(0);
    let capabilities = SERVER_CAPABILITIES.to_le_bytes();
    packet.extend_from_slice(capabilities.get(..2).unwrap_or_default());
    packet.push(COLLATION_UTF8MB4);
    packet.extend_from_slice(&SERVER_STATUS_AUTOCOMMIT.to_le_bytes());
    packet.extend_from_slice(capabilities.get(2..4).unwrap_or_default());
    packet.push(u8::try_from(NONCE_BYTES + 1).unwrap_or_default());
    packet.extend_from_slice(&[0u8; 10]);
    packet.extend_from_slice(nonce.get(8..).unwrap_or_default());
    packet.push(0);
    packet.extend_from_slice(AUTH_PLUGIN.as_bytes());
    packet.push(0);
    packet
}

/// The nonce this session challenges the client with.
///
/// Random per connection, from the operating system's own entropy, because that is what
/// makes the proof the client answers with non-replayable: the password itself never
/// crosses the wire, only a digest over this nonce.
fn session_nonce() -> std::io::Result<Vec<u8>> {
    let mut bytes = [0u8; NONCE_BYTES];
    ::getrandom::fill(&mut bytes).map_err(|error| std::io::Error::other(error.to_string()))?;
    // The protocol's nonce is a string, so no byte of it may be NUL; each byte is mapped
    // into the printable range the servers themselves use.
    Ok(bytes
        .iter()
        .map(|byte| 0x21 + (byte % 0x5e))
        .collect())
}

/// Serve one `COM_QUERY`.
async fn serve_query(
    conn: &mut ::emulator_server::ListenerConn,
    sequence: u8,
    context: &SessionContext,
    database: &Arc<InstanceDatabase>,
    facts: &mut SessionFacts,
    sql: &str,
    deprecate_eof: bool,
) -> std::io::Result<()> {
    let statements = match wire_session::split_statements(facts.wire, sql) {
        Ok(statements) => statements,
        Err(error) => return write_engine_error(conn, sequence, &error).await,
    };
    // `CLIENT_MULTI_STATEMENTS` is not among the capabilities this server offers, so a
    // command carries one statement; the protocol's own syntax error says so.
    if statements.len() > 1 {
        return write_error(
            conn,
            sequence,
            1064,
            "42000",
            "You have an error in your SQL syntax; this server serves one statement per command",
        )
        .await;
    }
    let Some(statement) = statements.into_iter().next() else {
        return write_error(conn, sequence, 1065, "42000", "Query was empty").await;
    };
    let answer = {
        // The instance's engine runtime as it is NOW, so a parameter group a caller
        // modifies under a live session is what this statement runs against — and so is the
        // endpoint's write role, which a failover moves.
        if let Some((runtime, read_only)) = context.engine_now() {
            facts.runtime = runtime;
            facts.read_only = read_only;
        }
        // `LOAD DATA FROM S3` is not a statement the SQL engine runs: the rows come from an
        // Amazon S3 object the engine reads under the role this resource is configured with.
        if let Some(parsed) = super::mysql_s3_load::s3_load_statement(facts, &statement) {
            return serve_s3_load(conn, sequence, context, database, facts, parsed).await;
        }
        let database = Arc::clone(database);
        let session = facts.clone();
        let statement = statement.clone();
        tokio::task::spawn_blocking(move || {
            wire_session::run_statement(&database, &session, &statement, &[])
        })
        .await
        .map_err(|error| std::io::Error::other(error.to_string()))?
    };
    let answer = match answer {
        Ok(answer) => answer,
        Err(error) => return write_engine_error(conn, sequence, &error).await,
    };
    if let Some((name, value)) = answer.setting_changed.clone() {
        facts.settings.insert(name, value);
    }
    if let Some(database) = answer.database_changed.clone() {
        facts.database = database;
    }
    // A statement that created or dropped a schema changed what this session may name.
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
    if answer.result.columns.is_empty() {
        return write_ok(
            conn,
            sequence,
            database,
            facts.connection_id,
            answer.result.affected,
            answer.result.last_insert_id,
        )
        .await;
    }
    write_result_set(
        conn,
        sequence,
        database,
        facts.connection_id,
        &answer.result,
        deprecate_eof,
    )
    .await
}

/// Serve one `LOAD DATA FROM S3`: read the objects it names out of Amazon S3 under the
/// resource's import role, load their records, and answer with how many rows were loaded —
/// or with the error the load stopped on.
async fn serve_s3_load(
    conn: &mut ::emulator_server::ListenerConn,
    sequence: u8,
    context: &SessionContext,
    database: &Arc<InstanceDatabase>,
    facts: &SessionFacts,
    parsed: Result<super::mysql_s3_load::S3Load, EngineError>,
) -> std::io::Result<()> {
    let statement = match parsed {
        Ok(statement) => statement,
        Err(error) => return write_engine_error(conn, sequence, &error).await,
    };
    let objects = match super::mysql_s3_load::objects_to_load(context, facts, &statement) {
        Ok(objects) => objects,
        Err(error) => return write_engine_error(conn, sequence, &error).await,
    };
    let loaded = {
        let database = Arc::clone(database);
        let session = facts.clone();
        let statement = statement.clone();
        tokio::task::spawn_blocking(move || {
            super::mysql_s3_load::load(&database, &session, &statement, &objects)
        })
        .await
        .map_err(|error| std::io::Error::other(error.to_string()))?
    };
    match loaded {
        Ok(loaded) => write_ok(conn, sequence, database, facts.connection_id, loaded, 0).await,
        Err(error) => write_engine_error(conn, sequence, &error).await,
    }
}

/// A result set in the text protocol: the column count, one definition per column, the
/// rows, and the packet that ends them.
async fn write_result_set(
    conn: &mut ::emulator_server::ListenerConn,
    sequence: u8,
    database: &Arc<InstanceDatabase>,
    session: u32,
    result: &super::sql_engine::ResultSet,
    deprecate_eof: bool,
) -> std::io::Result<()> {
    let mut sequence = sequence;
    let mut header = Vec::new();
    put_length_encoded(
        &mut header,
        u64::try_from(result.columns.len()).unwrap_or(u64::MAX),
    );
    write_packet(conn, sequence, &header).await?;
    for (index, column) in result.columns.iter().enumerate() {
        sequence = sequence.wrapping_add(1);
        let sample = result.rows.first().and_then(|row| row.get(index));
        write_packet(conn, sequence, &column_definition(column, sample)).await?;
    }
    if !deprecate_eof {
        sequence = sequence.wrapping_add(1);
        write_packet(conn, sequence, &eof_packet(database, session)).await?;
    }
    for row in &result.rows {
        let mut payload = Vec::new();
        for (index, cell) in row.iter().enumerate() {
            match render_cell(result.columns.get(index), cell) {
                None => payload.push(0xfb),
                Some(text) => put_length_encoded_string(&mut payload, text.as_bytes()),
            }
        }
        sequence = sequence.wrapping_add(1);
        write_packet(conn, sequence, &payload).await?;
    }
    sequence = sequence.wrapping_add(1);
    if deprecate_eof {
        let payload = ok_payload(database, session, 0, 0, true);
        write_packet(conn, sequence, &payload).await?;
    } else {
        write_packet(conn, sequence, &eof_packet(database, session)).await?;
    }
    conn.flush().await
}

/// One column definition packet (`ColumnDefinition41`).
fn column_definition(column: &ColumnInfo, sample: Option<&Cell>) -> Vec<u8> {
    let (column_type, binary) = column_type(column, sample);
    let mut payload = Vec::new();
    put_length_encoded_string(&mut payload, b"def");
    for part in [b"".as_slice(), b"".as_slice(), b"".as_slice()] {
        put_length_encoded_string(&mut payload, part);
    }
    put_length_encoded_string(&mut payload, column.name.as_bytes());
    put_length_encoded_string(&mut payload, column.name.as_bytes());
    put_length_encoded(&mut payload, 0x0c);
    payload.extend_from_slice(
        &if binary { COLLATION_BINARY } else { COLLATION_TEXT }.to_le_bytes(),
    );
    // The declared maximum display width; the engine does not keep one, so the column is
    // reported with the largest value this endpoint serves.
    payload.extend_from_slice(&u32::try_from(MAX_PACKET_BYTES).unwrap_or(u32::MAX).to_le_bytes());
    payload.push(column_type);
    payload.extend_from_slice(&0u16.to_le_bytes());
    payload.push(0);
    payload.extend_from_slice(&[0u8, 0u8]);
    payload
}

/// The MySQL column type a column is reported under, and whether it is binary.
///
/// Source: the type codes in the protocol's `ColumnDefinition41` (`MYSQL_TYPE_*`) and the
/// type names the MySQL reference manual's "Data Types" chapter documents. A column with
/// no declaration — a computed one — is reported under the type of the value the engine
/// holds for it.
fn column_type(column: &ColumnInfo, sample: Option<&Cell>) -> (u8, bool) {
    const TINY: u8 = 1;
    const SHORT: u8 = 2;
    const LONG: u8 = 3;
    const DOUBLE: u8 = 5;
    const TIMESTAMP: u8 = 7;
    const LONGLONG: u8 = 8;
    const DATE: u8 = 10;
    const TIME: u8 = 11;
    const DATETIME: u8 = 12;
    const JSON: u8 = 245;
    const NEWDECIMAL: u8 = 246;
    const BLOB: u8 = 252;
    const VAR_STRING: u8 = 253;
    if let Some(declared) = &column.declared_type {
        let lowered = declared.to_ascii_lowercase();
        let base = lowered.split('(').next().unwrap_or(&lowered).trim();
        let answer = match base {
            "tinyint" | "bool" | "boolean" => Some((TINY, true)),
            "smallint" => Some((SHORT, true)),
            "mediumint" | "int" | "integer" => Some((LONG, true)),
            "bigint" => Some((LONGLONG, true)),
            "float" | "double" | "real" | "double precision" => Some((DOUBLE, true)),
            "decimal" | "numeric" => Some((NEWDECIMAL, true)),
            "date" => Some((DATE, true)),
            "time" => Some((TIME, true)),
            "datetime" => Some((DATETIME, true)),
            "timestamp" => Some((TIMESTAMP, true)),
            "json" => Some((JSON, false)),
            "blob" | "bytea" | "binary" | "varbinary" => Some((BLOB, true)),
            "char" | "varchar" | "text" | "tinytext" | "mediumtext" | "longtext" => {
                Some((VAR_STRING, false))
            }
            _ => None,
        };
        if let Some(answer) = answer {
            return answer;
        }
    }
    match sample {
        Some(Cell::Integer(_)) => (LONGLONG, true),
        Some(Cell::Real(_)) => (DOUBLE, true),
        Some(Cell::Blob(_)) => (BLOB, true),
        _ => (VAR_STRING, false),
    }
}

/// One value in the text protocol, or `None` for SQL NULL.
///
/// MySQL's text protocol renders every value as a string; a `TINYINT(1)`/`BOOLEAN` column
/// renders as the `0`/`1` MySQL stores it as.
fn render_cell(column: Option<&ColumnInfo>, cell: &Cell) -> Option<String> {
    let boolean = column
        .and_then(|column| column.declared_type.as_ref())
        .is_some_and(|declared| {
            let lowered = declared.to_ascii_lowercase();
            lowered == "bool" || lowered == "boolean"
        });
    match cell {
        Cell::Null => None,
        Cell::Integer(value) if boolean => Some(i64::from(*value != 0).to_string()),
        Cell::Integer(value) => Some(value.to_string()),
        Cell::Real(value) => Some(value.to_string()),
        Cell::Text(value) if boolean => Some(
            match value.to_ascii_lowercase().as_str() {
                "0" | "f" | "false" | "n" | "no" | "off" => "0",
                _ => "1",
            }
            .to_string(),
        ),
        Cell::Text(value) => Some(value.clone()),
        Cell::Blob(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
    }
}

/// The error number and SQLSTATE MySQL refuses a login with, and its own sentence for it.
fn refusal_sentence(
    refusal: &LoginRefusal,
    facts: &wire_session::InstanceFacts,
    user: &str,
    encrypted: bool,
) -> (u16, &'static str, String) {
    let _ = encrypted;
    match refusal {
        // `ER_SERVER_SHUTDOWN`: there is no engine to connect to.
        LoginRefusal::NotRunning { .. } => (
            1053,
            "08S01",
            "Server shutdown in progress".to_string(),
        ),
        // `ER_SECURE_TRANSPORT_REQUIRED`, which is what `require_secure_transport` adds.
        LoginRefusal::PlaintextRefused => (
            3159,
            "HY000",
            "Connections using insecure transport are prohibited while \
             --require_secure_transport=ON."
                .to_string(),
        ),
        // `ER_BAD_DB_ERROR`.
        LoginRefusal::UnknownDatabase { database } => (
            1049,
            "42000",
            format!("Unknown database '{database}'"),
        ),
        // `ER_ACCESS_DENIED_ERROR`, whose message names the account the login was for and
        // whether a password was sent at all.
        LoginRefusal::AuthenticationFailed {
            user: _,
            password_given,
        } => (
            1045,
            "28000",
            format!(
                "Access denied for user '{}'@'{}' (using password: {})",
                user,
                facts.endpoint_host,
                if *password_given { "YES" } else { "NO" }
            ),
        ),
    }
}

/// The error number and SQLSTATE a failed statement is reported under.
///
/// Source: the server error numbers and SQLSTATE values in the MySQL reference manual's
/// "Server Error Message Reference", which is what a client branches on.
fn statement_error(kind: EngineErrorKind) -> (u16, &'static str) {
    match kind {
        EngineErrorKind::Syntax => (1064, "42000"),
        EngineErrorKind::UndefinedObject => (1146, "42S02"),
        EngineErrorKind::UndefinedDatabase => (1049, "42000"),
        EngineErrorKind::UniqueViolation => (1062, "23000"),
        EngineErrorKind::NotNullViolation => (1048, "23000"),
        EngineErrorKind::CheckViolation => (3819, "HY000"),
        EngineErrorKind::ForeignKeyViolation => (1452, "23000"),
        // `ER_LOCK_NOWAIT` and `ER_LOCK_DEADLOCK`: a row another transaction holds, which a
        // `NOWAIT` statement is refused for and which a cycle of waiting transactions is
        // broken with.
        EngineErrorKind::LockNotAvailable => (3572, "HY000"),
        EngineErrorKind::Deadlock => (1213, "40001"),
        // `ER_WARN_DATA_OUT_OF_RANGE` and `ER_DIVISION_BY_ZERO`: the two conditions the
        // exact decimal arithmetic reports (`super::pg_numeric`).
        EngineErrorKind::NumericValueOutOfRange => (1264, "22003"),
        EngineErrorKind::DivisionByZero => (1365, "22012"),
        // `ER_LOCK_DEADLOCK` is also the number the server reports a transaction it could
        // not serialize under, which is the same SQLSTATE class a client branches on.
        EngineErrorKind::SerializationFailure => (1213, "40001"),
        EngineErrorKind::DataError => (1366, "HY000"),
        EngineErrorKind::Cancelled => (1317, "70100"),
        // `ER_UNKNOWN_ERROR`: the statement asked for more than this endpoint serves,
        // which is not one of the server's own numbered conditions.
        EngineErrorKind::LimitExceeded => (1105, "HY000"),
        EngineErrorKind::InsufficientPrivilege => (1227, "42000"),
        // `ER_OPTION_PREVENTS_STATEMENT`: the server's `read_only` setting permits no client
        // update, which is what a read-only endpoint's session is held to.
        EngineErrorKind::ReadOnlySqlTransaction => super::read_only::MYSQL_READ_ONLY_ERROR,
        // `ER_TABLE_EXISTS_ERROR` and `ER_ROW_IS_REFERENCED`: the general number of each
        // class, which a statement whose own condition names a narrower one overrides
        // ([`EngineError::with_mysql_error`]).
        EngineErrorKind::DuplicateObject => (1050, "42S01"),
        EngineErrorKind::ObjectInUse => (1217, "23000"),
        // `ER_NOT_SUPPORTED_YET`.
        EngineErrorKind::FeatureNotSupported => (1235, "42000"),
        EngineErrorKind::Internal => (1105, "HY000"),
    }
}

/// Report a failed statement, keeping the engine's own message.
async fn write_engine_error(
    conn: &mut ::emulator_server::ListenerConn,
    sequence: u8,
    error: &EngineError,
) -> std::io::Result<()> {
    let (number, state) = error
        .mysql_error
        .unwrap_or_else(|| statement_error(error.kind));
    write_error(conn, sequence, number, state, &error.message).await
}

/// One `ERR` packet.
async fn write_error(
    conn: &mut ::emulator_server::ListenerConn,
    sequence: u8,
    number: u16,
    state: &str,
    message: &str,
) -> std::io::Result<()> {
    let mut payload = vec![0xffu8];
    payload.extend_from_slice(&number.to_le_bytes());
    payload.push(b'#');
    payload.extend_from_slice(state.as_bytes());
    payload.extend_from_slice(message.as_bytes());
    write_packet(conn, sequence, &payload).await?;
    conn.flush().await
}

/// One `OK` packet.
async fn write_ok(
    conn: &mut ::emulator_server::ListenerConn,
    sequence: u8,
    database: &Arc<InstanceDatabase>,
    session: u32,
    affected: u64,
    last_insert_id: i64,
) -> std::io::Result<()> {
    let payload = ok_payload(database, session, affected, last_insert_id, false);
    write_packet(conn, sequence, &payload).await?;
    conn.flush().await
}

/// The body of an `OK` packet, which also ends a result set for a client that asked for
/// `CLIENT_DEPRECATE_EOF`.
fn ok_payload(
    database: &Arc<InstanceDatabase>,
    session: u32,
    affected: u64,
    last_insert_id: i64,
    ends_result_set: bool,
) -> Vec<u8> {
    let mut payload = vec![if ends_result_set { 0xfe } else { 0x00 }];
    put_length_encoded(&mut payload, affected);
    put_length_encoded(&mut payload, last_insert_id.max(0).unsigned_abs());
    payload.extend_from_slice(&status_flags(database, session).to_le_bytes());
    payload.extend_from_slice(&0u16.to_le_bytes());
    payload
}

/// One `EOF` packet, for a client that did not ask for `CLIENT_DEPRECATE_EOF`.
fn eof_packet(database: &Arc<InstanceDatabase>, session: u32) -> Vec<u8> {
    let mut payload = vec![0xfeu8];
    payload.extend_from_slice(&0u16.to_le_bytes());
    payload.extend_from_slice(&status_flags(database, session).to_le_bytes());
    payload
}

/// The session's status flags: autocommit, and whether THIS SESSION has a transaction open.
fn status_flags(database: &Arc<InstanceDatabase>, session: u32) -> u16 {
    if database.in_transaction(session) {
        SERVER_STATUS_AUTOCOMMIT | SERVER_STATUS_IN_TRANS
    } else {
        SERVER_STATUS_AUTOCOMMIT
    }
}

/// Write one packet: its length, its sequence id and its payload.
async fn write_packet(
    conn: &mut ::emulator_server::ListenerConn,
    sequence: u8,
    payload: &[u8],
) -> std::io::Result<()> {
    let length = payload.len();
    if length > MAX_PACKET_BYTES {
        return Err(std::io::Error::other(
            "the answer is larger than this endpoint serves",
        ));
    }
    let length = u32::try_from(length).unwrap_or_default().to_le_bytes();
    conn.write_all(length.get(..3).unwrap_or_default()).await?;
    conn.write_all(&[sequence]).await?;
    conn.write_all(payload).await
}

/// Read one packet: its sequence id and its payload, or `None` at end of stream.
async fn read_packet(
    conn: &mut ::emulator_server::ListenerConn,
) -> std::io::Result<Option<(u8, Vec<u8>)>> {
    let mut header = [0u8; 4];
    match conn.read_exact(&mut header).await {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let length = usize::from(header[0])
        | (usize::from(header[1]) << 8)
        | (usize::from(header[2]) << 16);
    if length > MAX_PACKET_BYTES {
        return Err(std::io::Error::other(
            "the packet is larger than this endpoint serves",
        ));
    }
    let mut payload = vec![0u8; length];
    conn.read_exact(&mut payload).await?;
    Ok(Some((header[3], payload)))
}

/// The client went away before it said what it was doing.
fn closed() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        "the client closed the connection during the handshake",
    )
}

/// Append a length-encoded integer, in the protocol's own form.
fn put_length_encoded(payload: &mut Vec<u8>, value: u64) {
    match value {
        0..=250 => payload.push(u8::try_from(value).unwrap_or_default()),
        251..=0xffff => {
            payload.push(0xfc);
            payload.extend_from_slice(
                &u16::try_from(value).unwrap_or(u16::MAX).to_le_bytes(),
            );
        }
        0x1_0000..=0xff_ffff => {
            payload.push(0xfd);
            let bytes = u32::try_from(value).unwrap_or(u32::MAX).to_le_bytes();
            payload.extend_from_slice(bytes.get(..3).unwrap_or_default());
        }
        _ => {
            payload.push(0xfe);
            payload.extend_from_slice(&value.to_le_bytes());
        }
    }
}

/// Append a length-encoded string.
fn put_length_encoded_string(payload: &mut Vec<u8>, bytes: &[u8]) {
    put_length_encoded(payload, u64::try_from(bytes.len()).unwrap_or(u64::MAX));
    payload.extend_from_slice(bytes);
}

/// Read a length-encoded integer at `at`, and the index after it.
fn read_length_encoded(payload: &[u8], at: usize) -> (u64, usize) {
    match payload.get(at) {
        None => (0, at + 1),
        Some(0xfc) => (
            u64::from(u16::from_le_bytes([
                *payload.get(at + 1).unwrap_or(&0),
                *payload.get(at + 2).unwrap_or(&0),
            ])),
            at + 3,
        ),
        Some(0xfd) => (
            u64::from(u32::from_le_bytes([
                *payload.get(at + 1).unwrap_or(&0),
                *payload.get(at + 2).unwrap_or(&0),
                *payload.get(at + 3).unwrap_or(&0),
                0,
            ])),
            at + 4,
        ),
        Some(0xfe) => {
            let mut bytes = [0u8; 8];
            for (index, byte) in bytes.iter_mut().enumerate() {
                *byte = *payload.get(at + 1 + index).unwrap_or(&0);
            }
            (u64::from_le_bytes(bytes), at + 9)
        }
        Some(value) => (u64::from(*value), at + 1),
    }
}

/// The NUL-terminated string at `at`, and the index after it.
fn read_cstring(payload: &[u8], at: usize) -> (String, usize) {
    let rest = payload.get(at..).unwrap_or_default();
    let end = rest.iter().position(|byte| *byte == 0).unwrap_or(rest.len());
    let text = String::from_utf8_lossy(rest.get(..end).unwrap_or_default()).into_owned();
    (text, at + end + 1)
}
