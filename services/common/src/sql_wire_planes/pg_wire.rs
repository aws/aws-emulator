// SPDX-License-Identifier: CC0-1.0

//! The server half of the PostgreSQL frontend/backend protocol (version 3.0),
//! which is what the endpoint of a PostgreSQL-compatible database resource
//! answers.
//!
//! A stock client (psql, libpq, psycopg, a JDBC driver) connects to the
//! published address and port and speaks this. What it gets is this emulator's
//! own state: the login is admitted against the stored resource and the
//! credentials its control plane issued for it ([`super::admit_issued`]), the
//! parameters the backend reports carry the engine
//! version the resource publishes, and every statement is run by that
//! database's own SQL engine ([`super::sql_engine`]).
//!
//! The order of a connection, which is the protocol's and not this emulator's:
//!
//! 1. the client opens in the CLEAR and may send an `SSLRequest` as its first
//!    message; the server answers `S` and the core's TLS upgrader runs the
//!    handshake, after which the startup arrives over the upgraded stream;
//! 2. the `StartupMessage` names the user and the database; the server asks for
//!    the password (`AuthenticationCleartextPassword`), checks it against the
//!    stored master password and the credentials the control plane issued — or,
//!    for a resource whose service authenticates a PRESENTED credential, against
//!    what that service says of it ([`super::ServedDatabase::verified_login`]) —
//!    and answers `AuthenticationOk`, the backend
//!    parameters, `BackendKeyData` and `ReadyForQuery` — or one
//!    `ErrorResponse` carrying the SQLSTATE of the refusal, and closes;
//! 3. the simple query protocol: `Query` is run and answered with
//!    `RowDescription`/`DataRow`*/`CommandComplete`/`ReadyForQuery`, and
//!    `Terminate` ends the session. A message this server does not serve — the
//!    extended query protocol's `Parse`/`Bind`, `COPY` — is answered with an
//!    honest `ErrorResponse`, never with a fabricated success.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::session::{Answer, Session};
use super::sql_engine::{Cell, Column, EngineError, EngineErrorKind};
use super::{
    admit_login, EngineWire, LoginDenial, LoginRefusal, PasswordProof, PresentedLogin, Refusal,
    ServingState, SessionContext,
};

/// The protocol version this server speaks, as a startup message declares it
/// (3.0, `major << 16 | minor`).
const PROTOCOL_VERSION_3: i32 = 196_608;

/// The startup requests that are not a startup message: the TLS negotiation,
/// the GSSAPI one and a cancel request.
const SSL_REQUEST_CODE: i32 = 80_877_103;
const GSSENC_REQUEST_CODE: i32 = 80_877_104;
const CANCEL_REQUEST_CODE: i32 = 80_877_102;

/// How many bytes one message may carry, so a length prefix cannot make this
/// endpoint allocate without bound.
const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

/// The PostgreSQL wire handler for one database's listener.
pub struct PostgresWire {
    context: SessionContext,
    /// Which server answers behind this protocol: a PostgreSQL server, or an
    /// Amazon Redshift warehouse, which speaks the same protocol and reports its
    /// own identity and its own SQLSTATEs.
    wire: EngineWire,
}

impl PostgresWire {
    /// The handler for the database `context` names, serving `wire`.
    #[must_use]
    pub fn new(context: SessionContext, wire: EngineWire) -> Self {
        Self { context, wire }
    }
}

impl ::emulator_server::NativeHandler for PostgresWire {
    fn serve(
        &self,
        conn: ::emulator_server::ListenerConn,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let context = self.context.clone();
        let wire = self.wire;
        Box::pin(async move {
            // An I/O error is the client's connection going away, which ends
            // the session and nothing else.
            let _ = serve_session(context, wire, conn).await;
        })
    }
}

