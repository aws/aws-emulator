// SPDX-License-Identifier: CC0-1.0

//! The COMMANDS one session on an elastic cluster's endpoint may send, and what each is
//! answered with.
//!
//! An elastic cluster speaks the same wire as any other Amazon DocumentDB cluster, so what a
//! session may DO with its documents — insert, find, update, delete, aggregate, index — is the
//! shared command layer's
//! ([`::service_com_amazonaws_docdb_amazonrdsv19::state::cluster_data_plane::commands`]), over
//! the cluster's own volume. This module is what an elastic cluster does DIFFERENTLY, and
//! nothing else:
//!
//! * the HANDSHAKE and the SASL exchange: an elastic cluster offers one mechanism
//!   (`SCRAM-SHA-1`) and reports a sharded topology, because an elastic cluster distributes a
//!   collection over its shards;
//! * the AUTHORIZATION gate: a command outside the handshake is answered for a session that
//!   has not authenticated with this engine's own `AuthorizationFailed`;
//! * SHARDING: `shardCollection`, and the rules every document written to a sharded collection
//!   holds to — that it carries that collection's shard key, that an upsert is targeted by it
//!   and that it is never altered ([`super::sharding`]).
//!
//! Every fact a command is judged by is read from the control-plane STORE on each command
//! rather than captured when the connection was accepted ([`super::ClusterSocket::facts`]), so
//! a change the control plane makes while a session is open — the cluster stopped, the
//! administrator's password changed, the cluster deleted — is in force on the next command
//! that session sends.
//!
//! LOCK ORDER inside one command: the control-plane store first (the facts), then this
//! cluster's shard keys, then its documents. Nothing here takes them the other way round.

use ::bson::{Bson, Document};
use ::service_com_amazonaws_docdb_amazonrdsv19::state::cluster_data_plane as document_engine;

use document_engine::auth::{self, AuthFailure, ScramCredentials, ScramExchange};
use document_engine::commands as engine_commands;
use document_engine::documents;
use document_engine::query;

use super::refusal::{self, Refusal};
use super::session::ClusterSession;
use super::sharded_writes::Screening;
use super::sharding;
use super::{ClusterSocket, EndpointFacts, MONGO_COMPATIBILITY_VERSION};

/// The wire version an elastic cluster's endpoint reports, which is what a driver reads to
/// decide which commands it may send.
///
/// Source: the wire version the MongoDB release an elastic cluster is compatible with
/// ([`MONGO_COMPATIBILITY_VERSION`], 5.0) publishes, which is 13.
const WIRE_VERSION: i32 = 13;

/// How long a logical session a driver opened is kept, in minutes, which the handshake
/// publishes as `logicalSessionTimeoutMinutes`: the engine's own default.
const LOGICAL_SESSION_TIMEOUT_MINUTES: i32 = 30;

/// The conversation id every SASL exchange on this endpoint runs under. One connection runs
/// one exchange at a time, so the id a reply carries and a `saslContinue` names is this.
const CONVERSATION_ID: i32 = 1;

/// The mechanism an elastic cluster authenticates a client with: `SCRAM-SHA-1`, the one it
/// offers. A `saslStart` naming any other is refused ([`refusal::invalid_auth_mechanism`]).
const MECHANISM: &str = auth::SCRAM_SHA_1;

/// What a driver reads out of the handshake to know the topology it is talking to is a SHARDED
/// cluster, which an elastic cluster is: its collections are distributed over its shards.
const SHARDED_TOPOLOGY: &str = "isdbgrid";

/// Answer one command, as the reply document the client reads.
///
/// `database` is the `$db` the message named and `command` the command document itself.
pub fn answer(
    socket: &ClusterSocket,
    session: &mut ClusterSession,
    database: &str,
    command: &Document,
) -> Document {
    let Some((name, argument)) = command.iter().next() else {
        return refusal::failed_to_parse("the command document is empty").to_reply();
    };
    match run(socket, session, database, name, argument, command) {
        Ok(mut reply) => {
            // Every reply carries `ok`, which is what a client reads first.
            if !reply.contains_key("ok") {
                reply.insert("ok".to_string(), Bson::Double(1.0));
            }
            reply
        }
        Err(refused) => refused.to_reply(),
    }
}

