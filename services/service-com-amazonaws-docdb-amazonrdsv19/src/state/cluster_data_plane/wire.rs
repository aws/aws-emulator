// SPDX-License-Identifier: CC0-1.0

//! The WIRE one session on a cluster's endpoint runs on: the engine's own message framing.
//!
//! One of these runs per accepted connection. It reads messages off the connection, hands
//! each command document to the endpoint it serves ([`WireEndpoint`], which for an instance of
//! a cluster this service holds answers out of [`super::commands`]), and writes the reply back
//! in the form the message asked for. The core owns the socket, the TLS, the idle timeout and
//! the connection caps; this owns only the protocol.
//!
//! Two message kinds carry a command, and a driver uses both: `OP_MSG` (opcode 2013), which
//! is what every command of a current driver travels in, and `OP_QUERY` (opcode 2004) against
//! a `<db>.$cmd` collection, which is how a driver that does not yet know what it is talking
//! to sends its first handshake. A message of any other opcode is one this endpoint does not
//! serve, and the connection is closed rather than answered with something the client would
//! read as a result.
//!
//! # Bounds
//!
//! * a message longer than the endpoint publishes as `maxMessageSizeBytes`
//!   ([`super::commands::MAX_MESSAGE_SIZE_BYTES`]) is refused and the connection closed, so
//!   one client cannot make this process buffer without limit, and how many SECTIONS and
//!   hoisted DOCUMENTS one message may carry is bounded before any of them is read
//!   ([`MAX_MESSAGE_SECTIONS`], [`super::commands::MAX_WRITE_BATCH_SIZE`]);
//! * a command nested deeper than this plane walks is refused while the message is FRAMED,
//!   before the codec is handed it ([`raw_nests_within`] over the raw bytes, and
//!   [`super::query::MAX_NESTING_DEPTH`] again in [`answer`] as defence in depth), so the
//!   nesting a client chose bounds nothing but that walk;
//! * one command runs on a BLOCKING task under a wall-clock ceiling ([`COMMAND_TIMEOUT`]), so
//!   this connection's future stays pollable while it runs — the core's own idle timeout and
//!   its cancellation can land — and no command holds the cluster's documents for longer than
//!   that ceiling;
//! * every document inside a message is read by the BSON codec, so a length that does not
//!   match its content is a read error rather than a buffer walk;
//! * the connection's idle timeout, its lifetime and how many may be open are the CORE's,
//!   which owns the socket.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use ::bson::Document;
use ::emulator_server::{ListenerConn, NativeHandler};
use ::tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::commands;
use super::{fault, query, EndpointSocket};

/// The message header every message starts with: its length, its id, what it answers and its
/// opcode.
const HEADER_BYTES: usize = 16;

/// `OP_MSG`: the message every command of a current driver travels in.
const OP_MSG: i32 = 2013;

/// `OP_QUERY`: the message a driver's first handshake travels in, against `<db>.$cmd`.
const OP_QUERY: i32 = 2004;

/// `OP_REPLY`: what an `OP_QUERY` is answered with.
const OP_REPLY: i32 = 1;

/// `OP_MSG` flag bit 0: the message carries a trailing CRC-32C of itself.
const FLAG_CHECKSUM_PRESENT: u32 = 1;

/// `OP_MSG` flag bit 1: the sender is not waiting for a reply.
const FLAG_MORE_TO_COME: u32 = 1 << 1;

/// How many SECTIONS one `OP_MSG` may carry — the command document and the sequences hoisted
/// out of it, counted together.
///
/// CHOSEN as a process bound on the work one message schedules: a driver sends the command and
/// at most one such sequence (the `documents` of an insert, the `updates` of an update), and
/// each section costs this process a read whatever it carries.
const MAX_MESSAGE_SECTIONS: usize = 8;

/// The collection suffix an `OP_QUERY` command names: `<db>.$cmd`.
const COMMAND_COLLECTION_SUFFIX: &str = ".$cmd";