/// One connection, from its first message to its last.
async fn serve_session(
    context: SessionContext,
    wire: EngineWire,
    conn: ::emulator_server::ListenerConn,
) -> std::io::Result<()> {
    let mut conn = conn;
    let mut encrypted = false;

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
                SSL_REQUEST_CODE if !encrypted => match context.upgrader() {
                    Some(upgrader) => {
                        conn.write_all(b"S").await?;
                        conn.flush().await?;
                        let (upgraded, _alpn) = upgrader.accept(conn, None).await?;
                        conn = upgraded;
                        encrypted = true;
                        continue;
                    }
                    // `N` is the protocol's "not this one": this emulator has no
                    // TLS prepared, and the client carries on in the clear or
                    // closes, which is its own choice.
                    None => {
                        conn.write_all(b"N").await?;
                        conn.flush().await?;
                        continue;
                    }
                },
                // This server serves no GSSAPI encryption.
                GSSENC_REQUEST_CODE => {
                    conn.write_all(b"N").await?;
                    conn.flush().await?;
                    continue;
                }
                // A cancel request is sent on its own connection and names a
                // backend to cancel; this server runs each statement to
                // completion under its own deadline, so there is nothing to
                // cancel and the connection closes, as the protocol says it
                // does.
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
                &format!("unsupported frontend protocol {major}.{minor}: server supports 3.0"),
            )
            .await;
        }
        break startup_parameters(body.get(4..).unwrap_or_default());
    };

    // 2. The resource this listener belongs to, as the store holds it now.
    let Some(facts) = context.facts() else {
        return fatal(&mut conn, "57P03", "the database system is shutting down").await;
    };
    let named = |key: &str| -> Option<String> {
        startup
            .iter()
            .find(|(held, _)| held == key)
            .map(|(_, value)| value.clone())
    };
    let user = named("user").unwrap_or_default();
    let database = named("database");

    // 3. The password, then the admission decision.
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
    let proof = PasswordProof::Cleartext(&password);
    let presented = PresentedLogin {
        user: &user,
        database: database.as_deref(),
        proof: &proof,
        encrypted,
    };
    let database = match admit_login(&context, &facts, &presented) {
        Ok(database) => database,
        // A credential the owning service verified itself and refused: its own
        // words, under its own SQLSTATE, with the hint it states beside them.
        Err(LoginDenial::Stated(stated)) => {
            return hinted_fatal(
                &mut conn,
                stated.sqlstate,
                &stated.message,
                stated.hint.as_deref(),
            )
            .await
        }
        Err(LoginDenial::Wire(refusal)) => {
            // How the engine's own product refuses a login of its own
            // credentials, where it has its own words for it; the wire's
            // otherwise.
            let (code, message) = context
                .dialect()
                .and_then(|dialect| dialect.login_refusal(&refusal, &user))
                .unwrap_or_else(|| refusal_sentence(&refusal, &user));
            return fatal(&mut conn, code, &message).await;
        }
    };

    // 4. The session is open.
    let mut session = Session::new(
        Arc::clone(context.engine()),
        wire,
        facts,
        user.clone(),
        database,
        context.take_connection_id(),
    )
    .with_dialect(context.dialect())
    .with_optimistic(context.optimistic());
    send(&mut conn, b'R', &0i32.to_be_bytes()).await?;
    for (name, value) in [
        ("server_version", session.server_version().to_string()),
        ("server_encoding", "UTF8".to_string()),
        ("client_encoding", "UTF8".to_string()),
        ("DateStyle", "ISO, MDY".to_string()),
        ("TimeZone", "UTC".to_string()),
        ("integer_datetimes", "on".to_string()),
        ("standard_conforming_strings", "on".to_string()),
        ("is_superuser", "off".to_string()),
        ("session_authorization", session.user().to_string()),
    ] {
        let mut body = Vec::new();
        put_cstring(&mut body, name);
        put_cstring(&mut body, &value);
        send(&mut conn, b'S', &body).await?;
    }
    let mut key_data = Vec::new();
    let backend = i32::try_from(session.connection_id()).unwrap_or(i32::MAX);
    key_data.extend_from_slice(&backend.to_be_bytes());
    // The cancel key: this session's own id, which is all this server has to
    // identify it by, since it cancels nothing (see the cancel request above).
    key_data.extend_from_slice(&backend.to_be_bytes());
    send(&mut conn, b'K', &key_data).await?;
    ready(&mut conn, &session, false).await?;

    // 5. The simple query protocol.
    let mut failed_in_transaction = false;
    loop {
        let Some((kind, body)) = read_message(&mut conn).await? else {
            return Ok(());
        };
        // The resource is read again for every message: one that has been
        // stopped or deleted under this session ends it, which is what a client
        // sees when the backend terminates.
        match context.facts() {
            Some(facts) if facts.is_serving() => session.refresh(facts),
            _ => {
                return fatal(
                    &mut conn,
                    "57P01",
                    "terminating connection due to administrator command",
                )
                .await
            }
        }
        match kind {
            // Terminate.
            b'X' => return Ok(()),
            // Simple query.
            b'Q' => {
                let (sql, _) = cstring(&body, 0);
                match session.run(&sql).await {
                    Ok(answer) => {
                        write_answer(&mut conn, &answer).await?;
                        failed_in_transaction = false;
                    }
                    Err(error) => {
                        engine_error_response(
                            &mut conn,
                            wire,
                            &error,
                            session.dialect_refusal(&error),
                        )
                        .await?;
                        failed_in_transaction = session.in_transaction();
                    }
                }
                ready(&mut conn, &session, failed_in_transaction).await?;
            }
            // A `Flush` or a `Sync` outside the extended protocol is answered
            // the way the protocol says: nothing to flush, and a `Sync` ends
            // the implicit transaction block with a `ReadyForQuery`.
            b'H' => conn.flush().await?,
            b'S' => ready(&mut conn, &session, failed_in_transaction).await?,
            // The extended query protocol and `COPY`, which this endpoint does
            // not serve: the protocol's own "feature not supported", never a
            // fabricated answer.
            other => {
                error_response(
                    &mut conn,
                    "ERROR",
                    "0A000",
                    &format!(
                        "this endpoint serves the simple query protocol; message type '{}' is not \
                         supported",
                        char::from(other)
                    ),
                )
                .await?;
                ready(&mut conn, &session, failed_in_transaction).await?;
            }
        }
    }
}

