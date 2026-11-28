// SPDX-License-Identifier: CC0-1.0

//! The server half of the MySQL client/server protocol (protocol version 10),
//! which is what the endpoint of a MySQL-compatible database resource answers.
//!
//! A stock client (PyMySQL, libmysqlclient, a JDBC driver) connects to the
//! published address and port and speaks this. What it gets is this emulator's
//! own state: the greeting names the engine version the resource publishes, the
//! login is admitted against the stored resource ([`super::admit_issued`]), and every
//! statement is run by that database's own SQL engine
//! ([`super::sql_engine`]).
//!
//! The order of a connection, which is the protocol's and not this emulator's:
//!
//! 1. the server writes its greeting IN THE CLEAR — the version, the connection
//!    id, the nonce and the capabilities it offers, `CLIENT_SSL` among them;
//! 2. the client answers. A client that wants TLS answers with the first 32
//!    bytes only and `CLIENT_SSL` set, the core's TLS upgrader runs the
//!    handshake, and the full handshake response arrives over the upgraded
//!    stream;
//! 3. the server checks the `mysql_native_password` proof against the stored
//!    master password and answers `OK`, or one `ERR` packet carrying the error
//!    number and SQLSTATE of the refusal, and closes;
//! 4. the command phase: `COM_QUERY`, `COM_PING`, `COM_INIT_DB`, `COM_QUIT`. A
//!    command this server does not serve is answered with the protocol's own
//!    "unknown command" error, never with a fabricated success.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::session::Session;
use super::sql_engine::{Cell, Column, EngineError, EngineErrorKind, ResultSet};
use super::{
    admit_login, client_host, DatabaseFacts, EngineWire, LoginDenial, LoginRefusal, PasswordProof,
    PresentedLogin, ServingState, SessionContext,
};

/// How many bytes one packet may carry. The protocol allows 16 MiB per packet;
/// this is what this endpoint serves, so a length prefix cannot make it
/// allocate without bound.
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

/// `SERVER_STATUS_AUTOCOMMIT`, the status a session starts in, and
/// `SERVER_STATUS_IN_TRANS`, set while a transaction is open.
const SERVER_STATUS_AUTOCOMMIT: u16 = 0x0002;
const SERVER_STATUS_IN_TRANS: u16 = 0x0001;

/// The character set the greeting names: `utf8mb4_0900_ai_ci`, the default
/// collation of the MySQL 8 and 8.4 engines, whose character set is `utf8mb4`.
const COLLATION_UTF8MB4: u8 = 255;

/// The collation id a text column is reported under (`utf8mb4_0900_ai_ci`) and
/// the one a binary column is (`binary`).
const COLLATION_TEXT: u16 = 255;
const COLLATION_BINARY: u16 = 63;

/// The authentication plugin the greeting names.
const AUTH_PLUGIN: &str = "mysql_native_password";

/// How long the nonce in the greeting is.
const NONCE_BYTES: usize = 20;

/// `ER_SERVER_SHUTDOWN`'s own sentence, which is what a client is told when the
/// database behind this listener is going down.
const SHUTTING_DOWN: &str = "Server shutdown in progress";

/// `ER_SERVER_ISNT_AVAILABLE`'s own sentence, for a database whose engine has
/// not finished coming up.
const NOT_AVAILABLE: &str = "Server isn't available";

/// The MySQL wire handler for one database's listener.
pub struct MySqlWire {
    context: SessionContext,
}

impl MySqlWire {
    /// The handler for the database `context` names.
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
            // An I/O error is the client's connection going away, which ends
            // the session and nothing else.
            let _ = serve_session(context, conn).await;
        })
    }
}