/// The longest one command may take before this endpoint stops waiting for it.
///
/// CHOSEN as a server-side ceiling, not a protocol limit: the engine's own `maxTimeMS` has no
/// default, so without one a command could hold a worker of the blocking pool and the
/// cluster's documents for as long as its own bounds allow. Every command's work is bounded
/// well inside this ([`super::documents`] and [`commands`] cap what one may read, write and
/// walk), so a command that reaches it is one this endpoint stops serving: the client is told
/// so and the connection is closed.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// What ONE endpoint served on this wire answers FROM: the state a connection on it holds
/// while it is open, and the reply to one command document.
///
/// The framing above is the engine's own and says nothing about which resource is behind the
/// address — the same messages carry a command to a cluster this service holds or to one of its
/// member instances ([`EndpointSocket`], the implementation beside it) and to any other resource
/// of this emulator whose clients speak this wire. So what an endpoint serves is this one seam,
/// and nothing of the protocol is written twice.
pub trait WireEndpoint: Clone + Send + Sync + 'static {
    /// The state ONE connection on this endpoint holds while it is open — who it has
    /// authenticated as, the cursors it holds — which travels through every message it sends.
    type Session: Send + 'static;

    /// A session for a connection just accepted.
    fn open_session(&self) -> Self::Session;

    /// Answer one command, as the reply document the client reads. `database` is the `$db` the
    /// message named.
    ///
    /// Called on a BLOCKING task, so it may take the locks the answer needs; it never awaits.
    fn answer(
        &self,
        session: &mut Self::Session,
        database: &str,
        command: &Document,
    ) -> Document;

    /// Whether the connection CARRIES ON once the reply just written is out.
    ///
    /// `true` for every endpoint that answers command after command on one connection, which
    /// is what this answers unless an endpoint says otherwise. An endpoint whose engine ENDS a
    /// session itself — one that closes the connection a failed authentication was attempted
    /// on, rather than letting the client keep trying on it — says so here, and the connection
    /// is closed after its refusal has been written.
    fn carries_on(&self, session: &Self::Session) -> bool {
        let _ = session;
        true
    }
}

/// The server one endpoint of this wire answers with.
pub struct MongoWire<E = EndpointSocket> {
    endpoint: E,
}

impl<E: WireEndpoint> MongoWire<E> {
    #[must_use]
    pub fn new(endpoint: E) -> Self {
        Self { endpoint }
    }
}

impl<E: WireEndpoint> NativeHandler for MongoWire<E> {
    fn serve(&self, conn: ListenerConn) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let socket = self.endpoint.clone();
        Box::pin(async move {
            let mut session = socket.open_session();
            let mut conn = conn;
            // One message at a time, until the client goes away or sends something this
            // endpoint does not serve. The session travels THROUGH each message, because
            // answering one hands it to a blocking task and takes it back.
            while let Some(carried) = serve_one(&socket, session, &mut conn).await {
                session = carried;
            }
        })
    }
}

/// Read one message and answer it, handing back the session for the next one — and `None`
/// where the connection does not carry on.
async fn serve_one<E: WireEndpoint>(
    socket: &E,
    session: E::Session,
    conn: &mut ListenerConn,
) -> Option<E::Session> {
    let mut header = [0u8; HEADER_BYTES];
    conn.read_exact(&mut header).await.ok()?;
    let length = i32::from_le_bytes([header[0], header[1], header[2], header[3]]);
    let request_id = i32::from_le_bytes([header[4], header[5], header[6], header[7]]);
    let opcode = i32::from_le_bytes([header[12], header[13], header[14], header[15]]);
    let length = usize::try_from(length).ok()?;
    if length < HEADER_BYTES || length > commands::MAX_MESSAGE_SIZE_BYTES as usize {
        return None;
    }
    let mut body = vec![0u8; length - HEADER_BYTES];
    conn.read_exact(&mut body).await.ok()?;
    match opcode {
        OP_MSG => {
            let (command, database, flags) = read_op_msg(&body)?;
            let (reply, session) = answer(socket, session, database, command, conn).await?;
            // A client that said it is not waiting for a reply gets none, which is what
            // makes an unacknowledged write one round trip.
            if flags & FLAG_MORE_TO_COME != 0 {
                return carries_on(socket, session);
            }
            if !write_message(conn, request_id, OP_MSG, &op_msg_body(&reply)).await {
                return None;
            }
            carries_on(socket, session)
        }
        OP_QUERY => {
            let (command, database) = read_op_query(&body)?;
            let (reply, session) = answer(socket, session, database, command, conn).await?;
            if !write_message(conn, request_id, OP_REPLY, &op_reply_body(&reply)).await {
                return None;
            }
            carries_on(socket, session)
        }
        // Every other opcode is one this endpoint does not serve — the legacy write
        // messages, which carry no reply at all, and a compressed message, which this
        // endpoint never negotiates. Closing is what the engine does with a message it
        // cannot read, and is the only honest answer: a reply document would be read as the
        // result of an operation nothing ran.
        _ => None,
    }
}