/// The commands served whoever the session is: the handshake, the SASL exchange itself, the
/// liveness check and the session bookkeeping a driver sends before it has authenticated
/// anything.
fn served_unauthenticated(name: &str) -> bool {
    matches!(
        name,
        "hello"
            | "ismaster"
            | "isMaster"
            | "ping"
            | "saslStart"
            | "saslContinue"
            | "buildInfo"
            | "buildinfo"
            | "logout"
            | "endSessions"
            | "killSessions"
            | "killAllSessions"
    )
}

fn run(
    socket: &ClusterSocket,
    session: &mut ClusterSession,
    database: &str,
    name: &str,
    argument: &Bson,
    command: &Document,
) -> Result<Document, Refusal> {
    let facts = socket.facts();
    if served_unauthenticated(name) {
        return match name {
            "hello" | "ismaster" | "isMaster" => Ok(hello(session, &facts, command)),
            "buildInfo" | "buildinfo" => Ok(build_info()),
            "saslStart" => sasl_start(session, &facts, command),
            "saslContinue" => sasl_continue(session, &facts, command),
            "logout" => {
                session.forget();
                Ok(Document::new())
            }
            // `ping` and the commands that end the logical sessions a driver opened. This
            // endpoint keeps no state of its own beyond the connection, so there is nothing to
            // end — which is also what the engine answers.
            _ => Ok(Document::new()),
        };
    }
    // The cluster authorizes nothing for a connection that presented no credential: the
    // document commands below are reached only by a session that completed the exchange.
    if !session.is_authenticated() {
        return Err(refusal::authorization_failed());
    }
    // A command outside the handshake is served only while the cluster is up: one still being
    // created, being changed or stopped has no engine to answer out of yet.
    if !facts.exists || !facts.serving {
        return Err(refusal::illegal_operation(format!(
            "not serving: the elastic cluster behind this endpoint is {}",
            facts.status
        )));
    }
    match name {
        // Sharding a collection is this endpoint's own command: which collections are
        // distributed over the cluster's shards, and on which key, is the cluster's state and
        // not the document engine's.
        "shardCollection" => {
            let mut keys = socket.storage.lock_shard_keys();
            let mut volume = socket.storage.lock_documents();
            sharding::shard_collection(&mut volume, &mut keys, command, argument)
        }
        // A write of a sharded collection is judged against that collection's shard key before
        // any of it is carried out — the documents an `insert` stores, and the statements an
        // `update` applies, each entry on its own — and an entry the key refuses is reported as
        // that entry's write error of a command that otherwise ran ([`super::sharded_writes`]).
        "insert" => {
            let collection = collection_of(argument, name)?;
            let Some(Bson::Array(documents)) = command.get("documents") else {
                // A command carrying no documents array is the shared layer's to refuse.
                return delegate(socket, session, &facts, database, command);
            };
            if !within_the_write_batch(documents) {
                return delegate(socket, session, &facts, database, command);
            }
            let namespace = format!("{database}.{collection}");
            let screening = {
                let keys = socket.storage.lock_shard_keys();
                Screening::of(documents, ordered(command), |document| {
                    sharding::document_carries_the_shard_key(&keys, &namespace, document)
                })
            };
            if screening.admits_everything() {
                return delegate(socket, session, &facts, database, command);
            }
            let narrowed = screening.narrowed(command, "documents", documents);
            let reply = delegate(socket, session, &facts, database, &narrowed)?;
            Ok(screening.report(reply))
        }
        "update" => {
            let collection = collection_of(argument, name)?;
            let Some(Bson::Array(statements)) = command.get("updates") else {
                return delegate(socket, session, &facts, database, command);
            };
            if !within_the_write_batch(statements) {
                return delegate(socket, session, &facts, database, command);
            }
            let namespace = format!("{database}.{collection}");
            // A write the cluster can REPLAY carries a transaction number: that is what a
            // retryable write and a statement of a multi-statement transaction have, and what
            // decides whether a statement may move a document between shards.
            let retryable = command.contains_key("txnNumber");
            let screening = {
                let keys = socket.storage.lock_shard_keys();
                Screening::of(statements, ordered(command), |statement| {
                    // A statement that is not a document at all is the shared layer's to
                    // refuse, as it is for an unsharded collection.
                    let Bson::Document(statement) = statement else {
                        return Ok(());
                    };
                    // The members an update statement carries its filter and its change in
                    // (`q` and `u`), which is what the shared command layer reads them from
                    // too.
                    sharding::statement_respects_the_shard_key(
                        &keys,
                        &namespace,
                        statement.get("upsert").is_some_and(query::truthy),
                        retryable,
                        &document_member(statement, "q"),
                        &document_member(statement, "u"),
                    )
                })
            };
            if screening.admits_everything() {
                return delegate(socket, session, &facts, database, command);
            }
            let narrowed = screening.narrowed(command, "updates", statements);
            let reply = delegate(socket, session, &facts, database, &narrowed)?;
            Ok(screening.report(reply))
        }
        // A collection that is gone is sharded no longer, and neither is any collection of a
        // database that is gone: the keys are the cluster's record of its own namespaces.
        "drop" => {
            let collection = collection_of(argument, name)?;
            let reply = delegate(socket, session, &facts, database, command)?;
            if succeeded(&reply) {
                socket
                    .storage
                    .lock_shard_keys()
                    .forget(&format!("{database}.{collection}"));
            }
            Ok(reply)
        }
        "dropDatabase" => {
            let reply = delegate(socket, session, &facts, database, command)?;
            if succeeded(&reply) {
                socket.storage.lock_shard_keys().forget_database(database);
            }
            Ok(reply)
        }
        // Every other command is the document engine's, answered out of this cluster's volume.
        _ => delegate(socket, session, &facts, database, command),
    }
}

