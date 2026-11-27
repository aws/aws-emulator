// SPDX-License-Identifier: CC0-1.0

//! The COMMANDS one session on a cluster's endpoint may send, and what each is answered
//! with.
//!
//! A client of an Amazon DocumentDB cluster speaks the engine's own wire protocol: every
//! request is a command DOCUMENT and every answer a reply document ([`super::wire`] carries
//! them). This module is the server half of that conversation — it decides what a session may
//! do and answers out of the cluster's own documents ([`super::documents`]) — and the
//! [`Session`] here is what one connection holds while it is open: who it has authenticated
//! as, the SASL exchange it is in the middle of, and the cursors it has open.
//!
//! Every fact a command is judged by is read from the control-plane STORE on each command
//! rather than captured when the connection was accepted ([`Session::facts`]), so a change
//! the control plane makes while a session is open — a failover moving the writer role, the
//! instance deleted — is in force on the next command that session sends.
//!
//! A command this endpoint does not serve is answered with the engine's own
//! `CommandNotFound` ([`super::fault::command_not_found`]), never with an empty success: a
//! client must be able to tell a command that did nothing from one this emulator does not
//! have.

use std::collections::BTreeMap;

use ::bson::{Bson, Document};

use super::auth::{self, ScramCredentials, ScramExchange};
use super::documents::{self, IndexSpec};
use super::fault::{self, WireFault};
use super::query;

/// How many documents a batch carries when the client named no `batchSize`, which is the
/// engine's own default first batch.
const DEFAULT_BATCH_SIZE: usize = 101;

/// How many cursors one connection may hold open. CHOSEN as a process bound: each one holds
/// the documents it has not yet handed over.
const MAX_OPEN_CURSORS: usize = 64;

/// How many stages one aggregation pipeline may carry. CHOSEN as a process bound on the
/// PRODUCT a pipeline drives: each stage walks the documents the stages before it produced,
/// which is itself capped ([`documents::MAX_DOCUMENTS_PER_COLLECTION`]), so this is what
/// bounds stages times documents.
const MAX_PIPELINE_STAGES: usize = 32;

/// How many documents one PIPELINE may visit: the stages it carries times the accumulators the
/// widest of them names times the documents it reads, each of which every stage walks.
///
/// CHOSEN as a process bound on that PRODUCT, which no one factor bounds on its own — the
/// same reading the write path applies ([`MAX_WRITE_DOCUMENT_VISITS`]) — and sized so a
/// pipeline that reaches it is one no client sends rather than one a client has to split.
const MAX_PIPELINE_DOCUMENT_VISITS: usize = 20_000_000;

/// How many accumulator fields one `$group` stage may carry. CHOSEN as a process bound, for
/// the same reason [`MAX_PIPELINE_STAGES`] exists: each accumulator is evaluated once per
/// document of every group the stage forms, so the fields of one client-written stage document
/// multiply the documents the pipeline reads.
const MAX_GROUP_ACCUMULATORS: usize = 32;

/// How many values ONE accumulator may retain — the values `$push` keeps and the distinct ones
/// `$addToSet` keeps.
///
/// CHOSEN as a process bound on what one group's answer holds: a group is formed from the
/// documents the pipeline read, which is itself capped
/// ([`documents::MAX_DOCUMENTS_PER_COLLECTION`]), so this is the ceiling that keeps an
/// accumulator from outgrowing the documents it accumulated.
const MAX_ACCUMULATED_VALUES: usize = documents::MAX_DOCUMENTS_PER_COLLECTION;

/// How many BYTES one pipeline command may MATERIALISE: the values its stages resolve and hold
/// while they run, as opposed to how many of them there are.
///
/// CHOSEN as a process bound on that product, which the counting bounds do not reach: an
/// accumulator's argument is an arbitrary client expression, and resolving one builds a value
/// of about its own size for every member of every group, so a command carrying a large
/// expression would ask for memory in proportion to the documents it visits however few values
/// that is. Sized at the ceiling the engine publishes for ONE document
/// ([`documents::MAX_BSON_OBJECT_SIZE`]), so one command may build at most as much as one
/// document may hold — reached by nothing a client sends and by anything that would exhaust
/// this process.
const MAX_PIPELINE_MATERIALISED_BYTES: usize = documents::MAX_BSON_OBJECT_SIZE as usize;

/// How many documents one WRITE command may visit: the statements it carries times the
/// documents the collection holds, each of which it walks.
///
/// CHOSEN as a process bound on that PRODUCT, which neither factor bounds on its own — the
/// same reason [`MAX_PIPELINE_STAGES`] exists — and sized so a command that reaches it is one
/// no client sends rather than one a client has to split.
const MAX_WRITE_DOCUMENT_VISITS: usize = 20_000_000;

/// The largest message this endpoint accepts, which it publishes as `maxMessageSizeBytes`:
/// the engine's own ceiling of 48 MB.
pub const MAX_MESSAGE_SIZE_BYTES: i32 = 48_000_000;

/// How many documents one write command may carry, which the endpoint publishes as
/// `maxWriteBatchSize`: the engine's own ceiling.
pub const MAX_WRITE_BATCH_SIZE: i32 = 100_000;

/// The database a client authenticates against and sends server commands to.
pub const ADMIN_DATABASE: &str = "admin";

/// The facts of the endpoint a session is on, read from the control-plane store on every
/// command.
#[derive(Debug, Clone)]
pub struct EndpointFacts {
    /// Whether what this endpoint leads to — its instance, or the cluster whose primary a
    /// cluster endpoint leads to — is still in state.
    pub exists: bool,
    /// Whether that resource is SERVING: one still being created has no engine to answer out
    /// of yet, so only the handshake reaches it.
    pub serving: bool,
    /// The status a refusal names so a client can tell why it is not served: `DBInstanceStatus`
    /// for a member's endpoint, and for a cluster's own the status of the member it writes
    /// through — the cluster's own while it has no member to lead to.
    pub status: String,
    /// Whether writes are served here: a member that is not the one its cluster writes through
    /// serves reads only, as a replica does, while a cluster's own endpoint leads to its
    /// primary.
    pub is_writer: bool,
    /// The cluster's `EngineVersion`, which decides the wire version the handshake reports.
    pub engine_version: String,
    /// The master user's SCRAM credentials, or `None` for a cluster that recorded none — whose
    /// endpoints then admit no session at all, because there is nothing a client could
    /// authenticate against and a command outside the handshake is served only to a session
    /// that has authenticated.
    pub credentials: Option<ScramCredentials>,
}

/// One cursor a session holds open: the documents of one read it has not yet handed over.
#[derive(Debug)]
struct Cursor {
    namespace: String,
    remaining: std::collections::VecDeque<Document>,
}

/// What ONE connection on a cluster endpoint holds while it is open.
pub struct Session {
    /// The id this connection reports as its `connectionId`.
    connection_id: i64,
    /// Who the session has authenticated as, or `None` for one that has not.
    authenticated: Option<String>,
    /// The SASL exchange in progress, if any.
    exchange: Option<ScramExchange>,
    /// The cursors this connection holds, by id.
    cursors: BTreeMap<i64, Cursor>,
    /// The id of the next cursor this connection mints.
    next_cursor_id: i64,
    /// Whether the client asked, when it began the exchange, for the closing empty message to
    /// be skipped (`options.skipEmptyExchange`), which is what a driver asks for.
    skip_empty_exchange: bool,
    /// Whether the client said it understands the `hello` form of the handshake.
    hello_ok: bool,
}

impl std::fmt::Debug for Session {
    /// Everything about a session but the credential material of the exchange it is running.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Session")
            .field("connection_id", &self.connection_id)
            .field("authenticated", &self.authenticated)
            .field("open_cursors", &self.cursors.len())
            .finish_non_exhaustive()
    }
}