/// One connection, from the greeting to its last packet.
async fn serve_session(
    context: SessionContext,
    conn: ::emulator_server::ListenerConn,
) -> std::io::Result<()> {
    let mut conn = conn;
    let connection_id = context.take_connection_id();
    let Some(facts) = context.facts() else {
        // No database behind this listener any more: the protocol's own "server
        // shutting down" is the honest answer, sent as the first packet the
        // client reads.
        return write_error(&mut conn, 0, 1053, "08S01", SHUTTING_DOWN).await;
    };
    let nonce = session_nonce()?;

    // 1. The greeting, in the clear.
    write_packet(
        &mut conn,
        0,
        &greeting_packet(&facts.server_version, connection_id, &nonce),
    )
    .await?;
    conn.flush().await?;

    // 2. The handshake response, with the TLS upgrade in the middle when the
    //    client asks for one.
    let (mut sequence, response) = read_packet(&mut conn).await?.ok_or_else(closed)?;
    let mut capabilities = client_capabilities(&response);
    if capabilities & CLIENT_SSL != 0 && response.len() <= 36 {
        let Some(upgrader) = context.upgrader() else {
            // The client asked for a session this emulator cannot encrypt. The
            // protocol's own refusal says so, rather than leaving the client
            // waiting for a handshake nothing will answer.
            return write_error(
                &mut conn,
                sequence.wrapping_add(1),
                1045,
                "28000",
                "this endpoint has no TLS prepared, so an encrypted session cannot be opened",
            )
            .await;
        };
        let (upgraded, _alpn) = upgrader.accept(conn, None).await?;
        conn = upgraded;
        let (next, response) = read_packet(&mut conn).await?.ok_or_else(closed)?;
        sequence = next;
        capabilities = client_capabilities(&response);
        let login = handshake_response(&response, capabilities);
        return serve_login(
            conn,
            context,
            facts,
            connection_id,
            sequence,
            capabilities,
            &nonce,
            login,
        )
        .await;
    }
    let login = handshake_response(&response, capabilities);
    serve_login(
        conn,
        context,
        facts,
        connection_id,
        sequence,
        capabilities,
        &nonce,
        login,
    )
    .await
}