/// Hand one command to the shared command layer, over this cluster's documents.
///
/// The session it is given is the one opened when this connection authenticated, so the
/// cursors a read leaves open belong to this connection and to nobody else.
fn delegate(
    socket: &ClusterSocket,
    session: &mut ClusterSession,
    facts: &EndpointFacts,
    database: &str,
    command: &Document,
) -> Result<Document, Refusal> {
    let Some(served) = session.served.as_mut() else {
        return Err(refusal::authorization_failed());
    };
    let mut volume = socket.storage.lock_documents();
    Ok(engine_commands::execute(
        served, facts, &mut volume, database, command,
    ))
}

/// Whether a reply reports the command it answers as having been carried out.
fn succeeded(reply: &Document) -> bool {
    reply.get("ok").is_some_and(query::truthy)
}

/// The collection a command names in its first field.
fn collection_of(argument: &Bson, command_name: &str) -> Result<String, Refusal> {
    match argument {
        Bson::String(name) => Ok(name.clone()),
        _ => Err(refusal::failed_to_parse(format!(
            "{command_name} needs a collection name"
        ))),
    }
}

/// Whether a write command's entries are carried out in order, which is what decides whether
/// the entries after a refused one are carried out at all: `ordered` as the command carries it,
/// and in order for a command that carries no answer, which is the member's own default.
fn ordered(command: &Document) -> bool {
    command.get("ordered").is_none_or(query::truthy)
}

/// Whether a write command carries no more entries than one may — the ceiling the endpoint
/// publishes as `maxWriteBatchSize` and the shared command layer holds every write to.
///
/// A command past it is refused THERE, with the sentence that layer words for it, so a command
/// a client sized without limit is never screened entry by entry first: what bounds the work
/// one command drives, and the reply it is answered with, is that one ceiling.
fn within_the_write_batch(entries: &[Bson]) -> bool {
    entries.len() <= engine_commands::MAX_WRITE_BATCH_SIZE as usize
}

/// A document-valued member of a command, and the empty document for a member that is absent
/// or is not a document at all — which the shared command layer refuses for itself when it
/// runs the command, so nothing here decides that.
fn document_member(command: &Document, member: &str) -> Document {
    match command.get(member) {
        Some(Bson::Document(document)) => document.clone(),
        _ => Document::new(),
    }
}