impl Session {
    /// A fresh session on a connection whose `connectionId` is `connection_id`.
    #[must_use]
    pub fn new(connection_id: i64) -> Self {
        Self {
            connection_id,
            authenticated: None,
            exchange: None,
            cursors: BTreeMap::new(),
            next_cursor_id: 1,
            skip_empty_exchange: false,
            hello_ok: false,
        }
    }

    /// A session on a connection an endpoint's OWN authentication layer has already admitted
    /// as `username`.
    ///
    /// What it is for is an endpoint of this wire that runs the SASL exchange itself because
    /// its service words the refusals and offers the mechanisms differently
    /// (`wire::WireEndpoint`): the session it hands the command layer is one that has
    /// authenticated, so the gate below is the same single rule for both — a command outside
    /// the handshake is served only to a session that presented the endpoint's credential, and
    /// no caller can produce one by any other route.
    #[must_use]
    pub fn authenticated_as(connection_id: i64, username: &str) -> Self {
        Self {
            authenticated: Some(username.to_string()),
            ..Self::new(connection_id)
        }
    }
}

/// Answer one command, as the reply document the client reads.
///
/// `database` is the `$db` the command names (the legacy form carries it in the collection
/// name), `facts` the endpoint as it is NOW, and `volume` the cluster's documents.
pub fn execute(
    session: &mut Session,
    facts: &EndpointFacts,
    volume: &mut documents::Volume,
    database: &str,
    command: &Document,
) -> Document {
    let Some((name, argument)) = command.iter().next() else {
        return fault::failed_to_parse("the command document is empty").to_reply();
    };
    match run(session, facts, volume, database, name, argument, command) {
        Ok(mut reply) => {
            // Every reply carries `ok`, which is what a client reads first.
            if !reply.contains_key("ok") {
                reply.insert("ok".to_string(), Bson::Double(1.0));
            }
            reply
        }
        Err(refusal) => refusal.to_reply(),
    }
}

/// The commands that need no authentication, whoever the session is: the handshake, the SASL
/// exchange itself and the liveness check, which is what a driver sends before it has
/// authenticated anything.
fn served_unauthenticated(name: &str) -> bool {
    matches!(
        name,
        "hello" | "ismaster" | "isMaster" | "ping" | "saslStart" | "saslContinue" | "buildInfo"
            | "buildinfo" | "logout" | "endSessions"
    )
}

/// Whether a command WRITES, which an endpoint that serves reads only refuses.
fn writes(name: &str) -> bool {
    matches!(
        name,
        "insert"
            | "update"
            | "delete"
            | "findAndModify"
            | "create"
            | "drop"
            | "dropDatabase"
            | "createIndexes"
            | "dropIndexes"
    )
}

#[allow(clippy::too_many_arguments)]
fn run(
    session: &mut Session,
    facts: &EndpointFacts,
    volume: &mut documents::Volume,
    database: &str,
    name: &str,
    argument: &Bson,
    command: &Document,
) -> Result<Document, WireFault> {
    if !served_unauthenticated(name) {
        // Who the session is, before anything else: a command outside the handshake is served
        // only to a session that has AUTHENTICATED on this endpoint, whether or not the
        // cluster has a credential to authenticate against. An endpoint whose cluster recorded
        // none can admit no session at all — Amazon DocumentDB's `CreateDBCluster` requires a
        // master user, so no cluster of this engine is reached without one — and failing closed
        // is what keeps a socket this emulator opened from serving documents to a client that
        // presented nothing.
        if session.authenticated.is_none() {
            return Err(fault::unauthorized(name));
        }
        // A command that is not part of the handshake is served only once the resource behind
        // this endpoint is serving: one still being created has no engine to answer out of
        // yet, and a cluster endpoint has none until the member it writes through has one.
        if !facts.exists || !facts.serving {
            return Err(fault::illegal_operation(format!(
                "not serving: the resource behind this endpoint is {}",
                facts.status
            )));
        }
    }
    if writes(name) && !facts.is_writer {
        return Err(fault::not_writable_primary());
    }
    // A command inside a multi-document transaction is refused honestly: this endpoint runs
    // no transactions, so a client must not be told its writes were staged.
    if command
        .get("startTransaction")
        .is_some_and(query::truthy)
    {
        return Err(fault::illegal_operation(
            "this endpoint does not support multi-document transactions",
        ));
    }
    match name {
        "hello" | "ismaster" | "isMaster" => Ok(hello(session, facts, command)),
        "ping" => Ok(Document::new()),
        "buildInfo" | "buildinfo" => Ok(build_info(facts)),
        "saslStart" => sasl_start(session, facts, command),
        "saslContinue" => sasl_continue(session, facts, command),
        "logout" => {
            session.authenticated = None;
            session.exchange = None;
            Ok(Document::new())
        }
        // A driver ends the logical sessions it opened when it closes. This endpoint keeps no
        // session state of its own beyond the connection, so there is nothing to end — which
        // is also what the engine answers.
        "endSessions" | "killSessions" | "killAllSessions" => Ok(Document::new()),
        "insert" => insert(volume, database, argument, command),
        "find" => find(session, volume, database, argument, command),
        "getMore" => get_more(session, argument, command),
        "killCursors" => kill_cursors(session, argument, command),
        "update" => update(volume, database, argument, command),
        "delete" => delete(volume, database, argument, command),
        "count" => count(volume, database, argument, command),
        "distinct" => distinct(volume, database, argument, command),
        "aggregate" => aggregate(session, volume, database, argument, command),
        "create" => create_collection(volume, database, argument),
        "drop" => drop_collection(volume, database, argument),
        "dropDatabase" => {
            volume.drop_database(database);
            Ok(::bson::doc! { "dropped": database })
        }
        "createIndexes" => create_indexes(volume, database, argument, command),
        "dropIndexes" => drop_indexes(volume, database, argument, command),
        "listIndexes" => list_indexes(volume, database, argument),
        "listCollections" => list_collections(volume, database),
        "listDatabases" => Ok(list_databases(volume)),
        other => Err(fault::command_not_found(other)),
    }
}

/// `hello` (and its former name `isMaster`): what the endpoint is, what it accepts and how
/// big a message may be — the handshake every driver opens with.
fn hello(session: &mut Session, facts: &EndpointFacts, command: &Document) -> Document {
    if command.get("helloOk").is_some_and(query::truthy) {
        session.hello_ok = true;
    }
    let mut reply = ::bson::doc! {
        "helloOk": true,
        // Both spellings, because a driver reads whichever its wire version knows.
        "ismaster": facts.is_writer,
        "isWritablePrimary": facts.is_writer,
        "maxBsonObjectSize": documents::MAX_BSON_OBJECT_SIZE,
        "maxMessageSizeBytes": MAX_MESSAGE_SIZE_BYTES,
        "maxWriteBatchSize": MAX_WRITE_BATCH_SIZE,
        "localTime": ::bson::DateTime::now(),
        "logicalSessionTimeoutMinutes": 30_i32,
        "connectionId": session.connection_id,
        "minWireVersion": 0_i32,
        "maxWireVersion": wire_version(&facts.engine_version),
        "readOnly": !facts.is_writer,
    };
    // A client that asked which mechanisms a user may authenticate with is told, which is
    // what lets it start the exchange without a round trip of its own.
    if command.get("saslSupportedMechs").is_some() && facts.credentials.is_some() {
        reply.insert(
            "saslSupportedMechs".to_string(),
            Bson::Array(
                auth::SUPPORTED_MECHANISMS
                    .iter()
                    .map(|mechanism| Bson::String((*mechanism).to_string()))
                    .collect(),
            ),
        );
    }
    reply
}