/// The session, for a connection the endpoint carries on serving ([`WireEndpoint::carries_on`])
/// — and `None` for one it has ended, which closes the connection once its reply is out.
fn carries_on<E: WireEndpoint>(socket: &E, session: E::Session) -> Option<E::Session> {
    socket.carries_on(&session).then_some(session)
}

/// Answer one command out of the cluster's documents, with the endpoint's facts as they are
/// at this instant, handing back the session for the next message.
///
/// A command nested deeper than the plane walks is refused HERE, before any of the work it
/// asks for: the nesting is the client's, so the bound precedes the walk rather than being
/// discovered inside it ([`query::MAX_NESTING_DEPTH`], which every reader and writer under
/// this module holds to as well).
///
/// The command itself runs on a BLOCKING task: answering one reads and writes the cluster's
/// documents under their lock and never awaits, so running it on the worker that polls this
/// connection would keep the core from polling anything else about it — its idle timeout and
/// the cancellation that severs it when the listener is dropped — for as long as the command
/// took. On the blocking pool, this connection's future stays pollable, and the command is
/// bounded by [`COMMAND_TIMEOUT`] besides: a command that reaches it is answered as one this
/// endpoint stopped serving and the connection is closed, because the work it left behind
/// still holds the session it was handed.
///
/// `None` where the connection does not carry on: a task that panicked, or a command that ran
/// past the ceiling.
async fn answer<E: WireEndpoint>(
    socket: &E,
    session: E::Session,
    database: String,
    command: Document,
    conn: &mut ListenerConn,
) -> Option<(Document, E::Session)> {
    if !query::document_nests_within(&command, query::MAX_NESTING_DEPTH) {
        let refusal = fault::bad_value(format!(
            "the command exceeds the maximum nesting depth of {}",
            query::MAX_NESTING_DEPTH
        ))
        .to_reply();
        return Some((refusal, session));
    }
    let socket = socket.clone();
    let executing = ::tokio::task::spawn_blocking(move || {
        let mut session = session;
        let reply = socket.answer(&mut session, &database, &command);
        (reply, session)
    });
    match ::tokio::time::timeout(COMMAND_TIMEOUT, executing).await {
        Ok(Ok((reply, session))) => Some((reply, session)),
        // A task that panicked served no command, and its session went with it: the
        // connection ends rather than answering out of a state nobody holds.
        Ok(Err(_panicked)) => None,
        Err(_elapsed) => {
            let refusal = fault::timed_out(COMMAND_TIMEOUT.as_millis()).to_reply();
            write_message(conn, 0, OP_MSG, &op_msg_body(&refusal)).await;
            None
        }
    }
}