/// `hello` (and its former name `isMaster`): what the endpoint is, what it accepts and how big
/// a message may be — the handshake every driver opens with.
///
/// An elastic cluster is reached at ONE endpoint, which every read and every write goes
/// through, so the endpoint always answers as the writable one; and it reports a sharded
/// topology ([`SHARDED_TOPOLOGY`]), because a collection of such a cluster is distributed over
/// its shards.
fn hello(session: &ClusterSession, facts: &EndpointFacts, command: &Document) -> Document {
    let mut reply = ::bson::doc! {
        "helloOk": true,
        // Both spellings, because a driver reads whichever its wire version knows.
        "ismaster": true,
        "isWritablePrimary": true,
        "msg": SHARDED_TOPOLOGY,
        "maxBsonObjectSize": documents::MAX_BSON_OBJECT_SIZE,
        "maxMessageSizeBytes": engine_commands::MAX_MESSAGE_SIZE_BYTES,
        "maxWriteBatchSize": engine_commands::MAX_WRITE_BATCH_SIZE,
        "localTime": ::bson::DateTime::now(),
        "logicalSessionTimeoutMinutes": LOGICAL_SESSION_TIMEOUT_MINUTES,
        "connectionId": session.connection_id,
        "minWireVersion": 0_i32,
        "maxWireVersion": WIRE_VERSION,
        "readOnly": false,
    };
    // A client that asked which mechanisms a user may authenticate with is told, which is what
    // lets it start the exchange without a round trip of its own.
    if command.get("saslSupportedMechs").is_some() && facts.credentials.is_some() {
        reply.insert(
            "saslSupportedMechs".to_string(),
            Bson::Array(vec![Bson::String(MECHANISM.to_string())]),
        );
    }
    reply
}

/// `buildInfo`: which release the endpoint answers as, which a driver and a shell read to
/// decide what the server can do.
fn build_info() -> Document {
    let parts: Vec<Bson> = MONGO_COMPATIBILITY_VERSION
        .split('.')
        .map(|part| Bson::Int32(part.parse::<i32>().unwrap_or_default()))
        .collect();
    ::bson::doc! {
        "version": MONGO_COMPATIBILITY_VERSION,
        "versionArray": parts,
        "maxBsonObjectSize": documents::MAX_BSON_OBJECT_SIZE,
    }
}

/// The payload of a SASL command, which a driver sends as binary and a shell may send as a
/// string.
fn sasl_payload(command: &Document) -> Result<Vec<u8>, Refusal> {
    match command.get("payload") {
        Some(Bson::Binary(binary)) => Ok(binary.bytes.clone()),
        Some(Bson::String(text)) => ::aws_smithy_types::base64::decode(text)
            .map_err(|_| refusal::failed_to_parse("the SASL payload is not base64")),
        _ => Err(refusal::failed_to_parse("the SASL payload is missing")),
    }
}

/// How a SASL reply carries its payload: binary, which is what every driver reads.
fn binary(bytes: Vec<u8>) -> Bson {
    Bson::Binary(::bson::Binary {
        subtype: ::bson::spec::BinarySubtype::Generic,
        bytes,
    })
}

/// Whether the client asked for the closing empty message of the exchange to be skipped.
fn asks_to_skip_the_empty_exchange(command: &Document) -> bool {
    command
        .get_document("options")
        .ok()
        .and_then(|options| options.get("skipEmptyExchange").cloned())
        .as_ref()
        .is_some_and(query::truthy)
}