/// The wire version the endpoint reports for a cluster running `engine_version`.
///
/// An Amazon DocumentDB cluster is compatible with the MongoDB release its engine version
/// names, and a driver decides which commands it may send from the wire version that release
/// publishes. Source: the wire version each MongoDB release publishes (3.6 is 6, 4.0 is 8,
/// 5.0 is 13, 8.0 is 25); a version outside the table is served at the lowest of them, which
/// is the conservative answer for a driver.
fn wire_version(engine_version: &str) -> i32 {
    let major_line: String = engine_version
        .split('.')
        .take(2)
        .collect::<Vec<&str>>()
        .join(".");
    match major_line.as_str() {
        "3.6" => 6,
        "4.0" => 8,
        "5.0" => 13,
        "8.0" => 25,
        _ => 6,
    }
}

/// `buildInfo`: which release the endpoint answers as, which a driver and a shell read to
/// decide what the server can do.
fn build_info(facts: &EndpointFacts) -> Document {
    let version = facts.engine_version.clone();
    let parts: Vec<Bson> = version
        .split('.')
        .map(|part| Bson::Int32(part.parse::<i32>().unwrap_or_default()))
        .collect();
    ::bson::doc! {
        "version": version,
        "versionArray": parts,
        "maxBsonObjectSize": documents::MAX_BSON_OBJECT_SIZE,
    }
}

/// The payload of a SASL command, which a driver sends as binary and a shell may send as a
/// string.
fn sasl_payload(command: &Document) -> Result<Vec<u8>, WireFault> {
    match command.get("payload") {
        Some(Bson::Binary(binary)) => Ok(binary.bytes.clone()),
        Some(Bson::String(text)) => ::aws_smithy_types::base64::decode(text)
            .map_err(|_| fault::failed_to_parse("the SASL payload is not base64")),
        _ => Err(fault::failed_to_parse("the SASL payload is missing")),
    }
}

/// How a SASL reply carries its payload: binary, which is what every driver reads.
fn binary(bytes: Vec<u8>) -> Bson {
    Bson::Binary(::bson::Binary {
        subtype: ::bson::spec::BinarySubtype::Generic,
        bytes,
    })
}

/// `saslStart`: begin the exchange that authenticates this session.
fn sasl_start(
    session: &mut Session,
    facts: &EndpointFacts,
    command: &Document,
) -> Result<Document, WireFault> {
    let Some(credentials) = facts.credentials.as_ref() else {
        // Nothing to authenticate against: a cluster created with no master user has no
        // credential a client could present, so an exchange on it cannot hold.
        return Err(fault::authentication_failed());
    };
    let mechanism = command
        .get_str("mechanism")
        .map_err(|_| fault::failed_to_parse("saslStart needs a mechanism"))?;
    let payload = sasl_payload(command)?;
    // A driver asks for the closing empty message to be skipped when it BEGINS the exchange,
    // so the answer is remembered here and read again when the proof arrives.
    session.skip_empty_exchange = command
        .get_document("options")
        .ok()
        .and_then(|options| options.get("skipEmptyExchange").cloned())
        .as_ref()
        .is_some_and(query::truthy);
    let (exchange, reply) = ScramExchange::start(mechanism, credentials, 1, &payload)
        .map_err(|_| fault::authentication_failed())?;
    let conversation_id = exchange.conversation_id();
    session.exchange = Some(exchange);
    Ok(::bson::doc! {
        "conversationId": conversation_id,
        "done": false,
        "payload": binary(reply),
    })
}

/// `saslContinue`: carry the exchange on, and record who the session is once the proof holds.
fn sasl_continue(
    session: &mut Session,
    facts: &EndpointFacts,
    command: &Document,
) -> Result<Document, WireFault> {
    let payload = sasl_payload(command)?;
    let skip_empty_exchange = session.skip_empty_exchange
        || command
            .get_document("options")
            .ok()
            .and_then(|options| options.get("skipEmptyExchange").cloned())
            .as_ref()
            .is_some_and(query::truthy);
    let Some(exchange) = session.exchange.as_mut() else {
        return Err(fault::authentication_failed());
    };
    let conversation_id = exchange.conversation_id();
    if command
        .get_i32("conversationId")
        .is_ok_and(|named| named != conversation_id)
    {
        return Err(fault::authentication_failed());
    }
    let (reply, done, authenticated) = exchange
        .advance(&payload, skip_empty_exchange)
        .map_err(|_| fault::authentication_failed())?;
    if let Some(user) = authenticated {
        // The proof holds for the credential this cluster stores; the NAME the client used
        // has to be the cluster's master user too, which is what this check is.
        let master = facts
            .credentials
            .as_ref()
            .map(ScramCredentials::username)
            .unwrap_or_default();
        if user != master {
            session.exchange = None;
            return Err(fault::authentication_failed());
        }
        session.authenticated = Some(user);
    }
    if done {
        session.exchange = None;
    }
    Ok(::bson::doc! {
        "conversationId": conversation_id,
        "done": done,
        "payload": binary(reply),
    })
}

/// The collection a command names in its first field.
fn collection_of(argument: &Bson, command_name: &str) -> Result<String, WireFault> {
    match argument {
        Bson::String(name) => Ok(name.clone()),
        _ => Err(fault::failed_to_parse(format!(
            "{command_name} needs a collection name"
        ))),
    }
}

/// A document-valued member of a command, defaulting to the empty document.
fn document_member(command: &Document, member: &str) -> Result<Document, WireFault> {
    match command.get(member) {
        None | Some(Bson::Null) => Ok(Document::new()),
        Some(Bson::Document(document)) => Ok(document.clone()),
        Some(_) => Err(fault::failed_to_parse(format!(
            "{member} must be an object"
        ))),
    }
}

/// A number-valued member of a command, as a count.
fn count_member(command: &Document, member: &str) -> Result<Option<usize>, WireFault> {
    match command.get(member) {
        None | Some(Bson::Null) => Ok(None),
        Some(Bson::Int32(value)) => Ok(usize::try_from(*value).ok()),
        Some(Bson::Int64(value)) => Ok(usize::try_from(*value).ok()),
        Some(Bson::Double(value)) => Ok(Some(value.max(0.0) as usize)),
        Some(_) => Err(fault::failed_to_parse(format!(
            "{member} must be a number"
        ))),
    }
}

/// `insert`: store the documents the command carries.
fn insert(
    volume: &mut documents::Volume,
    database: &str,
    argument: &Bson,
    command: &Document,
) -> Result<Document, WireFault> {
    let collection = collection_of(argument, "insert")?;
    let Some(Bson::Array(values)) = command.get("documents") else {
        return Err(fault::failed_to_parse("insert needs a documents array"));
    };
    if values.len() > MAX_WRITE_BATCH_SIZE as usize {
        return Err(fault::bad_value(format!(
            "write batch sizes must be between 1 and {MAX_WRITE_BATCH_SIZE}. Got {} \
             operations.",
            values.len()
        )));
    }
    let mut to_store: Vec<Document> = Vec::with_capacity(values.len());
    for value in values {
        match value {
            Bson::Document(document) => to_store.push(document.clone()),
            _ => return Err(fault::failed_to_parse("every inserted value must be an object")),
        }
    }
    let ordered = command.get("ordered").is_none_or(query::truthy);
    let outcome = volume.insert(database, &collection, to_store, ordered)?;
    let mut reply = ::bson::doc! { "n": i32::try_from(outcome.inserted).unwrap_or(i32::MAX) };
    if !outcome.errors.is_empty() {
        reply.insert("writeErrors".to_string(), write_errors(&outcome.errors));
    }
    Ok(reply)
}

/// The `writeErrors` array a partly refused write reports.
fn write_errors(errors: &[(usize, WireFault)]) -> Bson {
    Bson::Array(
        errors
            .iter()
            .map(|(index, refusal)| {
                Bson::Document(::bson::doc! {
                    "index": i32::try_from(*index).unwrap_or(i32::MAX),
                    "code": refusal.code,
                    "codeName": refusal.code_name,
                    "errmsg": refusal.message.clone(),
                })
            })
            .collect(),
    )
}