/// Admit the login and, when it is admitted, serve the command phase.
#[allow(clippy::too_many_arguments)]
async fn serve_login(
    mut conn: ::emulator_server::ListenerConn,
    context: SessionContext,
    facts: DatabaseFacts,
    connection_id: u32,
    sequence: u8,
    capabilities: u32,
    nonce: &[u8],
    login: Login,
) -> std::io::Result<()> {
    let proof = PasswordProof::MysqlNative {
        nonce,
        response: &login.auth_response,
    };
    let presented = PresentedLogin {
        user: &login.user,
        database: login.database.as_deref(),
        proof: &proof,
        encrypted: capabilities & CLIENT_SSL != 0,
    };
    let database = match admit_login(&context, &facts, &presented) {
        Ok(database) => database,
        // A credential the owning service verified itself and refused: its own
        // words, under this protocol's access-denied error, which is the error a
        // MySQL client branches on. The protocol carries no hint field, so what
        // the service states there is not reported rather than reported
        // somewhere it does not belong.
        Err(LoginDenial::Stated(stated)) => {
            return write_error(
                &mut conn,
                sequence.wrapping_add(1),
                ACCESS_DENIED_ERROR,
                ACCESS_DENIED_STATE,
                &stated.message,
            )
            .await
        }
        Err(LoginDenial::Wire(refusal)) => {
            let (number, state, message) = refusal_sentence(&refusal);
            return write_error(&mut conn, sequence.wrapping_add(1), number, state, &message).await;
        }
    };
    let mut session = Session::new(
        Arc::clone(context.engine()),
        EngineWire::MySql,
        facts,
        login.user.clone(),
        database,
        connection_id,
    )
    .with_optimistic(context.optimistic());
    let deprecate_eof = capabilities & CLIENT_DEPRECATE_EOF != 0;
    write_ok(&mut conn, sequence.wrapping_add(1), &session, 0, 0).await?;

    // The command phase.
    loop {
        let Some((sequence, payload)) = read_packet(&mut conn).await? else {
            return Ok(());
        };
        let sequence = sequence.wrapping_add(1);
        // The resource is read again for every command: one that has been
        // stopped or deleted under this session ends it, which is what a
        // client sees when the server goes away.
        match context.facts() {
            Some(facts) if facts.is_serving() => session.refresh(facts),
            Some(facts) => {
                let (number, state, message) = refusal_sentence(&LoginRefusal::NotServing {
                    serving: facts.serving,
                });
                return write_error(&mut conn, sequence, number, state, &message).await;
            }
            None => return write_error(&mut conn, sequence, 1053, "08S01", SHUTTING_DOWN).await,
        }
        // A command packet carries at least the command byte; one that carries
        // nothing at all is no command, and the protocol's own "unknown
        // command" is what says so.
        let Some(command) = payload.first().copied() else {
            write_error(&mut conn, sequence, 1047, "08S01", "Unknown command").await?;
            continue;
        };
        let argument = payload.get(1..).unwrap_or_default();
        match command {
            // COM_QUIT.
            0x01 => return Ok(()),
            // COM_INIT_DB.
            0x02 => {
                let named = String::from_utf8_lossy(argument).into_owned();
                if named == session.database() {
                    write_ok(&mut conn, sequence, &session, 0, 0).await?;
                } else {
                    write_error(
                        &mut conn,
                        sequence,
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
                serve_query(&mut conn, sequence, &mut session, &sql, deprecate_eof).await?;
            }
            // COM_PING.
            0x0e => write_ok(&mut conn, sequence, &session, 0, 0).await?,
            // Every other command — the prepared-statement protocol, the
            // replication commands, `COM_CHANGE_USER` — is one this endpoint
            // does not serve, and the protocol has an error for exactly that.
            other => {
                write_error(
                    &mut conn,
                    sequence,
                    1047,
                    "08S01",
                    &format!("Unknown command {other}"),
                )
                .await?;
            }
        }
    }
}

/// Serve one `COM_QUERY`: run it and write what the engine answered.
async fn serve_query(
    conn: &mut ::emulator_server::ListenerConn,
    sequence: u8,
    session: &mut Session,
    sql: &str,
    deprecate_eof: bool,
) -> std::io::Result<()> {
    match session.run(sql).await {
        Err(error) => write_engine_error(conn, sequence, &error).await,
        Ok(answer) if answer.result.columns.is_empty() => {
            write_ok(
                conn,
                sequence,
                session,
                answer.result.affected,
                answer.result.last_insert_id,
            )
            .await
        }
        Ok(answer) => write_result_set(conn, sequence, session, &answer.result, deprecate_eof).await,
    }
}

/// What the client's handshake response said.
struct Login {
    user: String,
    database: Option<String>,
    auth_response: Vec<u8>,
}

/// The capability flags a handshake response declares, which are its first four
/// bytes.
fn client_capabilities(payload: &[u8]) -> u32 {
    u32::from_le_bytes([
        *payload.first().unwrap_or(&0),
        *payload.get(1).unwrap_or(&0),
        *payload.get(2).unwrap_or(&0),
        *payload.get(3).unwrap_or(&0),
    ])
}

/// Read a `HandshakeResponse41`: the user, the authentication response and the
/// database it asked for.
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
/// Random per connection, from the operating system's own entropy, because that
/// is what makes the proof the client answers with non-replayable: the password
/// itself never crosses the wire, only a digest over this nonce.
fn session_nonce() -> std::io::Result<Vec<u8>> {
    let mut bytes = [0u8; NONCE_BYTES];
    ::getrandom::fill(&mut bytes).map_err(|error| std::io::Error::other(error.to_string()))?;
    // The protocol's nonce is a string, so no byte of it may be NUL; each byte
    // is mapped into the printable range the servers themselves use.
    Ok(bytes.iter().map(|byte| 0x21 + (byte % 0x5e)).collect())
}

/// A result set in the text protocol: the column count, one definition per
/// column, the rows, and the packet that ends them.
async fn write_result_set(
    conn: &mut ::emulator_server::ListenerConn,
    sequence: u8,
    session: &Session,
    result: &ResultSet,
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
        write_packet(conn, sequence, &eof_payload(session)).await?;
    }
    for row in &result.rows {
        let mut payload = Vec::new();
        for (index, cell) in row.iter().enumerate() {
            match render_cell(result.columns.get(index), cell) {
                None => payload.push(0xfb),
                Some(value) => put_length_encoded_string(&mut payload, &value),
            }
        }
        sequence = sequence.wrapping_add(1);
        write_packet(conn, sequence, &payload).await?;
    }
    sequence = sequence.wrapping_add(1);
    if deprecate_eof {
        write_packet(conn, sequence, &ok_payload(session, 0, 0, true)).await?;
    } else {
        write_packet(conn, sequence, &eof_payload(session)).await?;
    }
    conn.flush().await
}

/// One column definition packet (`ColumnDefinition41`).
fn column_definition(column: &Column, sample: Option<&Cell>) -> Vec<u8> {
    let (column_type, binary) = column_type(column, sample);
    let mut payload = Vec::new();
    put_length_encoded_string(&mut payload, b"def");
    for part in [b"".as_slice(), b"".as_slice(), b"".as_slice()] {
        put_length_encoded_string(&mut payload, part);
    }
    put_length_encoded_string(&mut payload, column.name.as_bytes());
    put_length_encoded_string(&mut payload, column.name.as_bytes());
    put_length_encoded(&mut payload, 0x0c);
    payload.extend_from_slice(&if binary { COLLATION_BINARY } else { COLLATION_TEXT }.to_le_bytes());
    // The declared maximum display width; the engine keeps none, so the column
    // is reported with the largest value this endpoint serves.
    payload.extend_from_slice(&u32::try_from(MAX_PACKET_BYTES).unwrap_or(u32::MAX).to_le_bytes());
    payload.push(column_type);
    payload.extend_from_slice(&0u16.to_le_bytes());
    payload.push(0);
    payload.extend_from_slice(&[0u8, 0u8]);
    payload
}

/// The MySQL column type a column is reported under, and whether it is binary.
///
/// Source: the type codes in the protocol's `ColumnDefinition41`
/// (`MYSQL_TYPE_*`) and the type names the MySQL reference manual's "Data
/// Types" chapter documents. A column with no declaration — a computed one — is
/// reported under the type of the value the engine holds for it.
fn column_type(column: &Column, sample: Option<&Cell>) -> (u8, bool) {
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
            "blob" | "binary" | "varbinary" => Some((BLOB, true)),
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
/// MySQL's text protocol renders every value as a length-encoded STRING of
/// bytes: a number and a timestamp are written as their text, and a binary
/// value is written as the bytes it holds rather than as text, which is what
/// lets a client read back what it stored. A `TINYINT(1)`/`BOOLEAN` column
/// renders as the `0`/`1` MySQL stores it as.
fn render_cell(column: Option<&Column>, cell: &Cell) -> Option<Vec<u8>> {
    let boolean = column
        .and_then(|column| column.declared_type.as_ref())
        .is_some_and(|declared| {
            let lowered = declared.to_ascii_lowercase();
            lowered == "bool" || lowered == "boolean"
        });
    match cell {
        Cell::Null => None,
        Cell::Integer(value) if boolean => Some(i64::from(*value != 0).to_string().into_bytes()),
        Cell::Integer(value) => Some(value.to_string().into_bytes()),
        Cell::Real(value) => Some(value.to_string().into_bytes()),
        Cell::Text(value) if boolean => Some(
            match value.to_ascii_lowercase().as_str() {
                "0" | "f" | "false" | "n" | "no" | "off" => "0",
                _ => "1",
            }
            .as_bytes()
            .to_vec(),
        ),
        Cell::Text(value) => Some(value.clone().into_bytes()),
        Cell::Blob(bytes) => Some(bytes.clone()),
    }
}

/// The error number and SQLSTATE MySQL refuses a login with, and its own
/// sentence for it.
///
/// Source: the server error numbers and SQLSTATE values in the MySQL reference
/// manual's "Server Error Message Reference", which is what a client branches
/// on.
fn refusal_sentence(refusal: &LoginRefusal) -> (u16, &'static str, String) {
    match refusal {
        // `ER_SERVER_SHUTDOWN` for an engine that is going down, and
        // `ER_SERVER_ISNT_AVAILABLE` for one that has not finished coming up:
        // two conditions, and the protocol has an error for each.
        LoginRefusal::NotServing { serving } => match serving {
            ServingState::ComingUp => (3168, "HY000", NOT_AVAILABLE.to_string()),
            _ => (1053, "08S01", SHUTTING_DOWN.to_string()),
        },
        // `ER_BAD_DB_ERROR`.
        LoginRefusal::UnknownDatabase { database } => {
            (1049, "42000", format!("Unknown database '{database}'"))
        }
        // `ER_ACCESS_DENIED_ERROR`, whose message names the account the login
        // was for and whether a password was sent at all.
        LoginRefusal::AuthenticationFailed {
            user,
            password_given,
        } => (
            ACCESS_DENIED_ERROR,
            ACCESS_DENIED_STATE,
            format!(
                "Access denied for user '{user}'@'{}' (using password: {})",
                client_host(),
                if *password_given { "YES" } else { "NO" }
            ),
        ),
    }
}

/// `ER_ACCESS_DENIED_ERROR` and its SQLSTATE: the error a MySQL server refuses a
/// credential it does not accept with, whoever judged the credential.
///
/// Source: the MySQL reference manual's "Server Error Message Reference".
const ACCESS_DENIED_ERROR: u16 = 1045;
/// See [`ACCESS_DENIED_ERROR`].
const ACCESS_DENIED_STATE: &str = "28000";

/// The error number and SQLSTATE a failed statement is reported under.
///
/// Source: the server error numbers and SQLSTATE values in the MySQL reference
/// manual's "Server Error Message Reference".
fn statement_error(kind: EngineErrorKind) -> (u16, &'static str) {
    match kind {
        EngineErrorKind::Syntax => (1064, "42000"),
        EngineErrorKind::UndefinedObject => (1146, "42S02"),
        // `ER_PARSE_ERROR`: a type this server does not have is a declaration it
        // cannot read, which is how MySQL reports one.
        EngineErrorKind::UndefinedType => (1064, "42000"),
        // `ER_TABLE_EXISTS_ERROR`: an object of that name is already there.
        EngineErrorKind::DuplicateObject => (1050, "42S01"),
        EngineErrorKind::UniqueViolation => (1062, "23000"),
        EngineErrorKind::NotNullViolation => (1048, "23000"),
        EngineErrorKind::CheckViolation => (3819, "HY000"),
        EngineErrorKind::ForeignKeyViolation => (1452, "23000"),
        EngineErrorKind::DataError => (1366, "HY000"),
        // `ER_QUERY_INTERRUPTED`: the server stopped the statement.
        EngineErrorKind::Cancelled => (1317, "70100"),
        // `ER_UNKNOWN_ERROR`: the statement asked for more than this endpoint
        // serves, which is not one of the server's own numbered conditions.
        EngineErrorKind::LimitExceeded | EngineErrorKind::Internal => (1105, "HY000"),
        // `ER_NOT_SUPPORTED_YET`.
        EngineErrorKind::FeatureNotSupported => (1235, "42000"),
        EngineErrorKind::InsufficientPrivilege => (1227, "42000"),
        // `ER_LOCK_DEADLOCK`, whose SQLSTATE is the standard's serialization
        // failure: the condition a transaction that cannot stand beside the ones
        // that already have is rolled back under, and the one a client retries.
        EngineErrorKind::SerializationFailure => (1213, "40001"),
    }
}

/// Report a failed statement, keeping the engine's own message.
async fn write_engine_error(
    conn: &mut ::emulator_server::ListenerConn,
    sequence: u8,
    error: &EngineError,
) -> std::io::Result<()> {
    let (number, state) = statement_error(error.kind);
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
    session: &Session,
    affected: u64,
    last_insert_id: i64,
) -> std::io::Result<()> {
    let payload = ok_payload(session, affected, last_insert_id, false);
    write_packet(conn, sequence, &payload).await?;
    conn.flush().await
}

/// The body of an `OK` packet, which also ends a result set for a client that
/// asked for `CLIENT_DEPRECATE_EOF`.
fn ok_payload(
    session: &Session,
    affected: u64,
    last_insert_id: i64,
    ends_result_set: bool,
) -> Vec<u8> {
    let mut payload = vec![if ends_result_set { 0xfe } else { 0x00 }];
    put_length_encoded(&mut payload, affected);
    put_length_encoded(&mut payload, last_insert_id.max(0).unsigned_abs());
    payload.extend_from_slice(&status_flags(session).to_le_bytes());
    payload.extend_from_slice(&0u16.to_le_bytes());
    payload
}

/// One `EOF` packet, for a client that did not ask for
/// `CLIENT_DEPRECATE_EOF`.
fn eof_payload(session: &Session) -> Vec<u8> {
    let mut payload = vec![0xfeu8];
    payload.extend_from_slice(&0u16.to_le_bytes());
    payload.extend_from_slice(&status_flags(session).to_le_bytes());
    payload
}

/// The session's status flags: autocommit, and whether a transaction is open.
fn status_flags(session: &Session) -> u16 {
    let mut flags = 0u16;
    if session.autocommit() {
        flags |= SERVER_STATUS_AUTOCOMMIT;
    }
    if session.in_transaction() {
        flags |= SERVER_STATUS_IN_TRANS;
    }
    flags
}

/// Write one packet: its length, its sequence id and its payload — as ONE write.
///
/// A client reads a packet's four-byte header before it reads the payload, so a write
/// that carries only the header leaves it waiting for a second segment the kernel holds
/// back until the first is acknowledged (Nagle against the client's delayed
/// acknowledgement), which adds tens of milliseconds to every packet.
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
    let mut packet = Vec::with_capacity(payload.len() + 4);
    packet.extend_from_slice(length.get(..3).unwrap_or_default());
    packet.push(sequence);
    packet.extend_from_slice(payload);
    conn.write_all(&packet).await
}

/// Read one packet: its sequence id and its payload, or `None` at end of
/// stream.
async fn read_packet(
    conn: &mut ::emulator_server::ListenerConn,
) -> std::io::Result<Option<(u8, Vec<u8>)>> {
    let mut header = [0u8; 4];
    match conn.read_exact(&mut header).await {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let length =
        usize::from(header[0]) | (usize::from(header[1]) << 8) | (usize::from(header[2]) << 16);
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
            payload.extend_from_slice(&u16::try_from(value).unwrap_or(u16::MAX).to_le_bytes());
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