/// `saslStart`: begin the exchange that authenticates this session as the cluster's
/// administrator.
///
/// The mechanism is checked FIRST, because a client naming a mechanism this cluster does not
/// offer is told exactly that, whoever it claims to be; a well-formed exchange for a user that
/// is not this cluster's administrator is a user that does not exist.
///
/// An exchange that FAILED to authenticate ends the connection ([`ClusterSession::closing`]):
/// the client presented a credential the cluster does not hold, so it does not get to keep
/// presenting credentials on that connection. A mechanism the cluster does not offer is not
/// such a failure — nothing was presented — and the client may begin an exchange with one the
/// handshake offers instead.
fn sasl_start(
    session: &mut ClusterSession,
    facts: &EndpointFacts,
    command: &Document,
) -> Result<Document, Refusal> {
    let mechanism = command
        .get_str("mechanism")
        .map_err(|_| refusal::failed_to_parse("saslStart needs a mechanism"))?;
    if mechanism != MECHANISM {
        return Err(refusal::invalid_auth_mechanism(mechanism));
    }
    let payload = sasl_payload(command)?;
    // A driver asks for the closing empty message to be skipped when it BEGINS the exchange,
    // so the answer is remembered here and read again when the proof arrives.
    session.skip_empty_exchange = asks_to_skip_the_empty_exchange(command);
    // A cluster whose authentication type names a secret rather than carrying a password has
    // no credential this emulator can check a proof against, so no exchange on it can hold.
    let Some(credentials) = facts.credentials.as_ref() else {
        return Err(failed_to_authenticate(session, refusal::authentication_failed()));
    };
    let started = ScramExchange::start(mechanism, credentials, CONVERSATION_ID, &payload);
    let (exchange, reply) = match started {
        Ok(started) => started,
        // The name the client-first message carried is not this cluster's administrator, which
        // is the one user an elastic cluster has.
        Err(AuthFailure::NotAuthenticated) => {
            return Err(failed_to_authenticate(
                session,
                refusal::user_does_not_exist(),
            ))
        }
        Err(_) => {
            return Err(failed_to_authenticate(
                session,
                refusal::authentication_failed(),
            ))
        }
    };
    session.exchange = Some(exchange);
    Ok(::bson::doc! {
        "conversationId": CONVERSATION_ID,
        "done": false,
        "payload": binary(reply),
    })
}

/// `saslContinue`: carry the exchange on, and record who the session is once the proof holds.
///
/// A proof that did not hold ends the connection, exactly as a `saslStart` naming a user the
/// cluster does not have does ([`sasl_start`]).
fn sasl_continue(
    session: &mut ClusterSession,
    facts: &EndpointFacts,
    command: &Document,
) -> Result<Document, Refusal> {
    let payload = sasl_payload(command)?;
    let skip_empty_exchange =
        session.skip_empty_exchange || asks_to_skip_the_empty_exchange(command);
    // Taken out of the session, and put back only while the conversation carries on: an
    // exchange that did not hold is over, and the connection goes with it.
    let Some(mut exchange) = session.exchange.take() else {
        return Err(failed_to_authenticate(
            session,
            refusal::authentication_failed(),
        ));
    };
    let conversation_id = exchange.conversation_id();
    if command
        .get_i32("conversationId")
        .is_ok_and(|named| named != conversation_id)
    {
        return Err(failed_to_authenticate(
            session,
            refusal::authentication_failed(),
        ));
    }
    let advanced = exchange.advance(&payload, skip_empty_exchange);
    let (reply, done, authenticated) = match advanced {
        Ok(advanced) => advanced,
        Err(_) => {
            return Err(failed_to_authenticate(
                session,
                refusal::authentication_failed(),
            ))
        }
    };
    if let Some(user) = authenticated {
        // The proof holds for the credential this cluster stores; the NAME the client used has
        // to be the cluster's administrator too, which is what this check is.
        let administrator = facts
            .credentials
            .as_ref()
            .map(ScramCredentials::username)
            .unwrap_or_default();
        if user != administrator {
            return Err(failed_to_authenticate(
                session,
                refusal::authentication_failed(),
            ));
        }
        session.admit(&user);
    }
    if !done {
        session.exchange = Some(exchange);
    }
    Ok(::bson::doc! {
        "conversationId": conversation_id,
        "done": done,
        "payload": binary(reply),
    })
}

/// Mark this connection as ENDING with the refusal it is about to be answered: the reply goes
/// out and the endpoint then closes the connection
/// (`document_engine::wire::WireEndpoint::carries_on`), which is what a client that presented a
/// credential the cluster does not hold is answered with.
fn failed_to_authenticate(session: &mut ClusterSession, refused: Refusal) -> Refusal {
    session.closing = true;
    refused
}