/// The cursor document a read is answered with, holding the rest of the result on the
/// session when it did not fit in one batch.
fn cursor_reply(
    session: &mut Session,
    namespace: &str,
    mut selected: Vec<Document>,
    batch_size: usize,
    single_batch: bool,
    first: bool,
) -> Result<Document, WireFault> {
    let batch: Vec<Document> = selected
        .drain(..selected.len().min(batch_size))
        .collect();
    let cursor_id = match selected.is_empty() || single_batch {
        true => 0,
        false => {
            if session.cursors.len() >= MAX_OPEN_CURSORS {
                return Err(fault::over_capacity(format!(
                    "this emulator holds at most {MAX_OPEN_CURSORS} open cursors per connection"
                )));
            }
            let id = session.next_cursor_id;
            session.next_cursor_id += 1;
            session.cursors.insert(
                id,
                Cursor {
                    namespace: namespace.to_string(),
                    remaining: selected.into_iter().collect(),
                },
            );
            id
        }
    };
    let batch: Vec<Bson> = batch.into_iter().map(Bson::Document).collect();
    let member = match first {
        true => "firstBatch",
        false => "nextBatch",
    };
    Ok(::bson::doc! {
        "cursor": {
            "id": cursor_id,
            "ns": namespace,
            member: batch,
        },
    })
}

/// `find`: the documents a filter selects, as the first batch of a cursor.
fn find(
    session: &mut Session,
    volume: &documents::Volume,
    database: &str,
    argument: &Bson,
    command: &Document,
) -> Result<Document, WireFault> {
    let collection = collection_of(argument, "find")?;
    let filter = document_member(command, "filter")?;
    let sort = document_member(command, "sort")?;
    let projection = document_member(command, "projection")?;
    let skip = count_member(command, "skip")?.unwrap_or(0);
    let limit = count_member(command, "limit")?.filter(|limit| *limit > 0);
    let batch_size = count_member(command, "batchSize")?.unwrap_or(DEFAULT_BATCH_SIZE);
    let single_batch = command.get("singleBatch").is_some_and(query::truthy);
    let selected = volume.select(
        database,
        &collection,
        &filter,
        (!sort.is_empty()).then_some(&sort),
        skip,
        limit,
        (!projection.is_empty()).then_some(&projection),
    )?;
    let namespace = format!("{database}.{collection}");
    cursor_reply(
        session,
        &namespace,
        selected,
        batch_size,
        single_batch,
        true,
    )
}

/// `getMore`: the next batch of a cursor this connection holds.
fn get_more(
    session: &mut Session,
    argument: &Bson,
    command: &Document,
) -> Result<Document, WireFault> {
    let cursor_id = match argument {
        Bson::Int64(id) => *id,
        Bson::Int32(id) => i64::from(*id),
        _ => return Err(fault::failed_to_parse("getMore needs a cursor id")),
    };
    let collection = command.get_str("collection").unwrap_or_default().to_string();
    let batch_size = count_member(command, "batchSize")?.unwrap_or(DEFAULT_BATCH_SIZE);
    let Some(cursor) = session.cursors.get_mut(&cursor_id) else {
        return Err(fault::cursor_not_found(cursor_id, &collection));
    };
    let batch: Vec<Bson> = cursor
        .remaining
        .drain(..cursor.remaining.len().min(batch_size))
        .map(Bson::Document)
        .collect();
    let namespace = cursor.namespace.clone();
    let exhausted = cursor.remaining.is_empty();
    if exhausted {
        session.cursors.remove(&cursor_id);
    }
    Ok(::bson::doc! {
        "cursor": {
            "id": match exhausted { true => 0_i64, false => cursor_id },
            "ns": namespace,
            "nextBatch": batch,
        },
    })
}

/// `killCursors`: close the cursors a client is done with.
fn kill_cursors(
    session: &mut Session,
    argument: &Bson,
    command: &Document,
) -> Result<Document, WireFault> {
    let _ = collection_of(argument, "killCursors")?;
    let Some(Bson::Array(ids)) = command.get("cursors") else {
        return Err(fault::failed_to_parse("killCursors needs a cursors array"));
    };
    let mut killed: Vec<Bson> = Vec::new();
    let mut not_found: Vec<Bson> = Vec::new();
    for id in ids {
        let id = match id {
            Bson::Int64(id) => *id,
            Bson::Int32(id) => i64::from(*id),
            _ => continue,
        };
        match session.cursors.remove(&id).is_some() {
            true => killed.push(Bson::Int64(id)),
            false => not_found.push(Bson::Int64(id)),
        }
    }
    Ok(::bson::doc! {
        "cursorsKilled": killed,
        "cursorsNotFound": not_found,
        "cursorsAlive": Vec::<Bson>::new(),
        "cursorsUnknown": Vec::<Bson>::new(),
    })
}

/// Whether a write command carries no more statements than one may: the same ceiling the
/// endpoint publishes as `maxWriteBatchSize` and holds an `insert` to, so no write command can
/// make this process do work a client sized without limit.
fn within_write_batch(command: &str, statements: usize) -> Result<(), WireFault> {
    if statements > MAX_WRITE_BATCH_SIZE as usize {
        return Err(fault::bad_value(format!(
            "write batch sizes must be between 1 and {MAX_WRITE_BATCH_SIZE}. Got {statements} \
             operations for {command}."
        )));
    }
    Ok(())
}

/// Whether the WORK a write command drives is within [`MAX_WRITE_DOCUMENT_VISITS`]: every
/// statement walks the documents the collection holds, so what is bounded here is the product
/// of the two rather than either on its own.
fn within_write_work(
    volume: &documents::Volume,
    database: &str,
    collection: &str,
    command: &str,
    statements: usize,
) -> Result<(), WireFault> {
    let held = volume
        .collection(database, collection)
        .map_or(0, |held| held.documents().len());
    let visits = statements.saturating_mul(held.max(1));
    if visits > MAX_WRITE_DOCUMENT_VISITS {
        return Err(fault::over_capacity(format!(
            "this emulator visits at most {MAX_WRITE_DOCUMENT_VISITS} documents in one \
             command, and {command} over {held} documents with {statements} statements would \
             visit {visits}"
        )));
    }
    Ok(())
}

/// `update`: apply every statement the command carries, in order.
fn update(
    volume: &mut documents::Volume,
    database: &str,
    argument: &Bson,
    command: &Document,
) -> Result<Document, WireFault> {
    let collection = collection_of(argument, "update")?;
    let Some(Bson::Array(statements)) = command.get("updates") else {
        return Err(fault::failed_to_parse("update needs an updates array"));
    };
    within_write_batch("update", statements.len())?;
    within_write_work(volume, database, &collection, "update", statements.len())?;
    let mut matched = 0_usize;
    let mut modified = 0_usize;
    let mut upserted: Vec<Bson> = Vec::new();
    for (index, statement) in statements.iter().enumerate() {
        let Bson::Document(statement) = statement else {
            return Err(fault::failed_to_parse("every update statement must be an object"));
        };
        let filter = document_member(statement, "q")?;
        let change = document_member(statement, "u")?;
        let multi = statement.get("multi").is_some_and(query::truthy);
        let upsert = statement.get("upsert").is_some_and(query::truthy);
        let outcome =
            volume.update(database, &collection, &filter, &change, multi, upsert)?;
        matched += outcome.matched;
        modified += outcome.modified;
        if let Some(id) = outcome.upserted_id {
            upserted.push(Bson::Document(::bson::doc! {
                "index": i32::try_from(index).unwrap_or(i32::MAX),
                "_id": id,
            }));
        }
    }
    let mut reply = ::bson::doc! {
        "n": i32::try_from(matched).unwrap_or(i32::MAX),
        "nModified": i32::try_from(modified).unwrap_or(i32::MAX),
    };
    if !upserted.is_empty() {
        reply.insert("upserted".to_string(), Bson::Array(upserted));
    }
    Ok(reply)
}