/// Write what a statement answered: its rows, where it produced any, and the
/// tag that completes it.
async fn write_answer(
    conn: &mut ::emulator_server::ListenerConn,
    answer: &Answer,
) -> std::io::Result<()> {
    if answer.verb.is_empty() {
        send(conn, b'I', &[]).await?;
        return conn.flush().await;
    }
    if !answer.result.columns.is_empty() {
        let empty: Vec<Cell> = Vec::new();
        let sample = answer.result.rows.first().unwrap_or(&empty);
        row_description(conn, &answer.result.columns, sample).await?;
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

/// One `RowDescription`: the columns a statement produced, each under the type
/// a client decodes it with.
async fn row_description(
    conn: &mut ::emulator_server::ListenerConn,
    columns: &[Column],
    sample: &[Cell],
) -> std::io::Result<()> {
    let mut body = Vec::new();
    let count = i16::try_from(columns.len()).unwrap_or(i16::MAX);
    body.extend_from_slice(&count.to_be_bytes());
    for (index, column) in columns.iter().enumerate() {
        put_cstring(&mut body, &column.name);
        // No table and no column number: the value is this server's, and the
        // client reads the column by name and position.
        body.extend_from_slice(&0i32.to_be_bytes());
        body.extend_from_slice(&0i16.to_be_bytes());
        body.extend_from_slice(&type_oid(column, sample.get(index)).to_be_bytes());
        body.extend_from_slice(&(-1i16).to_be_bytes());
        body.extend_from_slice(&(-1i32).to_be_bytes());
        body.extend_from_slice(&0i16.to_be_bytes());
    }
    send(conn, b'T', &body).await
}

/// The `CommandComplete` tag: the statement's verb and the count PostgreSQL
/// reports with it ("the number of rows retrieved" for a SELECT, the number
/// changed for a write, and an `oid` field of 0 for an INSERT).
fn command_tag(answer: &Answer) -> String {
    match answer.verb.as_str() {
        "SELECT" | "SHOW" => format!("SELECT {}", answer.result.rows.len()),
        "INSERT" => format!("INSERT 0 {}", answer.result.affected),
        "UPDATE" | "DELETE" => format!("{} {}", answer.verb, answer.result.affected),
        other => other.to_string(),
    }
}

/// The PostgreSQL type OID a column is reported under: the type its DECLARATION
/// names, which is what a client decodes the value with, and the type of the
/// value the engine holds where the column has no declaration (a computed
/// column).
///
/// Source: the type names PostgreSQL's "Data Types" chapter documents (with the
/// aliases it lists for each) and the OIDs of those types in `pg_type`, which
/// is what the wire carries.
fn type_oid(column: &Column, sample: Option<&Cell>) -> i32 {
    if let Some(declared) = &column.declared_type {
        let lowered = declared.to_ascii_lowercase();
        let base = lowered.split('(').next().unwrap_or(&lowered).trim();
        let oid = match base {
            "bool" | "boolean" => Some(16),
            "bytea" | "blob" => Some(17),
            "char" | "character" | "bpchar" => Some(1042),
            "name" => Some(19),
            "int8" | "bigint" | "bigserial" => Some(20),
            "int2" | "smallint" | "smallserial" => Some(21),
            "int4" | "int" | "integer" | "serial" => Some(23),
            "text" | "citext" => Some(25),
            "json" => Some(114),
            "float4" | "real" => Some(700),
            "float8" | "double" | "double precision" => Some(701),
            "varchar" | "character varying" => Some(1043),
            "date" => Some(1082),
            "time" | "time without time zone" => Some(1083),
            "timestamp" | "timestamp without time zone" => Some(1114),
            "timestamptz" | "timestamp with time zone" => Some(1184),
            "numeric" | "decimal" => Some(1700),
            "uuid" => Some(2950),
            "jsonb" => Some(3802),
            _ => None,
        };
        if let Some(oid) = oid {
            return oid;
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
fn render_cell(column: Option<&Column>, cell: &Cell) -> Option<String> {
    let boolean = column
        .and_then(|column| column.declared_type.as_ref())
        .is_some_and(|declared| {
            let lowered = declared.to_ascii_lowercase();
            lowered == "bool" || lowered == "boolean"
        });
    match cell {
        Cell::Null => None,
        // PostgreSQL renders a boolean as `t`/`f`, which is what a client
        // decodes back to true and false.
        Cell::Integer(value) if boolean => Some(if *value != 0 { "t" } else { "f" }.to_string()),
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
        // A `bytea` is written in the hex format PostgreSQL has used since 9.0.
        Cell::Blob(bytes) => Some(format!("\\x{}", ::hex::encode(bytes))),
    }
}

/// The SQLSTATE and the sentence PostgreSQL refuses a login with.
///
/// Source: the codes in PostgreSQL's "Appendix A. Error Codes", which is what a
/// client branches on.
fn refusal_sentence(refusal: &LoginRefusal, user: &str) -> (&'static str, String) {
    match refusal {
        // `cannot_connect_now`, whose two sentences are the two conditions: an
        // engine that has not finished coming up, and one that is going down.
        LoginRefusal::NotServing { serving } => match serving {
            ServingState::ComingUp => (
                "57P03",
                "the database system is starting up".to_string(),
            ),
            _ => (
                "57P03",
                "the database system is shutting down".to_string(),
            ),
        },
        // `invalid_catalog_name`.
        LoginRefusal::UnknownDatabase { database } => (
            "3D000",
            format!("database \"{database}\" does not exist"),
        ),
        // `invalid_password`, whose message names the account the login was
        // for.
        LoginRefusal::AuthenticationFailed { .. } => (
            "28P01",
            format!("password authentication failed for user \"{user}\""),
        ),
    }
}

/// The SQLSTATE a failed statement is reported under.
///
/// PostgreSQL's "Appendix A. Error Codes" names a condition per class, and a
/// PostgreSQL server answers that condition's own code. An Amazon Redshift
/// warehouse does not for the one constraint class it enforces ITSELF: "Amazon
/// Redshift does enforce NOT NULL column constraints", while "uniqueness,
/// primary key, and foreign key constraints are informational only" (the Amazon
/// Redshift Database Developer Guide, "Defining table constraints"), and a row
/// refused for a `NOT NULL` column reaches the client under the warehouse's
/// internal-error code `XX000` rather than PostgreSQL's integrity-constraint
/// class — which is what a client of a Redshift endpoint branches on. Every
/// other class — the object a statement named, the text it could not parse, the
/// statement a caller cancelled — is the condition the protocol itself names, on
/// both.
fn sqlstate(wire: EngineWire, kind: EngineErrorKind) -> &'static str {
    if wire == EngineWire::Redshift && kind == EngineErrorKind::NotNullViolation {
        return "XX000";
    }
    match kind {
        EngineErrorKind::Syntax => "42601",
        EngineErrorKind::UndefinedObject => "42P01",
        // `undefined_object`: the TYPE a declaration named is not one this
        // engine has, which is not the same condition as a relation that is not
        // there.
        EngineErrorKind::UndefinedType => "42704",
        // `duplicate_object`: an object of that name is already there.
        EngineErrorKind::DuplicateObject => "42710",
        EngineErrorKind::UniqueViolation => "23505",
        EngineErrorKind::NotNullViolation => "23502",
        EngineErrorKind::CheckViolation => "23514",
        EngineErrorKind::ForeignKeyViolation => "23503",
        EngineErrorKind::DataError => "22000",
        EngineErrorKind::Cancelled => "57014",
        EngineErrorKind::LimitExceeded => "54000",
        EngineErrorKind::FeatureNotSupported => "0A000",
        EngineErrorKind::InsufficientPrivilege => "42501",
        // `serialization_failure`: the class PostgreSQL's "Appendix A. Error
        // Codes" names for a transaction that cannot be allowed to stand beside
        // the ones that already have, which is what an engine resolving two
        // writers optimistically refuses the losing commit under.
        EngineErrorKind::SerializationFailure => "40001",
        EngineErrorKind::Internal => "XX000",
    }
}

/// Report a failed statement, keeping the engine's own message.
///
/// `refusal` is how the engine's own product reports the failure — its
/// `SQLSTATE`, its wording and what it says in the detail field beside it —
/// where that differs from what the engine itself said
/// ([`super::SqlDialect::refusal`]).
async fn engine_error_response(
    conn: &mut ::emulator_server::ListenerConn,
    wire: EngineWire,
    error: &EngineError,
    refusal: Option<Refusal>,
) -> std::io::Result<()> {
    let (code, message, detail) = match refusal {
        Some(refusal) => (refusal.sqlstate, refusal.message, refusal.detail),
        None => (sqlstate(wire, error.kind), error.message.clone(), None),
    };
    reported(conn, "ERROR", code, &message, detail.as_deref(), None).await
}

/// One `ErrorResponse`.
async fn error_response(
    conn: &mut ::emulator_server::ListenerConn,
    severity: &str,
    code: &str,
    message: &str,
) -> std::io::Result<()> {
    reported(conn, severity, code, message, None, None).await
}

/// One `ErrorResponse`, with the `Detail` and `Hint` fields where the refusal
/// carries them.
async fn reported(
    conn: &mut ::emulator_server::ListenerConn,
    severity: &str,
    code: &str,
    message: &str,
    detail: Option<&str>,
    hint: Option<&str>,
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
    if let Some(detail) = detail {
        body.push(b'D');
        put_cstring(&mut body, detail);
    }
    if let Some(hint) = hint {
        body.push(b'H');
        put_cstring(&mut body, hint);
    }
    body.push(0);
    send(conn, b'E', &body).await?;
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

/// Report a refusal that ends the connection and carries a `Hint` beside its
/// message — the field the protocol puts a refusal's second sentence in.
async fn hinted_fatal(
    conn: &mut ::emulator_server::ListenerConn,
    code: &str,
    message: &str,
    hint: Option<&str>,
) -> std::io::Result<()> {
    reported(conn, "FATAL", code, message, None, hint).await
}

/// `ReadyForQuery` with the transaction status this session is in.
async fn ready(
    conn: &mut ::emulator_server::ListenerConn,
    session: &Session,
    failed: bool,
) -> std::io::Result<()> {
    let status = match (session.in_transaction(), failed) {
        (true, true) => b'E',
        (true, false) => b'T',
        (false, _) => b'I',
    };
    send(conn, b'Z', &[status]).await?;
    conn.flush().await
}

/// Write one message: its type byte, its length (counting the length itself)
/// and its body — as ONE write.
///
/// A client reads a message's type and length before it reads the body, so a write
/// that carries only the header leaves it waiting for a second segment the kernel
/// holds back until the first is acknowledged (Nagle against the client's delayed
/// acknowledgement), which adds tens of milliseconds to every message.
async fn send(
    conn: &mut ::emulator_server::ListenerConn,
    kind: u8,
    body: &[u8],
) -> std::io::Result<()> {
    let length = i32::try_from(body.len() + 4).unwrap_or(i32::MAX);
    let mut message = Vec::with_capacity(body.len() + 5);
    message.push(kind);
    message.extend_from_slice(&length.to_be_bytes());
    message.extend_from_slice(body);
    conn.write_all(&message).await
}

/// Read the next message: its type byte and its body, or `None` at end of
/// stream.
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
    let body_length = message_body_length(i32::from_be_bytes(length))?;
    let mut body = vec![0u8; body_length];
    conn.read_exact(&mut body).await?;
    Ok(Some((kind[0], body)))
}

/// Read a startup message or request: its length and the body that follows it.
/// The startup messages are the only ones with no type byte.
async fn read_startup(conn: &mut ::emulator_server::ListenerConn) -> std::io::Result<Vec<u8>> {
    let mut length = [0u8; 4];
    conn.read_exact(&mut length).await?;
    let body_length = message_body_length(i32::from_be_bytes(length))?;
    let mut body = vec![0u8; body_length];
    conn.read_exact(&mut body).await?;
    Ok(body)
}

/// The body length a declared message length means, refused when the protocol
/// does not allow it or it is larger than this endpoint serves.
fn message_body_length(declared: i32) -> std::io::Result<usize> {
    let body = declared
        .checked_sub(4)
        .and_then(|body| usize::try_from(body).ok())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "the message declared a length the protocol does not allow",
            )
        })?;
    if body > MAX_MESSAGE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "the message is larger than this endpoint serves",
        ));
    }
    Ok(body)
}

/// The key/value pairs a startup message carries, each NUL-terminated and the
/// list ended by an empty key.
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