/// The command document, the `$db` it names and the message flags of an `OP_MSG` body.
fn read_op_msg(body: &[u8]) -> Option<(Document, String, u32)> {
    let flags = u32::from_le_bytes([
        *body.first()?,
        *body.get(1)?,
        *body.get(2)?,
        *body.get(3)?,
    ]);
    let sections = match flags & FLAG_CHECKSUM_PRESENT != 0 {
        // The trailing CRC-32C is not read: the connection is a stream this process already
        // trusts for its framing, and a mismatch would be a transport fault the core owns.
        true => body.get(4..body.len().checked_sub(4)?)?,
        false => body.get(4..)?,
    };
    let mut command: Option<Document> = None;
    let mut sequences: Vec<(String, Vec<::bson::Bson>)> = Vec::new();
    let mut hoisted = 0_usize;
    let mut sections_read = 0_usize;
    let mut at = 0_usize;
    while at < sections.len() {
        // How many SECTIONS one message may carry is bounded before any of them is read: a
        // driver sends the command document and at most one hoisted sequence, and the cheapest
        // well-formed section costs a handful of bytes, so without this a message at the
        // published size ceiling would schedule millions of them.
        sections_read += 1;
        if sections_read > MAX_MESSAGE_SECTIONS {
            return None;
        }
        let kind = *sections.get(at)?;
        at += 1;
        match kind {
            // Kind 0: the command document itself.
            0 => {
                let (document, read) = read_document(sections.get(at..)?)?;
                at += read;
                command = Some(document);
            }
            // Kind 1: a sequence of documents hoisted out of the command, under the name the
            // section carries — which is how a driver sends the `documents` of an insert.
            1 => {
                let size = usize::try_from(i32::from_le_bytes([
                    *sections.get(at)?,
                    *sections.get(at + 1)?,
                    *sections.get(at + 2)?,
                    *sections.get(at + 3)?,
                ]))
                .ok()?;
                if size < 4 {
                    return None;
                }
                let section = sections.get(at + 4..at.checked_add(size)?)?;
                at += size;
                let end = section.iter().position(|byte| *byte == 0)?;
                let identifier = std::str::from_utf8(section.get(..end)?).ok()?.to_string();
                let mut rest = section.get(end + 1..)?;
                let mut documents: Vec<::bson::Bson> = Vec::new();
                while !rest.is_empty() {
                    // And how many DOCUMENTS all the sections together may hoist: the same
                    // ceiling the command itself is held to, so a hoisted batch is no larger
                    // than one the command could carry inline.
                    hoisted += 1;
                    if hoisted > commands::MAX_WRITE_BATCH_SIZE as usize {
                        return None;
                    }
                    let (document, read) = read_document(rest)?;
                    documents.push(::bson::Bson::Document(document));
                    rest = rest.get(read..)?;
                }
                sequences.push((identifier, documents));
            }
            _ => return None,
        }
    }
    let mut command = command?;
    for (identifier, documents) in sequences {
        command.insert(identifier, ::bson::Bson::Array(documents));
    }
    let database = command
        .get_str("$db")
        .map(str::to_string)
        .unwrap_or_else(|_| commands::ADMIN_DATABASE.to_string());
    Some((command, database, flags))
}

/// The command document and the database of an `OP_QUERY` body.
fn read_op_query(body: &[u8]) -> Option<(Document, String)> {
    // int32 flags, cstring fullCollectionName, int32 numberToSkip, int32 numberToReturn,
    // then the query document.
    let after_flags = body.get(4..)?;
    let end = after_flags.iter().position(|byte| *byte == 0)?;
    let full_collection = std::str::from_utf8(after_flags.get(..end)?).ok()?;
    let database = full_collection
        .strip_suffix(COMMAND_COLLECTION_SUFFIX)
        .unwrap_or(commands::ADMIN_DATABASE)
        .to_string();
    let after_name = after_flags.get(end + 1..)?;
    let (command, _) = read_document(after_name.get(8..)?)?;
    Some((command, database))
}