/// `delete`: remove what every statement selects.
fn delete(
    volume: &mut documents::Volume,
    database: &str,
    argument: &Bson,
    command: &Document,
) -> Result<Document, WireFault> {
    let collection = collection_of(argument, "delete")?;
    let Some(Bson::Array(statements)) = command.get("deletes") else {
        return Err(fault::failed_to_parse("delete needs a deletes array"));
    };
    within_write_batch("delete", statements.len())?;
    within_write_work(volume, database, &collection, "delete", statements.len())?;
    let mut removed = 0_usize;
    for statement in statements {
        let Bson::Document(statement) = statement else {
            return Err(fault::failed_to_parse("every delete statement must be an object"));
        };
        let filter = document_member(statement, "q")?;
        // `limit` is 0 for every matching document and 1 for at most one, which is the
        // member's own meaning.
        let limit = count_member(statement, "limit")?.filter(|limit| *limit > 0);
        removed += volume.delete(database, &collection, &filter, limit)?;
    }
    Ok(::bson::doc! { "n": i32::try_from(removed).unwrap_or(i32::MAX) })
}

/// `count`: how many documents a filter selects.
fn count(
    volume: &documents::Volume,
    database: &str,
    argument: &Bson,
    command: &Document,
) -> Result<Document, WireFault> {
    let collection = collection_of(argument, "count")?;
    let filter = document_member(command, "query")?;
    let skip = count_member(command, "skip")?.unwrap_or(0);
    let limit = count_member(command, "limit")?.filter(|limit| *limit > 0);
    let counted = volume.count(database, &collection, &filter, skip, limit)?;
    Ok(::bson::doc! { "n": i32::try_from(counted).unwrap_or(i32::MAX) })
}

/// `distinct`: the distinct values of one field over the documents a filter selects.
fn distinct(
    volume: &documents::Volume,
    database: &str,
    argument: &Bson,
    command: &Document,
) -> Result<Document, WireFault> {
    let collection = collection_of(argument, "distinct")?;
    let field = command
        .get_str("key")
        .map_err(|_| fault::failed_to_parse("distinct needs a key"))?
        .to_string();
    let filter = document_member(command, "query")?;
    let selected = volume.select(database, &collection, &filter, None, 0, None, None)?;
    // WHETHER a value has already been seen is asked of an ordered set ([`ComparedValue`]), so
    // one value costs a logarithmic number of comparisons rather than one per value already
    // held — a command whose values are all distinct would otherwise cost the square of the
    // documents it read.
    let mut held: std::collections::BTreeSet<ComparedValue> = std::collections::BTreeSet::new();
    let mut values: Vec<Bson> = Vec::new();
    for document in &selected {
        let found = match query::value_at(document, &field) {
            None => continue,
            // A field holding an array contributes each of its elements, which is the
            // command's own rule.
            Some(Bson::Array(elements)) => elements.clone(),
            Some(value) => vec![value.clone()],
        };
        for value in found {
            if held.insert(ComparedValue(value.clone())) {
                values.push(value);
            }
        }
    }
    values.sort_by(query::compare);
    Ok(::bson::doc! { "values": values })
}

/// `create`: a collection with nothing in it.
fn create_collection(
    volume: &mut documents::Volume,
    database: &str,
    argument: &Bson,
) -> Result<Document, WireFault> {
    let collection = collection_of(argument, "create")?;
    volume.create_collection(database, &collection)?;
    Ok(Document::new())
}

/// `drop`: a collection and everything in it.
fn drop_collection(
    volume: &mut documents::Volume,
    database: &str,
    argument: &Bson,
) -> Result<Document, WireFault> {
    let collection = collection_of(argument, "drop")?;
    if !volume.drop_collection(database, &collection) {
        return Err(fault::namespace_not_found(&format!(
            "{database}.{collection}"
        )));
    }
    Ok(::bson::doc! { "nIndexesWas": 1_i32, "ns": format!("{database}.{collection}") })
}

/// `createIndexes`: build the indexes the command describes.
fn create_indexes(
    volume: &mut documents::Volume,
    database: &str,
    argument: &Bson,
    command: &Document,
) -> Result<Document, WireFault> {
    let collection = collection_of(argument, "createIndexes")?;
    let Some(Bson::Array(specs)) = command.get("indexes") else {
        return Err(fault::failed_to_parse("createIndexes needs an indexes array"));
    };
    // Bounded by the ceiling a collection's indexes are bounded by rather than by the write
    // batch: each entry builds one index, and a collection holds only so many.
    if specs.len() > documents::MAX_INDEXES_PER_COLLECTION {
        return Err(fault::over_capacity(format!(
            "this emulator builds at most {} indexes in one call, got {}",
            documents::MAX_INDEXES_PER_COLLECTION,
            specs.len()
        )));
    }
    let mut before = None;
    let mut after = 0_usize;
    let mut created_collection = false;
    for spec in specs {
        let Bson::Document(spec) = spec else {
            return Err(fault::failed_to_parse("every index must be an object"));
        };
        let key = document_member(spec, "key")?;
        if key.is_empty() {
            return Err(fault::failed_to_parse("every index needs a key"));
        }
        let name = match spec.get_str("name") {
            Ok(name) => name.to_string(),
            Err(_) => IndexSpec::derived_name(&key),
        };
        let unique = spec.get("unique").is_some_and(query::truthy);
        let (was, now, created) = volume.create_index(
            database,
            &collection,
            IndexSpec { name, key, unique },
        )?;
        before = Some(before.unwrap_or(was));
        after = now;
        created_collection |= created;
    }
    let before = before.unwrap_or(after);
    Ok(::bson::doc! {
        "createdCollectionAutomatically": created_collection,
        "numIndexesBefore": i32::try_from(before).unwrap_or(i32::MAX),
        "numIndexesAfter": i32::try_from(after).unwrap_or(i32::MAX),
    })
}

/// `dropIndexes`: remove an index by name.
fn drop_indexes(
    volume: &mut documents::Volume,
    database: &str,
    argument: &Bson,
    command: &Document,
) -> Result<Document, WireFault> {
    let collection = collection_of(argument, "dropIndexes")?;
    let name = command
        .get_str("index")
        .map_err(|_| fault::failed_to_parse("dropIndexes needs an index name"))?
        .to_string();
    let before = volume
        .collection(database, &collection)
        .map(|held| held.indexes().len())
        .unwrap_or_default();
    if !volume.drop_index(database, &collection, &name)? {
        return Err(fault::namespace_not_found(&format!(
            "{database}.{collection} index: {name}"
        )));
    }
    Ok(::bson::doc! { "nIndexesWas": i32::try_from(before).unwrap_or(i32::MAX) })
}

/// `listIndexes`: the indexes over one collection, as a cursor whose whole result is in its
/// first batch.
fn list_indexes(
    volume: &documents::Volume,
    database: &str,
    argument: &Bson,
) -> Result<Document, WireFault> {
    let collection = collection_of(argument, "listIndexes")?;
    let namespace = format!("{database}.{collection}");
    let Some(held) = volume.collection(database, &collection) else {
        return Err(fault::namespace_not_found(&namespace));
    };
    let indexes: Vec<Bson> = held
        .indexes()
        .iter()
        .map(|index| {
            let mut described = ::bson::doc! {
                "v": 2_i32,
                "key": index.key.clone(),
                "name": index.name.clone(),
            };
            if index.unique {
                described.insert("unique".to_string(), true);
            }
            Bson::Document(described)
        })
        .collect();
    Ok(::bson::doc! {
        "cursor": { "id": 0_i64, "ns": format!("{database}.$cmd.listIndexes.{collection}"), "firstBatch": indexes },
    })
}

/// `listCollections`: the collections of one database.
fn list_collections(
    volume: &documents::Volume,
    database: &str,
) -> Result<Document, WireFault> {
    let names = volume
        .database(database)
        .map(super::documents::Database::collection_names)
        .unwrap_or_default();
    let described: Vec<Bson> = names
        .into_iter()
        .map(|name| {
            Bson::Document(::bson::doc! {
                "name": name,
                "type": "collection",
                "options": Document::new(),
                "info": { "readOnly": false },
            })
        })
        .collect();
    Ok(::bson::doc! {
        "cursor": { "id": 0_i64, "ns": format!("{database}.$cmd.listCollections"), "firstBatch": described },
    })
}

/// `listDatabases`: the databases this cluster holds.
fn list_databases(volume: &documents::Volume) -> Document {
    let databases: Vec<Bson> = volume
        .database_names()
        .into_iter()
        .map(|name| {
            let documents = volume
                .database(&name)
                .map(|held| {
                    held.collection_names()
                        .iter()
                        .filter_map(|collection| held.collection(collection))
                        .map(documents_bytes)
                        .sum::<i64>()
                })
                .unwrap_or_default();
            Bson::Document(::bson::doc! {
                "name": name,
                "sizeOnDisk": documents,
                "empty": documents == 0,
            })
        })
        .collect();
    let total: i64 = databases
        .iter()
        .filter_map(|database| database.as_document()?.get_i64("sizeOnDisk").ok())
        .sum();
    ::bson::doc! { "databases": databases, "totalSize": total }
}

/// How many bytes one collection's documents are, which is what the size a database reports
/// is the sum of.
fn documents_bytes(collection: &super::documents::Collection) -> i64 {
    collection
        .documents()
        .iter()
        .map(|document| i64::try_from(documents::encoded_len(document)).unwrap_or_default())
        .sum()
}

/// `aggregate`: run the pipeline over the collection's documents.
///
/// The stages this endpoint serves are the ones below; any other is refused by name, so a
/// client is never handed a result computed by a pipeline that was only partly run.
fn aggregate(
    session: &mut Session,
    volume: &documents::Volume,
    database: &str,
    argument: &Bson,
    command: &Document,
) -> Result<Document, WireFault> {
    let collection = match argument {
        Bson::String(name) => name.clone(),
        // `aggregate: 1` runs a pipeline over no collection at all, which only a stage that
        // reads none can do — and this endpoint serves no such stage.
        _ => {
            return Err(fault::illegal_operation(
                "this endpoint runs a pipeline over a collection",
            ))
        }
    };
    let Some(Bson::Array(stages)) = command.get("pipeline") else {
        return Err(fault::failed_to_parse("aggregate needs a pipeline array"));
    };
    if stages.len() > MAX_PIPELINE_STAGES {
        return Err(fault::over_capacity(format!(
            "this emulator runs at most {MAX_PIPELINE_STAGES} pipeline stages, got {}",
            stages.len()
        )));
    }
    let mut documents =
        volume.select(database, &collection, &Document::new(), None, 0, None, None)?;
    // What the stages ASK FOR is read before any of them runs: a `$group` evaluates each of
    // its accumulators once per document of every group, so the accumulators one stage carries
    // multiply the documents the pipeline reads, exactly as the stages themselves do.
    let accumulators = widest_accumulator_spec(stages)?;
    // The WORK the pipeline drives is bounded before any of it is done: every stage walks the
    // documents the stages before it produced and evaluates its accumulators over each, so
    // what is capped is the PRODUCT of the three.
    within_pipeline_work(stages.len(), accumulators, documents.len())?;
    // And the MEMORY it would materialise, which that product does not reach: resolving an
    // accumulator's argument builds a value of about that expression's own size once per
    // member of every group, so the bytes are capped before the first value is built
    // ([`within_pipeline_memory`]).
    within_pipeline_memory(
        documents.len(),
        accumulators,
        widest_expression_bytes(stages),
    )?;
    for stage in stages {
        let Bson::Document(stage) = stage else {
            return Err(fault::failed_to_parse("every pipeline stage must be an object"));
        };
        let Some((name, argument)) = stage.iter().next() else {
            return Err(fault::failed_to_parse("a pipeline stage names no stage"));
        };
        documents = run_stage(name, argument, documents)?;
        // And the SIZE of what the stage produced, which the bounds above do not reach: they
        // count documents, and a stage can emit a document larger than the ones it read
        // ([`within_document_sizes`]).
        within_document_sizes(name, &documents)?;
    }
    let batch_size = command
        .get_document("cursor")
        .ok()
        .and_then(|cursor| count_member(cursor, "batchSize").ok().flatten())
        .unwrap_or(DEFAULT_BATCH_SIZE);
    let namespace = format!("{database}.{collection}");
    cursor_reply(session, &namespace, documents, batch_size, false, true)
}

/// One pipeline stage, over the documents the stages before it produced.
fn run_stage(
    name: &str,
    argument: &Bson,
    documents: Vec<Document>,
) -> Result<Vec<Document>, WireFault> {
    match name {
        "$match" => {
            let Bson::Document(filter) = argument else {
                return Err(fault::failed_to_parse("$match needs an object"));
            };
            let mut kept = Vec::new();
            for document in documents {
                if query::matches(&document, filter)? {
                    kept.push(document);
                }
            }
            Ok(kept)
        }
        "$sort" => {
            let Bson::Document(sort) = argument else {
                return Err(fault::failed_to_parse("$sort needs an object"));
            };
            let mut sorted = documents;
            query::sort_documents(&mut sorted, sort)?;
            Ok(sorted)
        }
        "$skip" => {
            let by = number_argument("$skip", argument)?;
            Ok(documents.into_iter().skip(by).collect())
        }
        "$limit" => {
            let to = number_argument("$limit", argument)?;
            Ok(documents.into_iter().take(to).collect())
        }
        "$project" => {
            let Bson::Document(projection) = argument else {
                return Err(fault::failed_to_parse("$project needs an object"));
            };
            documents
                .iter()
                .map(|document| query::project(document, projection))
                .collect()
        }
        "$count" => {
            let Bson::String(field) = argument else {
                return Err(fault::failed_to_parse("$count needs a field name"));
            };
            let mut counted = Document::new();
            counted.insert(
                field.clone(),
                Bson::Int32(i32::try_from(documents.len()).unwrap_or(i32::MAX)),
            );
            Ok(vec![counted])
        }
        "$group" => {
            let Bson::Document(spec) = argument else {
                return Err(fault::failed_to_parse("$group needs an object"));
            };
            group(spec, documents)
        }
        other => Err(WireFault {
            code: 40324,
            code_name: "Location40324",
            message: format!("Unrecognized pipeline stage name: '{other}'"),
        }),
    }
}

/// The number a `$skip` or `$limit` stage carries.
fn number_argument(stage: &str, argument: &Bson) -> Result<usize, WireFault> {
    match argument {
        Bson::Int32(value) => Ok(usize::try_from(*value).unwrap_or_default()),
        Bson::Int64(value) => Ok(usize::try_from(*value).unwrap_or_default()),
        Bson::Double(value) => Ok(value.max(0.0) as usize),
        _ => Err(fault::failed_to_parse(format!("{stage} needs a number"))),
    }
}

/// The MOST accumulators any one `$group` stage of a pipeline carries, refusing a stage that
/// carries more than [`MAX_GROUP_ACCUMULATORS`] of them.
///
/// Read from the stages BEFORE any of them runs, because it is one of the factors the work of
/// the pipeline is the product of ([`within_pipeline_work`]): a stage's accumulators are each
/// evaluated once per document of every group it forms, and how many a client's stage document
/// carries is bounded by nothing else — a 16 MB document holds a great many fields.
fn widest_accumulator_spec(stages: &[Bson]) -> Result<usize, WireFault> {
    let mut widest = 0_usize;
    for stage in stages {
        let Some(spec) = stage
            .as_document()
            .and_then(|stage| stage.get_document("$group").ok())
        else {
            continue;
        };
        let accumulators = accumulators_of(spec);
        if accumulators > MAX_GROUP_ACCUMULATORS {
            return Err(fault::over_capacity(format!(
                "this emulator accumulates at most {MAX_GROUP_ACCUMULATORS} fields in one \
                 $group stage, got {accumulators}"
            )));
        }
        widest = widest.max(accumulators);
    }
    Ok(widest)
}