/// One length-prefixed BSON document at the front of `bytes`, and how many bytes it took.
///
/// The nesting bound is applied to the RAW BYTES first ([`raw_nests_within`]): the BSON codec
/// materialises a nested document by recursion, so a message nested deeper than this plane
/// walks has to be refused BEFORE it is decoded rather than after. A message this refuses is
/// one the connection ends on, which is what the engine does with a message it cannot read.
fn read_document(bytes: &[u8]) -> Option<(Document, usize)> {
    let length = usize::try_from(i32::from_le_bytes([
        *bytes.first()?,
        *bytes.get(1)?,
        *bytes.get(2)?,
        *bytes.get(3)?,
    ]))
    .ok()?;
    if length < 5 {
        return None;
    }
    let document = bytes.get(..length)?;
    if !raw_nests_within(document, query::MAX_NESTING_DEPTH)? {
        return None;
    }
    let decoded = Document::from_reader(&mut std::io::Cursor::new(document)).ok()?;
    Some((decoded, length))
}

/// Whether the BSON document `bytes` holds nests no deeper than `limit`, decided by WALKING
/// THE RAW BYTES with an explicit stack of the open documents — so the answer costs no stack
/// of its own, whatever a client sent.
///
/// `None` for bytes this walk cannot read as BSON at all: a length that does not fit its
/// content, an element type this endpoint does not accept, or JavaScript-with-scope, whose own
/// nested document the codec would recurse into. Such a message is not decoded.
fn raw_nests_within(bytes: &[u8], limit: usize) -> Option<bool> {
    // The end offset of each document still open, innermost last; its own terminating byte is
    // the last one before that offset.
    let mut open: Vec<usize> = vec![read_length(bytes, 0).filter(|end| *end <= bytes.len())?];
    let mut at = 4_usize;
    while let Some(&end) = open.last() {
        if open.len() > limit {
            return Some(false);
        }
        if at >= end {
            return None;
        }
        let kind = *bytes.get(at)?;
        at += 1;
        // A zero where an element type is expected terminates the document that is open.
        if kind == 0 {
            open.pop();
            continue;
        }
        // The element's name, a NUL-terminated string.
        at = bytes.get(at..end)?.iter().position(|byte| *byte == 0)? + at + 1;
        match kind {
            // double, UTC datetime, timestamp, int64
            0x01 | 0x09 | 0x11 | 0x12 => at += 8,
            // string, JavaScript, symbol: an int32 length that counts the trailing NUL
            0x02 | 0x0D | 0x0E => at += 4 + read_length(bytes, at)?,
            // an embedded document or array: one level deeper, and its own length says where
            // it ends
            0x03 | 0x04 => {
                let child = at + read_length(bytes, at).filter(|length| *length >= 5)?;
                if child > end {
                    return None;
                }
                open.push(child);
                at += 4;
            }
            // binary: an int32 length, a subtype byte, then the bytes
            0x05 => at += 4 + 1 + read_length(bytes, at)?,
            // undefined, null, max key, min key: no bytes of their own
            0x06 | 0x0A | 0x7F | 0xFF => {}
            0x07 => at += 12,
            0x08 => at += 1,
            // a regular expression: two NUL-terminated strings
            0x0B => {
                for _ in 0..2 {
                    at = bytes.get(at..end)?.iter().position(|byte| *byte == 0)? + at + 1;
                }
            }
            // a DB pointer: a string and an object id
            0x0C => at += 4 + read_length(bytes, at)? + 12,
            0x10 => at += 4,
            0x13 => at += 16,
            _ => return None,
        }
        if at > end {
            return None;
        }
    }
    Some(true)
}

/// The little-endian int32 at `at`, as a length.
fn read_length(bytes: &[u8], at: usize) -> Option<usize> {
    usize::try_from(i32::from_le_bytes([
        *bytes.get(at)?,
        *bytes.get(at + 1)?,
        *bytes.get(at + 2)?,
        *bytes.get(at + 3)?,
    ]))
    .ok()
}

/// An `OP_MSG` body carrying one reply document.
fn op_msg_body(reply: &Document) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&0_u32.to_le_bytes());
    body.push(0);
    append_document(&mut body, reply);
    body
}