/// The BYTES of the widest expression any `$group` stage of a pipeline would resolve per
/// document: its key expression and each of its accumulator arguments.
///
/// Read from the stages BEFORE any of them runs, for the same reason their COUNT is: an
/// expression is an arbitrary client document arriving inside the command, and resolving one
/// builds a value of about its own size for every document of every group — so the memory a
/// pipeline asks of this process is the product of how many documents it visits and how big
/// the expression it resolves over each of them is ([`within_pipeline_memory`]), and neither
/// factor bounds the other.
fn widest_expression_bytes(stages: &[Bson]) -> usize {
    let mut widest = 0_usize;
    for stage in stages {
        let Some(spec) = stage
            .as_document()
            .and_then(|stage| stage.get_document("$group").ok())
        else {
            continue;
        };
        for (_field, expression) in spec {
            widest = widest.max(value_bytes(expression));
        }
    }
    widest
}

/// How many BYTES a value is on the wire, measured by the engine's own document encoder
/// ([`documents::encoded_len`]) over the document that would carry it.
///
/// One value at a time, so measuring a value costs one copy of THAT value and never of the
/// collection it is measured for — which is what lets a budget refuse the collection before it
/// grows ([`within_pipeline_memory`], [`group`]).
fn value_bytes(value: &Bson) -> usize {
    let mut carrier = Document::new();
    carrier.insert("v".to_string(), value.clone());
    documents::encoded_len(&carrier)
}

/// Whether the MEMORY a pipeline would materialise is within
/// [`MAX_PIPELINE_MATERIALISED_BYTES`]: every accumulator of every group resolves its argument
/// expression once per member, so what is bounded here is the product of the documents
/// visited, the accumulators over each and the SIZE of the expression each resolves — the
/// factor the counting bounds above do not reach.
fn within_pipeline_memory(
    documents_visited: usize,
    accumulators: usize,
    expression_bytes: usize,
) -> Result<(), WireFault> {
    let bytes = documents_visited
        .max(1)
        .saturating_mul(accumulators.max(1))
        .saturating_mul(expression_bytes);
    if bytes > MAX_PIPELINE_MATERIALISED_BYTES {
        return Err(fault::over_capacity(format!(
            "this emulator materialises at most {MAX_PIPELINE_MATERIALISED_BYTES} bytes in one \
             command, and resolving an expression of {expression_bytes} bytes over \
             {documents_visited} documents for {accumulators} accumulators would materialise \
             {bytes}"
        )));
    }
    Ok(())
}

/// How many ACCUMULATORS a `$group` spec names: its fields but the key expression, which is
/// evaluated once per document rather than once per document per field.
fn accumulators_of(spec: &Document) -> usize {
    spec.keys().filter(|field| *field != "_id").count()
}

/// Whether the WORK a pipeline drives is within [`MAX_PIPELINE_DOCUMENT_VISITS`]: every stage
/// walks the documents the stages before it produced and evaluates each of its accumulators
/// over every one of them, so what is bounded here is the product of the three rather than any
/// of them on its own — the same reading [`within_write_work`] applies to a write command.
fn within_pipeline_work(
    stages: usize,
    accumulators: usize,
    documents: usize,
) -> Result<(), WireFault> {
    let visits = stages
        .saturating_mul(accumulators.max(1))
        .saturating_mul(documents.max(1));
    if visits > MAX_PIPELINE_DOCUMENT_VISITS {
        return Err(fault::over_capacity(format!(
            "this emulator visits at most {MAX_PIPELINE_DOCUMENT_VISITS} documents in one \
             command, and a pipeline of {stages} stages accumulating {accumulators} fields \
             over {documents} documents would visit {visits}"
        )));
    }
    Ok(())
}

/// Whether every document a pipeline stage PRODUCED is within the engine's own document
/// ceiling ([`documents::MAX_BSON_OBJECT_SIZE`], which the handshake publishes as
/// `maxBsonObjectSize` and a write is held to).
///
/// The pipeline's other bounds are COUNTS — the stages, the accumulators and the documents
/// they walk ([`within_pipeline_work`], [`within_accumulated_values`]) — and a stage can emit a
/// document far LARGER than the ones it read without emitting more of them: an accumulator's
/// argument is an arbitrary client expression, and one that assembles a document out of fields
/// it names several times grows what it emits while the count stays as it was. Each stage's
/// output feeds the next, so that growth would compound over the pipeline; checking it here
/// bounds the memory one command can ask of this process by the ceiling the engine publishes.
fn within_document_sizes(stage: &str, produced: &[Document]) -> Result<(), WireFault> {
    let ceiling = documents::MAX_BSON_OBJECT_SIZE as usize;
    for document in produced {
        let size = documents::encoded_len(document);
        if size > ceiling {
            return Err(fault::over_capacity(format!(
                "{stage} produced a document of {size} bytes, past the {ceiling} byte limit"
            )));
        }
    }
    Ok(())
}

/// One value a dedupe is keyed by — the `_id` of a `$group`, a value of an `$addToSet` or of a
/// `distinct` — ORDERED by the engine's own comparison of two values ([`query::compare`]).
///
/// What it is for is the cost of that dedupe: a new value compared against every value already
/// held costs one comparison per value, so a command whose values are all distinct would cost
/// the square of however many documents it read. Held in an ordered set instead, each value
/// finds its place in a logarithmic number of comparisons, so one request's dedupe stays
/// within the documents it read rather than their square.
///
/// The ORDER a command answers in is not this type's: the caller keeps the order it owes (the
/// order the groups of a `$group` were first seen, the engine's own order for `distinct`) in a
/// list beside the set.
#[derive(Clone, Debug)]
struct ComparedValue(Bson);

/// Two values are the same value exactly when the engine's own comparison says so, which is
/// what makes the ordered sets below dedupe the way a scan with [`query::compare`] did.
impl PartialEq for ComparedValue {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}

impl Eq for ComparedValue {}

impl Ord for ComparedValue {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        query::compare(&self.0, &other.0)
    }
}