/// An `OP_REPLY` body carrying one reply document, which is what an `OP_QUERY` is answered
/// with.
fn op_reply_body(reply: &Document) -> Vec<u8> {
    let mut body = Vec::new();
    // responseFlags, cursorID, startingFrom, numberReturned.
    body.extend_from_slice(&0_i32.to_le_bytes());
    body.extend_from_slice(&0_i64.to_le_bytes());
    body.extend_from_slice(&0_i32.to_le_bytes());
    body.extend_from_slice(&1_i32.to_le_bytes());
    append_document(&mut body, reply);
    body
}

/// Append a document's BSON to a buffer.
fn append_document(buffer: &mut Vec<u8>, document: &Document) {
    if document.to_writer(&mut *buffer).is_err() {
        // A reply that cannot be encoded is not one the client can read either. An empty
        // document is appended instead, which a client reads as a reply carrying no `ok` and
        // treats as a protocol error rather than as a success.
        buffer.extend_from_slice(&5_i32.to_le_bytes());
        buffer.push(0);
    }
}

/// Write one message with its header, and report whether the connection carries on.
async fn write_message(
    conn: &mut ListenerConn,
    response_to: i32,
    opcode: i32,
    body: &[u8],
) -> bool {
    let length = i32::try_from(HEADER_BYTES + body.len()).unwrap_or(i32::MAX);
    let mut message = Vec::with_capacity(HEADER_BYTES + body.len());
    message.extend_from_slice(&length.to_le_bytes());
    // This endpoint answers rather than initiates, so its own request id is the id of the
    // message it answers, which is what a client correlates on.
    message.extend_from_slice(&response_to.to_le_bytes());
    message.extend_from_slice(&response_to.to_le_bytes());
    message.extend_from_slice(&opcode.to_le_bytes());
    message.extend_from_slice(body);
    if conn.write_all(&message).await.is_err() {
        return false;
    }
    conn.flush().await.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::bson::doc;

    /// RULE: an `OP_MSG` carries its command in a kind-0 section, and the documents a write
    /// hoists into a kind-1 sequence belong to that same command.
    #[test]
    fn an_op_msg_is_read_with_its_hoisted_documents() {
        let command = doc! { "insert": "widgets", "$db": "shop" };
        let hoisted = doc! { "_id": 1_i32 };
        let mut body = Vec::new();
        body.extend_from_slice(&0_u32.to_le_bytes());
        body.push(0);
        command.to_writer(&mut body).expect("the command encodes");
        // The kind-1 section: its own size, the identifier, then the documents.
        let mut section = Vec::new();
        section.extend_from_slice(b"documents\0");
        hoisted.to_writer(&mut section).expect("the document encodes");
        body.push(1);
        body.extend_from_slice(
            &i32::try_from(section.len() + 4)
                .expect("the section is small")
                .to_le_bytes(),
        );
        body.extend_from_slice(&section);

        let (read, database, flags) = read_op_msg(&body).expect("the message is well formed");
        assert_eq!(database, "shop", "expected the $db of the command, got {database}");
        assert_eq!(flags, 0, "expected no flags, got {flags}");
        assert_eq!(
            read.get("documents"),
            Some(&::bson::Bson::Array(vec![::bson::Bson::Document(hoisted)])),
            "expected the hoisted documents on the command, got {read:?}"
        );
    }

    /// RULE: an `OP_QUERY` against `<db>.$cmd` carries a command in that database.
    #[test]
    fn an_op_query_names_its_database() {
        let command = doc! { "isMaster": 1_i32 };
        let mut body = Vec::new();
        body.extend_from_slice(&0_i32.to_le_bytes());
        body.extend_from_slice(b"admin.$cmd\0");
        body.extend_from_slice(&0_i32.to_le_bytes());
        body.extend_from_slice(&(-1_i32).to_le_bytes());
        command.to_writer(&mut body).expect("the command encodes");
        let (read, database) = read_op_query(&body).expect("the message is well formed");
        assert_eq!(database, "admin", "expected the admin database, got {database}");
        assert_eq!(read, command, "expected the command back, got {read:?}");
    }

    /// RULE: a truncated or otherwise unreadable message is not read as a command, and
    /// neither is one carrying more sections than this endpoint reads.
    #[test]
    fn a_truncated_message_is_not_a_command() {
        for (case, body) in [
            ("no flags", Vec::new()),
            ("no section", 0_u32.to_le_bytes().to_vec()),
            (
                "a section kind this endpoint does not serve",
                [0_u32.to_le_bytes().as_slice(), &[7_u8]].concat(),
            ),
        ] {
            assert!(
                read_op_msg(&body).is_none(),
                "case {case}: expected the message to be unreadable"
            );
        }
        // One hoisted sequence past the bound: the cheapest well-formed section there is, so
        // the bound is what stops it rather than its size.
        let empty_section = {
            let mut section = vec![1_u8];
            section.extend_from_slice(&5_i32.to_le_bytes());
            section.push(0);
            section
        };
        let mut body = 0_u32.to_le_bytes().to_vec();
        body.push(0);
        doc! { "insert": "widgets", "$db": "shop" }
            .to_writer(&mut body)
            .expect("the command encodes");
        for _ in 0..MAX_MESSAGE_SECTIONS + 1 {
            body.extend_from_slice(&empty_section);
        }
        assert!(
            read_op_msg(&body).is_none(),
            "expected a message carrying more than {MAX_MESSAGE_SECTIONS} sections to be \
             refused"
        );
    }

    /// RULE: the nesting bound is applied to the RAW BYTES, before the codec is handed them —
    /// so a document nested deeper than this plane walks is never decoded.
    ///
    /// The rows span what the walk has to read to answer: an ordinary command, a document at
    /// the bound, one past it, one whose nesting is inside an ARRAY rather than a document,
    /// and the element types a command carries beside them.
    #[test]
    fn nesting_past_the_bound_is_refused_before_the_codec_sees_it() {
        /// The bytes of one document, as a client sends it.
        fn encoded(document: &Document) -> Vec<u8> {
            let mut bytes = Vec::new();
            document.to_writer(&mut bytes).expect("the document encodes");
            bytes
        }
        /// A document nested `levels` deep, `nested` in a document or in an array.
        fn deep(levels: usize, in_array: bool) -> Document {
            let mut value = ::bson::Bson::Int32(1);
            for _ in 1..levels {
                value = match in_array {
                    true => ::bson::Bson::Array(vec![value]),
                    false => ::bson::Bson::Document(doc! { "a": value }),
                };
            }
            doc! { "a": value }
        }
        let rows: Vec<(&str, Document, bool)> = vec![
            (
                "an ordinary command, with the element types one carries",
                doc! {
                    "insert": "widgets",
                    "ordered": true,
                    "documents": [{ "_id": 1_i32, "at": ::bson::DateTime::now(), "n": 2_i64 }],
                    "comment": "a string",
                    "nothing": ::bson::Bson::Null,
                    "id": ::bson::oid::ObjectId::new(),
                    "size": 1.5_f64,
                    "raw": ::bson::Bson::Binary(::bson::Binary {
                        subtype: ::bson::spec::BinarySubtype::Generic,
                        bytes: vec![1, 2, 3],
                    }),
                },
                true,
            ),
            ("a document at the bound", deep(query::MAX_NESTING_DEPTH, false), true),
            ("a document one level past it", deep(query::MAX_NESTING_DEPTH + 1, false), false),
            ("arrays one level past it", deep(query::MAX_NESTING_DEPTH + 1, true), false),
        ];
        for (case, document, expected) in rows {
            let bytes = encoded(&document);
            let walked = raw_nests_within(&bytes, query::MAX_NESTING_DEPTH);
            assert_eq!(
                walked,
                Some(expected),
                "case {case}: expected the raw walk to answer {expected}, got {walked:?}"
            );
            let read = read_document(&bytes).map(|(document, _)| document);
            assert_eq!(
                read.is_some(),
                expected,
                "case {case}: expected the document to be read {expected}, got {read:?}"
            );
        }
    }
}