impl PartialOrd for ComparedValue {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// `$group`: one document per distinct `_id` expression, carrying the accumulators the stage
/// names.
fn group(spec: &Document, documents: Vec<Document>) -> Result<Vec<Document>, WireFault> {
    let key_expression = spec
        .get("_id")
        .cloned()
        .ok_or_else(|| fault::failed_to_parse("$group needs an _id"))?;
    // How many fields this stage accumulates is one of the factors the pipeline's work was
    // bounded by before any stage ran ([`widest_accumulator_spec`]); it is asked again here, so
    // the bound holds for every caller of this stage and not only for that one path.
    let accumulators = accumulators_of(spec);
    if accumulators > MAX_GROUP_ACCUMULATORS {
        return Err(fault::over_capacity(format!(
            "this emulator accumulates at most {MAX_GROUP_ACCUMULATORS} fields in one $group \
             stage, got {accumulators}"
        )));
    }
    // The groups keep the order their keys were first seen, which is the order the engine
    // produces them in for a pipeline with no sort after the group — and WHICH group a document
    // belongs to is resolved through an ordered map ([`ComparedValue`]), so one document costs a
    // logarithmic number of comparisons rather than one per group already seen.
    let mut positions: BTreeMap<ComparedValue, usize> = BTreeMap::new();
    let mut keys: Vec<Bson> = Vec::new();
    let mut grouped: Vec<Vec<Document>> = Vec::new();
    // The bytes THIS STAGE has materialised, carried across every group and every accumulator
    // of it: one budget for the whole stage, so what is bounded is everything the stage builds
    // and not each group's own share of it ([`charged`]). It is charged for every value the
    // stage resolves — a group key, an accumulator's value over a member, the accumulated
    // result — because each is a value this process holds and none of their sizes is bounded by
    // the expression that produced it: a field path of a few bytes resolves to whatever the
    // stored field holds.
    let mut materialised = 0_usize;
    for document in documents {
        let key = evaluate(&key_expression, &document)?;
        materialised = charged(materialised, value_bytes(&key), "the $group key")?;
        match positions.get(&ComparedValue(key.clone())) {
            Some(position) => grouped[*position].push(document),
            None => {
                positions.insert(ComparedValue(key.clone()), keys.len());
                keys.push(key);
                grouped.push(vec![document]);
            }
        }
    }
    let mut out: Vec<Document> = Vec::new();
    for (key, members) in keys.into_iter().zip(grouped) {
        let mut result = Document::new();
        result.insert("_id".to_string(), key);
        for (field, accumulator) in spec {
            if field == "_id" {
                continue;
            }
            let Bson::Document(accumulator) = accumulator else {
                return Err(fault::failed_to_parse(format!(
                    "the accumulator of {field} must be an object"
                )));
            };
            let Some((operator, argument)) = accumulator.iter().next() else {
                return Err(fault::failed_to_parse(format!(
                    "the accumulator of {field} names no operator"
                )));
            };
            // The values of this accumulator over this group's members, built ONE at a time
            // under the stage's running byte budget: the argument is a client expression and
            // the value it resolves to is whatever the document holds, so the collection is
            // refused as it grows rather than after it has been materialised, and the count it
            // may reach is the accumulator's own ceiling ([`within_accumulated_values`]).
            let mut values: Vec<Bson> = Vec::new();
            for document in &members {
                within_accumulated_values(operator, values.len() + 1)?;
                let value = evaluate(argument, document)?;
                materialised = charged(materialised, value_bytes(&value), operator)?;
                values.push(value);
            }
            let value = accumulate(operator, &values)?;
            // And the accumulated result, which is what the stage HOLDS once the values it was
            // accumulated from are dropped.
            materialised = charged(materialised, value_bytes(&value), operator)?;
            result.insert(field.clone(), value);
        }
        // The document this group produced, held to the engine's own document ceiling as it is
        // produced rather than once the whole stage has been built
        // ([`within_document_sizes`]).
        within_document_sizes("$group", std::slice::from_ref(&result))?;
        out.push(result);
    }
    Ok(out)
}

/// Charge `bytes` against what this command has MATERIALISED so far, refusing it past
/// [`MAX_PIPELINE_MATERIALISED_BYTES`] — `what` names the expression or accumulator that asked
/// for them, so a refusal says which part of the pipeline reached the ceiling.
///
/// Monotonic over one stage: a budget that only ever grows refuses a command by what it has
/// built in total, which is the one reading that cannot be escaped by building the same bytes
/// again under another group or another accumulator.
fn charged(materialised: usize, bytes: usize, what: &str) -> Result<usize, WireFault> {
    let total = materialised.saturating_add(bytes);
    if total > MAX_PIPELINE_MATERIALISED_BYTES {
        return Err(fault::over_capacity(format!(
            "this emulator materialises at most {MAX_PIPELINE_MATERIALISED_BYTES} bytes in one \
             command, and {what} has reached {total}"
        )));
    }
    Ok(total)
}

/// One accumulator over the values of one group.
fn accumulate(operator: &str, values: &[Bson]) -> Result<Bson, WireFault> {
    let numbers: Vec<f64> = values
        .iter()
        .filter_map(|value| match value {
            Bson::Int32(number) => Some(f64::from(*number)),
            Bson::Int64(number) => Some(*number as f64),
            Bson::Double(number) => Some(*number),
            _ => None,
        })
        .collect();
    match operator {
        "$sum" => {
            let total: f64 = numbers.iter().sum();
            Ok(whole_or_double(total))
        }
        "$avg" => match numbers.is_empty() {
            true => Ok(Bson::Null),
            false => Ok(Bson::Double(
                numbers.iter().sum::<f64>() / numbers.len() as f64,
            )),
        },
        "$min" => Ok(values
            .iter()
            .filter(|value| !matches!(value, Bson::Null))
            .min_by(|left, right| query::compare(left, right))
            .cloned()
            .unwrap_or(Bson::Null)),
        "$max" => Ok(values
            .iter()
            .filter(|value| !matches!(value, Bson::Null))
            .max_by(|left, right| query::compare(left, right))
            .cloned()
            .unwrap_or(Bson::Null)),
        "$first" => Ok(values.first().cloned().unwrap_or(Bson::Null)),
        "$last" => Ok(values.last().cloned().unwrap_or(Bson::Null)),
        "$push" => {
            within_accumulated_values("$push", values.len())?;
            Ok(Bson::Array(values.to_vec()))
        }
        "$addToSet" => {
            within_accumulated_values("$addToSet", values.len())?;
            // The set keeps the order its values were first seen, and WHETHER a value is
            // already held is asked of an ordered set ([`ComparedValue`]), so one value costs a
            // logarithmic number of comparisons rather than one per value already held.
            let mut held: std::collections::BTreeSet<ComparedValue> =
                std::collections::BTreeSet::new();
            let mut set: Vec<Bson> = Vec::new();
            for value in values {
                if held.insert(ComparedValue(value.clone())) {
                    set.push(value.clone());
                }
            }
            Ok(Bson::Array(set))
        }
        "$count" => Ok(Bson::Int32(
            i32::try_from(values.len()).unwrap_or(i32::MAX),
        )),
        other => Err(fault::failed_to_parse(format!(
            "unknown group operator '{other}'"
        ))),
    }
}

/// Whether an accumulator may hold this many values ([`MAX_ACCUMULATED_VALUES`]), asked as the
/// collection grows and again before an array is built out of it.
fn within_accumulated_values(operator: &str, values: usize) -> Result<(), WireFault> {
    if values > MAX_ACCUMULATED_VALUES {
        return Err(fault::over_capacity(format!(
            "this emulator retains at most {MAX_ACCUMULATED_VALUES} values in one accumulator, \
             and {operator} over this group would retain {values}"
        )));
    }
    Ok(())
}

/// A whole number as an integer and anything else as a double, which is how the engine types
/// the result of a sum over integers.
fn whole_or_double(total: f64) -> Bson {
    match total.fract() == 0.0 && total.abs() <= f64::from(i32::MAX) {
        true => Bson::Int32(total as i32),
        false => Bson::Double(total),
    }
}

/// One aggregation expression over a document: a `$`-prefixed string is a FIELD PATH, and
/// anything else is the literal it is.
///
/// An expression nested deeper than the plane's own ceiling
/// ([`query::MAX_NESTING_DEPTH`]) is REFUSED by name: the nesting is the client's, so what
/// bounds the walk is that ceiling rather than this process's stack.
fn evaluate(expression: &Bson, document: &Document) -> Result<Bson, WireFault> {
    evaluate_at(expression, document, 0)
}

fn evaluate_at(
    expression: &Bson,
    document: &Document,
    depth: usize,
) -> Result<Bson, WireFault> {
    if depth > query::MAX_NESTING_DEPTH {
        return Err(fault::failed_to_parse(format!(
            "the expression exceeds the maximum nesting depth of {}",
            query::MAX_NESTING_DEPTH
        )));
    }
    match expression {
        Bson::String(text) => Ok(match text.strip_prefix('$') {
            Some(path) => query::value_at(document, path).cloned().unwrap_or(Bson::Null),
            None => Bson::String(text.clone()),
        }),
        Bson::Document(fields) => {
            let mut resolved = Document::new();
            for (field, value) in fields {
                resolved.insert(field.clone(), evaluate_at(value, document, depth + 1)?);
            }
            Ok(Bson::Document(resolved))
        }
        other => Ok(other.clone()),
    }
}
