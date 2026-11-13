// SPDX-License-Identifier: CC0-1.0

//! The git smart-HTTP transport behind a CodeCommit repository, driven over the
//! wire through the router that serves the real emulator.
//!
//! The rules under test:
//!
//! 1. `RepositoryMetadata.cloneUrlHttp` names an address THIS emulator serves the
//!    git transport at, and that URL — not a rewritten one — is the address the git
//!    transport answers on (`info/refs`, `git-upload-pack`, `git-receive-pack`).
//! 2. A push is stored as git objects and projected into the control plane: the
//!    commits and tags a client pushes are readable through `GetBranch`/`GetFile`
//!    at the ids the CLIENT computed.
//! 3. A fetch serves the complete object closure of the refs asked for,
//!    whichever face wrote them — a push or `PutFile`.
//! 4. A push whose declared old value is not where the ref now stands is refused
//!    by the server and leaves the ref unmoved.
//!
//! Every case here builds its git objects, packfiles and pkt-lines in this file,
//! with the names computed from the bytes, so the emulator is checked against
//! git's wire format rather than against itself. The tables span what the rules
//! generalise over: file count, path depth, file mode, content kind and size,
//! branch and tag naming, commit depth, and the endpoint the request arrives at.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use sha1::Digest;
use smithy_server_runtime::MultiServiceRouter;

// The control-plane harness every suite in this target shares: one definition of
// "the emulator", of a signed call, and of how a failed call's code is read, so
// this suite and its siblings cannot drift about them. Only the GIT-specific
// builders (pkt-lines, packfiles, objects) are local to this file.
use crate::repository_lifecycle::{
    call, emulator, emulator_with_engine, error as error_code, from_base64, ok, string_at,
};

/// The authority the shared harness addresses the emulator on, and so the one
/// every URL the emulator announces for itself must name (`crate::wire`).
const ENDPOINT: &str = "codecommit.us-west-2.amazonaws.com";

/// The all-zero object name: "no object" in a ref update command.
const NULL_ID: &str = "0000000000000000000000000000000000000000";

// ----------------------------------------------------------------- the emulator

/// The base64 blob content of a response, read through the shared decoder.
fn content_of(payload: &Value) -> Vec<u8> {
    from_base64(&string_at(payload, "/fileContent"))
}

/// One git transport response: status, `content-type` and body.
struct GitReply {
    status: u16,
    content_type: String,
    body: Vec<u8>,
}

/// Send a git transport request to `url` exactly as a git client would: no SigV4
/// credential, no `X-Amz-Target`, the `Host` of the URL it was given, and the git
/// credentials of a caller as HTTP Basic ones — which is how a git client
/// authenticates to CodeCommit, and what the transport challenges a request that
/// carries none for.
async fn git(
    router: &MultiServiceRouter,
    method: &str,
    url: &str,
    content_type: Option<&str>,
    body: Vec<u8>,
) -> GitReply {
    git_as(router, method, url, content_type, body, Some(GIT_USER)).await
}

/// The user name of the git credentials the cases here present. The account holds no
/// record of it, so it speaks for no IAM identity and no identity-based policy is in
/// force on it — the position the account's own credentials are in.
const GIT_USER: &str = "git-user-at-000000000000";

/// The `Authorization` header a git client sends once it has been challenged:
/// Basic credentials whose user name is the git credential's.
fn basic(user: &str) -> String {
    use base64::Engine as _;
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{user}:password"))
    )
}

/// [`git`], with the git credentials the request presents named — `None` for a
/// request that presents none at all.
async fn git_as(
    router: &MultiServiceRouter,
    method: &str,
    url: &str,
    content_type: Option<&str>,
    body: Vec<u8>,
    user: Option<&str>,
) -> GitReply {
    let mut request = http::Request::builder()
        .method(method)
        .uri(url)
        .header("host", ENDPOINT)
        .header("user-agent", "git/2.43.0")
        .header("accept", "*/*");
    if let Some(user) = user {
        request = request.header("authorization", basic(user));
    }
    if let Some(content_type) = content_type {
        request = request.header("content-type", content_type);
    }
    let response = router
        .dispatch(
            request
                .body(bytes::Bytes::from(body))
                .expect("a well-formed request"),
        )
        .await;
    GitReply {
        status: response.status().as_u16(),
        content_type: response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string(),
        body: response.body().to_vec(),
    }
}

async fn advertise(router: &MultiServiceRouter, clone_url: &str, service: &str) -> GitReply {
    git(
        router,
        "GET",
        &format!("{clone_url}/info/refs?service={service}"),
        None,
        Vec::new(),
    )
    .await
}

async fn rpc(router: &MultiServiceRouter, clone_url: &str, service: &str, body: Vec<u8>) -> GitReply {
    git(
        router,
        "POST",
        &format!("{clone_url}/{service}"),
        Some(&format!("application/x-{service}-request")),
        body,
    )
    .await
}

/// Assert that the clone URL `created` announced for `repository` is the address this
/// emulator serves git at, in the FORM the announcement has to take — the git
/// listener's own loopback authority under the scheme it speaks in the clear, with the
/// repository on the path — and that `GetRepository` reports the same value.
async fn assert_announced_git_url(
    router: &MultiServiceRouter,
    label: &str,
    repository: &str,
    clone_url: &str,
) {
    let (scheme, rest) = clone_url
        .split_once("://")
        .unwrap_or_else(|| panic!("case {label:?}: the clone URL names no origin: {clone_url:?}"));
    assert_eq!(
        scheme, "http",
        "case {label:?}: the git address is served in the clear, so a stock client dials \
         it with no trust material: {clone_url:?}"
    );
    let (authority, path) = rest
        .split_once('/')
        .unwrap_or_else(|| panic!("case {label:?}: the clone URL names no path: {clone_url:?}"));
    assert_eq!(
        path,
        format!("v1/repos/{repository}"),
        "case {label:?}: the clone URL's path: {clone_url:?}"
    );
    let (host, port) = authority
        .rsplit_once(':')
        .unwrap_or_else(|| panic!("case {label:?}: the clone URL names no port: {clone_url:?}"));
    assert_eq!(
        host, "127.0.0.1",
        "case {label:?}: the git listener's address is this machine's own: {clone_url:?}"
    );
    assert!(
        port.parse::<u16>().is_ok_and(|port| port > 0),
        "case {label:?}: the clone URL names the port the listener bound: {clone_url:?}"
    );
    let read = ok(
        router,
        "GetRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;
    assert_eq!(
        string_at(&read, "/repositoryMetadata/cloneUrlHttp"),
        clone_url,
        "case {label:?}: every read announces the one address this emulator serves git at"
    );
}

// ------------------------------------------------------------------- git objects

/// A git object as a client builds it: its kind and its payload.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Object {
    kind: &'static str,
    data: Vec<u8>,
}

/// git's object name function: `SHA-1("<kind> <len>\0" + payload)`.
fn id_of(object: &Object) -> String {
    let mut hasher = sha1::Sha1::new();
    hasher.update(format!("{} {}\0", object.kind, object.data.len()).as_bytes());
    hasher.update(&object.data);
    hex::encode(hasher.finalize())
}

fn blob(content: &[u8]) -> Object {
    Object {
        kind: "blob",
        data: content.to_vec(),
    }
}

/// A tree object over `(mode, name, id)` entries, in git's tree entry order (a
/// subtree's name compares as though it ended in `/`).
fn tree(entries: &[(String, String, String)]) -> Object {
    let mut sorted = entries.to_vec();
    sorted.sort_by_key(|(mode, name, _)| {
        if mode == "40000" {
            format!("{name}/")
        } else {
            name.clone()
        }
    });
    let mut data = Vec::new();
    for (mode, name, id) in sorted {
        data.extend_from_slice(mode.as_bytes());
        data.push(b' ');
        data.extend_from_slice(name.as_bytes());
        data.push(0);
        data.extend_from_slice(&hex::decode(&id).expect("an object name is hex"));
    }
    Object {
        kind: "tree",
        data,
    }
}

/// A commit object over a tree, its parents and a message.
fn commit(tree_id: &str, parents: &[String], message: &str) -> Object {
    let mut text = format!("tree {tree_id}\n");
    for parent in parents {
        text.push_str(&format!("parent {parent}\n"));
    }
    text.push_str("author Pusher <pusher@example.com> 1770000000 +0000\n");
    text.push_str("committer Pusher <pusher@example.com> 1770000060 +0000\n");
    text.push('\n');
    text.push_str(message);
    Object {
        kind: "commit",
        data: text.into_bytes(),
    }
}

/// An annotated tag object naming `target`.
fn annotated_tag(target: &str, name: &str, message: &str) -> Object {
    let text = format!(
        "object {target}\ntype commit\ntag {name}\n\
         tagger Pusher <pusher@example.com> 1770000120 +0000\n\n{message}"
    );
    Object {
        kind: "tag",
        data: text.into_bytes(),
    }
}

/// The entries of a tree object: `(mode, name, id)`.
fn parse_tree(data: &[u8]) -> Vec<(String, String, String)> {
    let mut entries = Vec::new();
    let mut rest = data;
    while !rest.is_empty() {
        let space = rest.iter().position(|b| *b == b' ').expect("a tree mode");
        let nul = rest.iter().position(|b| *b == 0).expect("a tree name");
        let mode = String::from_utf8_lossy(&rest[..space]).to_string();
        let name = String::from_utf8_lossy(&rest[space + 1..nul]).to_string();
        let id = hex::encode(&rest[nul + 1..nul + 21]);
        entries.push((mode, name, id));
        rest = &rest[nul + 21..];
    }
    entries
}

/// The `tree` header of a commit object.
fn commit_tree(data: &[u8]) -> String {
    let text = String::from_utf8_lossy(data);
    text.lines()
        .find_map(|line| line.strip_prefix("tree ").map(str::to_string))
        .expect("a commit names a tree")
}

/// Build the tree objects for a whole snapshot, reporting the root's name and
/// every object (trees and blobs) the snapshot is made of.
fn snapshot_objects(files: &BTreeMap<String, (String, Vec<u8>)>) -> (String, Vec<Object>) {
    let mut objects: Vec<Object> = Vec::new();
    for (_, content) in files.values() {
        objects.push(blob(content));
    }
    let root = build_tree(files, &mut objects);
    (root, objects)
}

fn build_tree(files: &BTreeMap<String, (String, Vec<u8>)>, objects: &mut Vec<Object>) -> String {
    let mut entries: Vec<(String, String, String)> = Vec::new();
    let mut directories: BTreeMap<String, BTreeMap<String, (String, Vec<u8>)>> = BTreeMap::new();
    for (path, (mode, content)) in files {
        match path.split_once('/') {
            None => entries.push((mode.clone(), path.clone(), id_of(&blob(content)))),
            Some((head, rest)) => {
                directories
                    .entry(head.to_string())
                    .or_default()
                    .insert(rest.to_string(), (mode.clone(), content.clone()));
            }
        }
    }
    for (name, subtree) in &directories {
        let id = build_tree(subtree, objects);
        entries.push(("40000".to_string(), name.clone(), id));
    }
    let object = tree(&entries);
    let id = id_of(&object);
    objects.push(object);
    id
}

// --------------------------------------------------------------------- packfiles

/// A one-entry packfile whose header DECLARES `declared` bytes while the entry
/// inflates to `object.data` — the shape of a pack that asks the server to
/// reserve far more than it sends.
fn pack_with_declared_size(object: &Object, declared: usize) -> Vec<u8> {
    use std::io::Write as _;
    let mut out: Vec<u8> = b"PACK".to_vec();
    out.extend_from_slice(&2u32.to_be_bytes());
    out.extend_from_slice(&1u32.to_be_bytes());
    let pack_type: u8 = match object.kind {
        "commit" => 1,
        "tree" => 2,
        "blob" => 3,
        "tag" => 4,
        other => panic!("no pack type for {other}"),
    };
    let mut size = declared;
    let mut byte = (pack_type << 4) | (size & 0x0f) as u8;
    size >>= 4;
    while size > 0 {
        out.push(byte | 0x80);
        byte = (size & 0x7f) as u8;
        size >>= 7;
    }
    out.push(byte);
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&object.data).expect("deflate into a vec");
    out.extend_from_slice(&encoder.finish().expect("deflate finishes"));
    let mut hasher = sha1::Sha1::new();
    hasher.update(&out);
    out.extend_from_slice(&hasher.finalize());
    out
}

/// A packfile carrying `objects`, each whole (no deltas) — the simplest form a
/// client may send.
fn pack(objects: &[Object]) -> Vec<u8> {
    use std::io::Write as _;
    let mut out: Vec<u8> = b"PACK".to_vec();
    out.extend_from_slice(&2u32.to_be_bytes());
    out.extend_from_slice(&(objects.len() as u32).to_be_bytes());
    for object in objects {
        let pack_type: u8 = match object.kind {
            "commit" => 1,
            "tree" => 2,
            "blob" => 3,
            "tag" => 4,
            other => panic!("no pack type for {other}"),
        };
        let mut size = object.data.len();
        let mut byte = (pack_type << 4) | (size & 0x0f) as u8;
        size >>= 4;
        while size > 0 {
            out.push(byte | 0x80);
            byte = (size & 0x7f) as u8;
            size >>= 7;
        }
        out.push(byte);
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&object.data).expect("deflate into a vec");
        out.extend_from_slice(&encoder.finish().expect("deflate finishes"));
    }
    let mut hasher = sha1::Sha1::new();
    hasher.update(&out);
    out.extend_from_slice(&hasher.finalize());
    out
}

/// A git delta that copies the whole of `base` and then inserts the tail of
/// `target`, which is the shape git's own delta compression produces for an
/// appended file. `target` must begin with `base`.
fn delta(base: &[u8], target: &[u8]) -> Vec<u8> {
    assert!(
        target.starts_with(base),
        "this delta builder only appends to its base"
    );
    let mut out = Vec::new();
    for size in [base.len(), target.len()] {
        let mut remaining = size;
        loop {
            let byte = (remaining & 0x7f) as u8;
            remaining >>= 7;
            if remaining == 0 {
                out.push(byte);
                break;
            }
            out.push(byte | 0x80);
        }
    }
    // One copy instruction with every offset and size byte present (0x80 marks a
    // copy, the low nibble the four offset bytes, bits 4-6 the three size bytes).
    out.push(0xff);
    out.extend_from_slice(&(0u32).to_le_bytes());
    let size = base.len() as u32;
    out.extend_from_slice(&size.to_le_bytes()[..3]);
    // Then insert instructions for the tail, each carrying at most 127 bytes.
    for chunk in target[base.len()..].chunks(127) {
        out.push(chunk.len() as u8);
        out.extend_from_slice(chunk);
    }
    out
}

/// git's offset encoding for an offset-delta's distance back to its base.
fn offset_bytes(mut distance: usize) -> Vec<u8> {
    let mut bytes = vec![(distance & 0x7f) as u8];
    distance >>= 7;
    while distance > 0 {
        distance -= 1;
        bytes.insert(0, 0x80 | (distance & 0x7f) as u8);
        distance >>= 7;
    }
    bytes
}

/// One entry of a packfile: a whole object, or a delta against an object named
/// directly (a thin pack) or against an earlier entry of the same pack.
enum Entry {
    Whole(Object),
    /// A delta whose base is named directly: `(base id, delta)`.
    Reference(String, Vec<u8>),
    /// A delta against the entry at `base` in this pack, by its index.
    Offset(usize, Vec<u8>),
}

/// A packfile built from entries, so a test can send the delta forms a real git
/// client sends.
fn pack_entries(entries: &[Entry]) -> Vec<u8> {
    use std::io::Write as _;
    let mut out: Vec<u8> = b"PACK".to_vec();
    out.extend_from_slice(&2u32.to_be_bytes());
    out.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    let mut offsets: Vec<usize> = Vec::new();
    for entry in entries {
        offsets.push(out.len());
        let (pack_type, size, payload, tail): (u8, usize, &[u8], Vec<u8>) = match entry {
            Entry::Whole(object) => {
                let pack_type = match object.kind {
                    "commit" => 1,
                    "tree" => 2,
                    "blob" => 3,
                    "tag" => 4,
                    other => panic!("no pack type for {other}"),
                };
                (pack_type, object.data.len(), &object.data, Vec::new())
            }
            Entry::Reference(base, delta) => (
                7,
                delta.len(),
                delta.as_slice(),
                hex::decode(base).expect("an object name is hex"),
            ),
            Entry::Offset(base, delta) => {
                let distance = out.len() - offsets[*base];
                (6, delta.len(), delta.as_slice(), offset_bytes(distance))
            }
        };
        let mut remaining = size;
        let mut byte = (pack_type << 4) | (remaining & 0x0f) as u8;
        remaining >>= 4;
        while remaining > 0 {
            out.push(byte | 0x80);
            byte = (remaining & 0x7f) as u8;
            remaining >>= 7;
        }
        out.push(byte);
        out.extend_from_slice(&tail);
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(payload).expect("deflate into a vec");
        out.extend_from_slice(&encoder.finish().expect("deflate finishes"));
    }
    let mut hasher = sha1::Sha1::new();
    hasher.update(&out);
    out.extend_from_slice(&hasher.finalize());
    out
}

/// Read a packfile the emulator served, reporting its objects.
fn unpack(bytes: &[u8]) -> Vec<Object> {
    assert_eq!(&bytes[..4], b"PACK", "a packfile starts with PACK");
    assert_eq!(
        u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
        2,
        "the emulator serves pack version 2"
    );
    let count = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
    let mut hasher = sha1::Sha1::new();
    hasher.update(&bytes[..bytes.len() - 20]);
    assert_eq!(
        hex::encode(hasher.finalize()),
        hex::encode(&bytes[bytes.len() - 20..]),
        "the packfile's trailing checksum must cover its contents"
    );

    let mut objects = Vec::new();
    let mut cursor = 12usize;
    for index in 0..count {
        let first = bytes[cursor];
        cursor += 1;
        let kind = match (first >> 4) & 0x07 {
            1 => "commit",
            2 => "tree",
            3 => "blob",
            4 => "tag",
            other => panic!("object {index} has pack type {other}; a served pack carries no deltas"),
        };
        let mut size = usize::from(first & 0x0f);
        let mut shift = 4u32;
        let mut byte = first;
        while byte & 0x80 != 0 {
            byte = bytes[cursor];
            cursor += 1;
            size |= usize::from(byte & 0x7f) << shift;
            shift += 7;
        }
        let mut decompressor = flate2::Decompress::new(true);
        let mut data: Vec<u8> = Vec::with_capacity(size + 1);
        loop {
            let consumed = decompressor.total_in() as usize;
            let before = data.len();
            let status = decompressor
                .decompress_vec(
                    &bytes[cursor + consumed..],
                    &mut data,
                    flate2::FlushDecompress::None,
                )
                .expect("the pack carries deflated objects");
            if status == flate2::Status::StreamEnd {
                break;
            }
            if data.len() == data.capacity() {
                data.reserve(1024);
            } else {
                assert!(
                    data.len() > before,
                    "object {index} does not inflate: {status:?}"
                );
            }
        }
        assert_eq!(
            data.len(),
            size,
            "object {index} must inflate to the size its header declares"
        );
        cursor += decompressor.total_in() as usize;
        objects.push(Object { kind, data });
    }
    assert_eq!(
        cursor,
        bytes.len() - 20,
        "the pack must hold exactly the {count} objects it declares"
    );
    objects
}

// ---------------------------------------------------------------------- pkt-line

fn pkt(data: &[u8]) -> Vec<u8> {
    let mut out = format!("{:04x}", data.len() + 4).into_bytes();
    out.extend_from_slice(data);
    out
}

const FLUSH: &[u8] = b"0000";

/// The pkt-lines at the head of a response, plus the bytes after the first flush
/// packet (a packfile, when there is one).
fn parse_pkts(body: &[u8]) -> (Vec<String>, Vec<u8>) {
    let mut lines = Vec::new();
    let mut cursor = 0usize;
    while cursor + 4 <= body.len() {
        let header = std::str::from_utf8(&body[cursor..cursor + 4]).expect("a hex pkt-line length");
        let length = usize::from_str_radix(header, 16).expect("a hex pkt-line length");
        cursor += 4;
        if length == 0 {
            return (lines, body[cursor..].to_vec());
        }
        assert!(length >= 4, "a pkt-line length of {length} is not valid");
        let end = cursor + length - 4;
        lines.push(String::from_utf8_lossy(&body[cursor..end]).to_string());
        cursor = end;
    }
    (lines, Vec::new())
}

/// A fetch response: its single negotiation line (`NAK`, `ACK <oid>` or `ERR …`)
/// and the packfile that follows it. The pack is NOT pkt-line framed — a client
/// switches to reading the raw stream after that line, which is why this cannot
/// use [`parse_pkts`].
fn parse_fetch(body: &[u8]) -> (Vec<String>, Vec<u8>) {
    assert!(body.len() >= 4, "a fetch response carries at least one line");
    let header = std::str::from_utf8(&body[..4]).expect("a hex pkt-line length");
    let length = usize::from_str_radix(header, 16).expect("a hex pkt-line length");
    assert!(length >= 4, "a pkt-line length of {length} is not valid");
    (
        vec![String::from_utf8_lossy(&body[4..length]).to_string()],
        body[length..].to_vec(),
    )
}

/// Every pkt-line of a response, in order, with flush packets skipped — which is
/// how an advertisement (two sections separated by a flush) reads.
fn all_pkts(body: &[u8]) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cursor = 0usize;
    while cursor + 4 <= body.len() {
        let header = std::str::from_utf8(&body[cursor..cursor + 4]).expect("a hex pkt-line length");
        let length = usize::from_str_radix(header, 16).expect("a hex pkt-line length");
        cursor += 4;
        if length == 0 {
            continue;
        }
        assert!(length >= 4, "a pkt-line length of {length} is not valid");
        let end = cursor + length - 4;
        lines.push(String::from_utf8_lossy(&body[cursor..end]).to_string());
        cursor = end;
    }
    lines
}

/// The advertised refs of an `info/refs` response: `(refname, object name)`, with
/// the capability list of the first line returned separately.
fn parse_advertisement(service: &str, reply: &GitReply) -> (BTreeMap<String, String>, Vec<String>) {
    assert_eq!(reply.status, 200, "{service}: the advertisement must be 200");
    assert_eq!(
        reply.content_type,
        format!("application/x-{service}-advertisement"),
        "{service}: the advertisement's content-type is what a git client checks first"
    );
    let lines = all_pkts(&reply.body);
    assert_eq!(
        lines.first().map(String::as_str),
        Some(format!("# service={service}\n").as_str()),
        "{service}: the first pkt-line names the service, got {lines:?}"
    );
    let mut refs = BTreeMap::new();
    let mut capabilities = Vec::new();
    for line in lines.iter().skip(1) {
        let (record, caps) = line
            .split_once('\0')
            .map_or((line.as_str(), ""), |(record, caps)| (record, caps));
        if !caps.is_empty() {
            capabilities = caps
                .trim_end_matches('\n')
                .split(' ')
                .map(str::to_string)
                .collect();
        }
        let record = record.trim_end_matches('\n');
        let Some((id, name)) = record.split_once(' ') else {
            continue;
        };
        refs.insert(name.to_string(), id.to_string());
    }
    (refs, capabilities)
}

/// One ref update command, as a pushing client frames it.
fn command(old: &str, new: &str, name: &str, capabilities: &str) -> Vec<u8> {
    if capabilities.is_empty() {
        pkt(format!("{old} {new} {name}\n").as_bytes())
    } else {
        pkt(format!("{old} {new} {name}\0{capabilities}").as_bytes())
    }
}

/// A whole push request: the commands, a flush, and the packfile.
fn push_body(commands: &[Vec<u8>], objects: &[Object]) -> Vec<u8> {
    let mut body: Vec<u8> = commands.concat();
    body.extend_from_slice(FLUSH);
    if !objects.is_empty() {
        body.extend_from_slice(&pack(objects));
    }
    body
}

/// A whole fetch request: the wants, a flush, and `done`.
fn fetch_body(wants: &[String], haves: &[String]) -> Vec<u8> {
    let mut body: Vec<u8> = Vec::new();
    for (index, want) in wants.iter().enumerate() {
        if index == 0 {
            body.extend_from_slice(&pkt(format!("want {want} agent=git/2.43.0\n").as_bytes()));
        } else {
            body.extend_from_slice(&pkt(format!("want {want}\n").as_bytes()));
        }
    }
    body.extend_from_slice(FLUSH);
    for have in haves {
        body.extend_from_slice(&pkt(format!("have {have}\n").as_bytes()));
    }
    body.extend_from_slice(&pkt(b"done\n"));
    body
}

// -------------------------------------------------------------------- the tables

/// One pushed repository: the files of its first commit, then of its second.
struct PushCase {
    repository: &'static str,
    branch: &'static str,
    tag: &'static str,
    first: Vec<(&'static str, &'static str, Vec<u8>)>,
    second: Vec<(&'static str, &'static str, Vec<u8>)>,
}

/// The table: rows differ along what the rules generalise over — file count,
/// path depth, file mode (`100644`, `100755`, `120000`, the modes git writes for
/// the model's `FileModeTypeEnum`), content kind and size (text, every byte
/// value, unicode, empty, 64 KiB), and branch and tag naming.
fn push_cases() -> Vec<PushCase> {
    vec![
        PushCase {
            repository: "git-flat",
            branch: "main",
            tag: "v1.0.0",
            first: vec![("100644", "README.md", b"# flat\n".to_vec())],
            second: vec![
                ("100644", "README.md", b"# flat\n".to_vec()),
                ("100755", "run.sh", b"#!/bin/sh\necho hi\n".to_vec()),
            ],
        },
        PushCase {
            repository: "git-nested",
            branch: "feature/deep",
            tag: "release-2024.01",
            first: vec![
                ("100644", "a/b/c/deep.txt", b"deep\n".to_vec()),
                ("120000", "link", b"a/b/c/deep.txt".to_vec()),
                ("100644", "bin.dat", (0..=255u8).collect()),
            ],
            second: vec![
                ("100644", "a/b/c/deep.txt", "⚙ configuración\n".as_bytes().to_vec()),
                ("120000", "link", b"a/b/c/deep.txt".to_vec()),
                ("100644", "bin.dat", (0..=255u8).collect()),
                ("100644", "a/b/other.txt", b"sibling\n".to_vec()),
            ],
        },
        PushCase {
            repository: "git.wide_1",
            branch: "master",
            tag: "v2",
            // An empty file is a legal git blob, and a 64 KiB one spans more than
            // one deflate buffer.
            first: vec![
                ("100644", "empty", Vec::new()),
                ("100644", "big.bin", (0..=255u8).cycle().take(65_536).collect()),
                ("100644", "d1/one.txt", b"1\n".to_vec()),
                ("100644", "d1/two.txt", b"2\n".to_vec()),
                ("100644", "d2/three.txt", b"3\n".to_vec()),
            ],
            second: vec![
                ("100644", "empty", Vec::new()),
                ("100644", "big.bin", (0..=255u8).cycle().take(65_536).collect()),
                ("100644", "d1/one.txt", b"1 revised\n".to_vec()),
                ("100644", "d1/two.txt", b"2\n".to_vec()),
                ("100644", "d2/three.txt", b"3\n".to_vec()),
            ],
        },
    ]
}

fn snapshot(files: &[(&'static str, &'static str, Vec<u8>)]) -> BTreeMap<String, (String, Vec<u8>)> {
    files
        .iter()
        .map(|(mode, path, content)| {
            ((*path).to_string(), ((*mode).to_string(), content.clone()))
        })
        .collect()
}

/// The modeled `fileMode` a git tree entry mode names.
fn file_mode_of(mode: &str) -> &'static str {
    match mode {
        "100755" => "EXECUTABLE",
        "120000" => "SYMLINK",
        _ => "NORMAL",
    }
}

// --------------------------------------------------------------------- the tests

#[tokio::test]
async fn a_push_to_the_announced_clone_url_is_readable_through_the_api_and_clonable() {
    let router = emulator();

    for case in push_cases() {
        let label = case.repository;

        // The clone URL the API announces is the address the transport is driven
        // at below: nothing in this test rewrites it.
        let created = ok(
            &router,
            "CreateRepository",
            &json!({ "repositoryName": case.repository }),
        )
        .await;
        let clone_url = string_at(&created, "/repositoryMetadata/cloneUrlHttp");
        assert_announced_git_url(&router, label, case.repository, &clone_url).await;

        // An empty repository advertises no ref, and advertises its capabilities
        // on the line git reserves for that.
        let reply = advertise(&router, &clone_url, "git-receive-pack").await;
        let (refs, capabilities) = parse_advertisement("git-receive-pack", &reply);
        assert!(
            refs.keys().all(|name| name == "capabilities^{}"),
            "case {label:?}: an empty repository advertises no ref, got {refs:?}"
        );
        assert_eq!(
            refs.get("capabilities^{}").map(String::as_str),
            Some(NULL_ID),
            "case {label:?}: the capability line names the null object, got {refs:?}"
        );
        assert!(
            capabilities.iter().any(|cap| cap == "report-status"),
            "case {label:?}: a pushing client needs report-status advertised, got {capabilities:?}"
        );

        // PUSH ONE — create the branch.
        let files = snapshot(&case.first);
        let (root, mut objects) = snapshot_objects(&files);
        let first_commit = commit(&root, &[], &format!("first on {}", case.branch));
        let first_id = id_of(&first_commit);
        objects.push(first_commit);
        let reply = rpc(
            &router,
            &clone_url,
            "git-receive-pack",
            push_body(
                &[command(
                    NULL_ID,
                    &first_id,
                    &format!("refs/heads/{}", case.branch),
                    "report-status agent=git/2.43.0",
                )],
                &objects,
            ),
        )
        .await;
        assert_eq!(
            reply.content_type,
            "application/x-git-receive-pack-result",
            "case {label:?}: the push result's content-type"
        );
        let (report, _) = parse_pkts(&reply.body);
        assert_eq!(
            report,
            vec![
                "unpack ok\n".to_string(),
                format!("ok refs/heads/{}\n", case.branch)
            ],
            "case {label:?}: the push must be reported accepted"
        );

        // The control plane sees the pushed commit at the id the CLIENT computed.
        let branch = ok(
            &router,
            "GetBranch",
            &json!({ "repositoryName": case.repository, "branchName": case.branch }),
        )
        .await;
        assert_eq!(
            string_at(&branch, "/branch/commitId"),
            first_id,
            "case {label:?}: GetBranch must report the pushed commit"
        );
        for (mode, path, content) in &case.first {
            let got = ok(
                &router,
                "GetFile",
                &json!({ "repositoryName": case.repository, "filePath": path }),
            )
            .await;
            assert_eq!(
                content_of(&got),
                *content,
                "case {label:?}: GetFile {path} must return the pushed bytes"
            );
            assert_eq!(
                string_at(&got, "/blobId"),
                id_of(&blob(content)),
                "case {label:?}: GetFile {path} blobId must be the blob's object name"
            );
            assert_eq!(
                string_at(&got, "/fileMode"),
                file_mode_of(mode),
                "case {label:?}: GetFile {path} fileMode"
            );
            assert_eq!(
                got.get("fileSize").and_then(Value::as_i64),
                Some(content.len() as i64),
                "case {label:?}: GetFile {path} fileSize, got {got}"
            );
            assert_eq!(
                string_at(&got, "/commitId"),
                first_id,
                "case {label:?}: GetFile {path} commitId"
            );
        }

        // The first branch pushed becomes the default branch, and HEAD is
        // advertised as a symref to it.
        let fetched = ok(
            &router,
            "GetRepository",
            &json!({ "repositoryName": case.repository }),
        )
        .await;
        assert_eq!(
            string_at(&fetched, "/repositoryMetadata/defaultBranch"),
            case.branch,
            "case {label:?}: the first branch pushed becomes the default branch"
        );
        let reply = advertise(&router, &clone_url, "git-upload-pack").await;
        let (refs, capabilities) = parse_advertisement("git-upload-pack", &reply);
        assert_eq!(
            refs.get("HEAD").map(String::as_str),
            Some(first_id.as_str()),
            "case {label:?}: HEAD must be advertised at the branch tip, got {refs:?}"
        );
        assert_eq!(
            refs.get(&format!("refs/heads/{}", case.branch))
                .map(String::as_str),
            Some(first_id.as_str()),
            "case {label:?}: the branch must be advertised, got {refs:?}"
        );
        assert!(
            capabilities.contains(&format!("symref=HEAD:refs/heads/{}", case.branch)),
            "case {label:?}: HEAD's symref target must be advertised, got {capabilities:?}"
        );

        // PUSH TWO — move the branch on, declaring the tip the advertisement gave.
        let second_files = snapshot(&case.second);
        let (second_root, mut second_objects) = snapshot_objects(&second_files);
        let second_commit = commit(&second_root, std::slice::from_ref(&first_id), "second");
        let second_id = id_of(&second_commit);
        second_objects.push(second_commit);
        let reply = rpc(
            &router,
            &clone_url,
            "git-receive-pack",
            push_body(
                &[command(
                    &first_id,
                    &second_id,
                    &format!("refs/heads/{}", case.branch),
                    "report-status",
                )],
                &second_objects,
            ),
        )
        .await;
        let (report, _) = parse_pkts(&reply.body);
        assert_eq!(
            report,
            vec![
                "unpack ok\n".to_string(),
                format!("ok refs/heads/{}\n", case.branch)
            ],
            "case {label:?}: a fast-forward push must be accepted"
        );
        for (_, path, content) in &case.second {
            let got = ok(
                &router,
                "GetFile",
                &json!({ "repositoryName": case.repository, "filePath": path }),
            )
            .await;
            assert_eq!(
                content_of(&got),
                *content,
                "case {label:?}: GetFile {path} must return the second push's bytes"
            );
        }
        // The first commit is still readable at its own id: history is kept.
        let at_first = ok(
            &router,
            "GetFile",
            &json!({
                "repositoryName": case.repository,
                "filePath": case.first[0].1,
                "commitSpecifier": first_id,
            }),
        )
        .await;
        assert_eq!(
            content_of(&at_first),
            case.first[0].2,
            "case {label:?}: the first commit must stay readable at its own id"
        );

        // A STALE PUSH — the loser of a race declares an old value the ref has
        // moved past. It is refused and the ref stays where it is.
        let stale_files = snapshot(&case.first);
        let (stale_root, mut stale_objects) = snapshot_objects(&stale_files);
        let stale_commit = commit(&stale_root, std::slice::from_ref(&first_id), "loser");
        let stale_id = id_of(&stale_commit);
        stale_objects.push(stale_commit);
        let reply = rpc(
            &router,
            &clone_url,
            "git-receive-pack",
            push_body(
                &[command(
                    &first_id,
                    &stale_id,
                    &format!("refs/heads/{}", case.branch),
                    "report-status",
                )],
                &stale_objects,
            ),
        )
        .await;
        let (report, _) = parse_pkts(&reply.body);
        assert_eq!(
            report.first().map(String::as_str),
            Some("unpack ok\n"),
            "case {label:?}: the pack of a refused push still unpacks, got {report:?}"
        );
        assert_eq!(
            report.get(1).map(String::as_str),
            Some(format!("ng refs/heads/{} fetch first\n", case.branch).as_str()),
            "case {label:?}: a stale old value must be refused per ref, got {report:?}"
        );
        let branch = ok(
            &router,
            "GetBranch",
            &json!({ "repositoryName": case.repository, "branchName": case.branch }),
        )
        .await;
        assert_eq!(
            string_at(&branch, "/branch/commitId"),
            second_id,
            "case {label:?}: the refused push must leave the ref unmoved"
        );

        // A TAG — the only way to create one, and a branchName that names it is
        // not a branch.
        let tag_object = annotated_tag(&second_id, case.tag, "tagged");
        let tag_id = id_of(&tag_object);
        let reply = rpc(
            &router,
            &clone_url,
            "git-receive-pack",
            push_body(
                &[command(
                    NULL_ID,
                    &tag_id,
                    &format!("refs/tags/{}", case.tag),
                    "report-status",
                )],
                &[tag_object],
            ),
        )
        .await;
        let (report, _) = parse_pkts(&reply.body);
        assert_eq!(
            report,
            vec![
                "unpack ok\n".to_string(),
                format!("ok refs/tags/{}\n", case.tag)
            ],
            "case {label:?}: an annotated tag must be accepted"
        );
        let reply = advertise(&router, &clone_url, "git-upload-pack").await;
        let (refs, _) = parse_advertisement("git-upload-pack", &reply);
        assert_eq!(
            refs.get(&format!("refs/tags/{}", case.tag)).map(String::as_str),
            Some(tag_id.as_str()),
            "case {label:?}: the tag must be advertised, got {refs:?}"
        );
        assert_eq!(
            refs.get(&format!("refs/tags/{}^{{}}", case.tag))
                .map(String::as_str),
            Some(second_id.as_str()),
            "case {label:?}: an annotated tag is advertised with the commit it peels to"
        );
        let code = error_code(
            &router,
            "GetBranch",
            &json!({ "repositoryName": case.repository, "branchName": case.tag }),
            400,
        )
        .await;
        assert_eq!(
            code, "BranchDoesNotExistException",
            "case {label:?}: a tag name is not a branch name"
        );

        // A CLONE — every object the advertised refs reach is served, and the
        // bytes round-trip.
        let wants: Vec<String> = refs
            .iter()
            .filter(|(name, _)| !name.ends_with("^{}") && *name != "HEAD")
            .map(|(_, id)| id.clone())
            .collect();
        let reply = rpc(
            &router,
            &clone_url,
            "git-upload-pack",
            fetch_body(&wants, &[]),
        )
        .await;
        assert_eq!(
            reply.content_type, "application/x-git-upload-pack-result",
            "case {label:?}: the fetch result's content-type"
        );
        let (lines, packfile) = parse_fetch(&reply.body);
        assert_eq!(
            lines,
            vec!["NAK\n".to_string()],
            "case {label:?}: a clone with no haves is answered NAK then the pack"
        );
        let served: BTreeMap<String, Object> = unpack(&packfile)
            .into_iter()
            .map(|object| (id_of(&object), object))
            .collect();
        // Connectivity: a clone fails unless every object the wants reach is in
        // the pack.
        let mut queue: Vec<String> = wants.clone();
        let mut reached: Vec<String> = Vec::new();
        while let Some(id) = queue.pop() {
            if reached.contains(&id) {
                continue;
            }
            reached.push(id.clone());
            let object = served.get(&id).unwrap_or_else(|| {
                panic!("case {label:?}: the served pack is missing the object {id}")
            });
            match object.kind {
                "commit" => {
                    queue.push(commit_tree(&object.data));
                    for line in String::from_utf8_lossy(&object.data).lines() {
                        if let Some(parent) = line.strip_prefix("parent ") {
                            queue.push(parent.to_string());
                        }
                    }
                }
                "tree" => queue.extend(parse_tree(&object.data).into_iter().map(|(_, _, id)| id)),
                "tag" => {
                    for line in String::from_utf8_lossy(&object.data).lines() {
                        if let Some(target) = line.strip_prefix("object ") {
                            queue.push(target.to_string());
                        }
                    }
                }
                _ => {}
            }
        }
        assert_eq!(
            served.len(),
            reached.len(),
            "case {label:?}: the pack must carry exactly the closure of the wants: {} served, {} reached",
            served.len(),
            reached.len()
        );
        for (_, path, content) in &case.second {
            let id = id_of(&blob(content));
            assert_eq!(
                served.get(&id).map(|object| object.data.clone()),
                Some(content.clone()),
                "case {label:?}: the clone must serve {path} byte for byte"
            );
        }
    }
}

#[tokio::test]
async fn a_clone_serves_the_objects_the_control_plane_wrote() {
    let router = emulator();
    // The rows differ along path depth, content kind and size, and file mode —
    // the same class the push table spans, written through the API instead.
    let files: Vec<(&str, &str, Vec<u8>)> = vec![
        ("NORMAL", "README.md", b"# api written\n".to_vec()),
        ("EXECUTABLE", "scripts/build.sh", b"#!/bin/sh\nexit 0\n".to_vec()),
        ("SYMLINK", "docs/latest", b"docs/v1.md".to_vec()),
        ("NORMAL", "docs/v1.md", "⚙ versión 1\n".as_bytes().to_vec()),
        ("NORMAL", "data/bytes.bin", (0..=255u8).collect()),
    ];

    let created = ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "api-written" }),
    )
    .await;
    let clone_url = string_at(&created, "/repositoryMetadata/cloneUrlHttp");

    let mut parent: Option<String> = None;
    for (mode, path, content) in &files {
        let mut input = json!({
            "repositoryName": "api-written",
            "branchName": "main",
            "filePath": path,
            "fileContent": aws_smithy_types::base64::encode(content),
            "fileMode": mode,
            "commitMessage": format!("add {path}"),
            "name": "Author",
            "email": "author@example.com",
        });
        if let Some(parent) = &parent {
            input["parentCommitId"] = json!(parent);
        }
        let put = ok(&router, "PutFile", &input).await;
        assert_eq!(
            string_at(&put, "/blobId"),
            id_of(&blob(content)),
            "PutFile {path}: blobId must be the blob's git object name"
        );
        parent = Some(string_at(&put, "/commitId"));
    }
    let tip = parent.expect("the writes above");

    // The advertisement names the tip the API wrote.
    let reply = advertise(&router, &clone_url, "git-upload-pack").await;
    let (refs, _) = parse_advertisement("git-upload-pack", &reply);
    assert_eq!(
        refs.get("refs/heads/main").map(String::as_str),
        Some(tip.as_str()),
        "the branch the API wrote must be advertised at its tip, got {refs:?}"
    );

    // Cloning it serves the objects, and the tree walk recovers every file.
    let reply = rpc(
        &router,
        &clone_url,
        "git-upload-pack",
        fetch_body(std::slice::from_ref(&tip), &[]),
    )
    .await;
    let (lines, packfile) = parse_fetch(&reply.body);
    assert_eq!(lines, vec!["NAK\n".to_string()], "NAK precedes the pack");
    let served: BTreeMap<String, Object> = unpack(&packfile)
        .into_iter()
        .map(|object| (id_of(&object), object))
        .collect();
    let commit = served
        .get(&tip)
        .unwrap_or_else(|| panic!("the pack must carry the commit {tip}"));
    assert_eq!(commit.kind, "commit", "the tip must be served as a commit");

    let mut found: BTreeMap<String, (String, Vec<u8>)> = BTreeMap::new();
    let mut stack: Vec<(String, String)> = vec![(String::new(), commit_tree(&commit.data))];
    while let Some((prefix, tree_id)) = stack.pop() {
        let tree_object = served
            .get(&tree_id)
            .unwrap_or_else(|| panic!("the pack must carry the tree {tree_id}"));
        for (mode, name, id) in parse_tree(&tree_object.data) {
            let path = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if mode == "40000" {
                stack.push((path, id));
                continue;
            }
            let blob_object = served
                .get(&id)
                .unwrap_or_else(|| panic!("the pack must carry the blob {id} for {path}"));
            found.insert(path, (mode, blob_object.data.clone()));
        }
    }
    let expected: BTreeMap<String, (String, Vec<u8>)> = files
        .iter()
        .map(|(mode, path, content)| {
            let git_mode = match *mode {
                "EXECUTABLE" => "100755",
                "SYMLINK" => "120000",
                _ => "100644",
            };
            ((*path).to_string(), (git_mode.to_string(), content.clone()))
        })
        .collect();
    assert_eq!(
        found, expected,
        "a clone of API-written content must serve exactly the files the API wrote"
    );

    // A fetch that already has the tip is served no object at all.
    let reply = rpc(
        &router,
        &clone_url,
        "git-upload-pack",
        fetch_body(std::slice::from_ref(&tip), std::slice::from_ref(&tip)),
    )
    .await;
    let (lines, packfile) = parse_fetch(&reply.body);
    assert_eq!(
        lines,
        vec![format!("ACK {tip}\n")],
        "a have the server holds is acknowledged"
    );
    assert!(
        unpack(&packfile).is_empty(),
        "a client that already has the tip is sent an empty pack"
    );
}

/// One refusal the transport has to answer precisely.
struct RefusalCase {
    name: &'static str,
    /// The path appended to the repository's clone URL, with its query.
    suffix: &'static str,
    method: &'static str,
    body: Vec<u8>,
    status: u16,
    /// A fragment the response body must carry.
    contains: &'static str,
}

#[tokio::test]
async fn the_transport_refuses_what_it_cannot_serve() {
    let router = emulator();
    let created = ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "refusals" }),
    )
    .await;
    let clone_url = string_at(&created, "/repositoryMetadata/cloneUrlHttp");

    // A pack whose checksum does not cover its contents.
    let object = blob(b"corrupt\n");
    let mut corrupt = pack(std::slice::from_ref(&object));
    let last = corrupt.len() - 1;
    corrupt[last] ^= 0xff;
    let mut corrupt_push = command(
        NULL_ID,
        &id_of(&commit(&id_of(&tree(&[])), &[], "x")),
        "refs/heads/main",
        "report-status",
    );
    corrupt_push.extend_from_slice(FLUSH);
    corrupt_push.extend_from_slice(&corrupt);

    let cases = vec![
        RefusalCase {
            name: "the dumb HTTP protocol",
            suffix: "/info/refs",
            method: "GET",
            body: Vec::new(),
            status: 403,
            contains: "smart HTTP",
        },
        RefusalCase {
            name: "a service this server does not speak",
            suffix: "/info/refs?service=git-wrap-pack",
            method: "GET",
            body: Vec::new(),
            status: 403,
            contains: "smart HTTP",
        },
        RefusalCase {
            name: "a want for an object the repository does not hold",
            suffix: "/git-upload-pack",
            method: "POST",
            body: fetch_body(&[NULL_ID.replace('0', "a")], &[]),
            status: 200,
            contains: "ERR upload-pack: not our ref",
        },
        RefusalCase {
            name: "a shallow fetch, which needs a capability this server does not advertise",
            suffix: "/git-upload-pack",
            method: "POST",
            body: {
                let mut body = pkt(b"want 1111111111111111111111111111111111111111\n");
                body.extend_from_slice(&pkt(b"deepen 1\n"));
                body.extend_from_slice(FLUSH);
                body.extend_from_slice(&pkt(b"done\n"));
                body
            },
            status: 200,
            contains: "deepen is not supported",
        },
        RefusalCase {
            name: "a corrupt packfile",
            suffix: "/git-receive-pack",
            method: "POST",
            body: corrupt_push,
            status: 200,
            contains: "unpack the pack checksum does not match its contents",
        },
        RefusalCase {
            name: "a ref outside the branch and tag namespaces",
            suffix: "/git-receive-pack",
            method: "POST",
            body: push_body(
                &[command(NULL_ID, &NULL_ID.replace('0', "b"), "refs/notes/x", "report-status")],
                &[],
            ),
            status: 200,
            contains: "ng refs/notes/x funny refname",
        },
        RefusalCase {
            name: "a branch name git itself refuses",
            suffix: "/git-receive-pack",
            method: "POST",
            body: push_body(
                &[command(NULL_ID, &NULL_ID.replace('0', "c"), "refs/heads/a..b", "report-status")],
                &[],
            ),
            status: 200,
            contains: "ng refs/heads/a..b funny refname",
        },
        RefusalCase {
            name: "a commit the push does not carry",
            suffix: "/git-receive-pack",
            method: "POST",
            body: push_body(
                &[command(NULL_ID, &NULL_ID.replace('0', "d"), "refs/heads/main", "report-status")],
                &[],
            ),
            status: 200,
            contains: "ng refs/heads/main missing necessary object",
        },
        RefusalCase {
            name: "a commit whose parent the push does not carry",
            suffix: "/git-receive-pack",
            method: "POST",
            body: {
                // The tip, its tree and its blob are all in the pack; its PARENT
                // is not, so the ref's closure would be incomplete and a later
                // fetch could not serve it.
                let files = snapshot(&[("100644", "f.txt", b"orphan\n".to_vec())]);
                let (root, mut objects) = snapshot_objects(&files);
                let orphan = commit(&root, &[NULL_ID.replace('0', "f")], "orphan");
                let orphan_id = id_of(&orphan);
                objects.push(orphan);
                push_body(
                    &[command(NULL_ID, &orphan_id, "refs/heads/main", "report-status")],
                    &objects,
                )
            },
            status: 200,
            contains: "ng refs/heads/main missing necessary objects:",
        },
        RefusalCase {
            name: "a ref name longer than the model gives one",
            suffix: "/git-receive-pack",
            method: "POST",
            body: push_body(
                &[command(
                    NULL_ID,
                    &NULL_ID.replace('0', "e"),
                    &format!("refs/heads/{}", "b".repeat(257)),
                    "report-status",
                )],
                &[],
            ),
            status: 200,
            contains: "funny refname",
        },
        RefusalCase {
            name: "a ref update command that is not <old> <new> <ref>",
            suffix: "/git-receive-pack",
            method: "POST",
            body: {
                let mut body = pkt(b"not-a-command\n");
                body.extend_from_slice(FLUSH);
                body
            },
            status: 400,
            contains: "a ref update command is malformed",
        },
        RefusalCase {
            name: "a push of a blob as a branch",
            suffix: "/git-receive-pack",
            method: "POST",
            body: push_body(
                &[command(NULL_ID, &id_of(&object), "refs/heads/main", "report-status")],
                std::slice::from_ref(&object),
            ),
            status: 200,
            contains: "ng refs/heads/main a branch cannot point at a blob",
        },
    ];

    for case in cases {
        let reply = git(
            &router,
            case.method,
            &format!("{clone_url}{}", case.suffix),
            Some("application/x-git-request"),
            case.body,
        )
        .await;
        assert_eq!(
            reply.status, case.status,
            "case {:?}: status, body {:?}",
            case.name,
            String::from_utf8_lossy(&reply.body)
        );
        let body = String::from_utf8_lossy(&reply.body).to_string();
        assert!(
            body.contains(case.contains),
            "case {:?}: the response must say {:?}, got {body:?}",
            case.name,
            case.contains
        );
    }

    // A path that names no repository is a 404, whichever endpoint it addresses.
    let reply = advertise(
        &router,
        &format!("https://{ENDPOINT}/v1/repos/no-such-repository"),
        "git-upload-pack",
    )
    .await;
    assert_eq!(reply.status, 404, "an unknown repository is not found");
    assert!(
        String::from_utf8_lossy(&reply.body).contains("no-such-repository"),
        "the refusal names the repository, got {:?}",
        String::from_utf8_lossy(&reply.body)
    );
}

#[tokio::test]
async fn a_branch_is_deleted_by_a_push_but_the_default_branch_is_kept() {
    let router = emulator();
    let created = ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "deletions" }),
    )
    .await;
    let clone_url = string_at(&created, "/repositoryMetadata/cloneUrlHttp");

    // Two branches off one commit.
    let files = snapshot(&[("100644", "f.txt", b"content\n".to_vec())]);
    let (root, mut objects) = snapshot_objects(&files);
    let tip = commit(&root, &[], "base");
    let tip_id = id_of(&tip);
    objects.push(tip);
    let reply = rpc(
        &router,
        &clone_url,
        "git-receive-pack",
        push_body(
            &[
                command(NULL_ID, &tip_id, "refs/heads/main", "report-status"),
                command(NULL_ID, &tip_id, "refs/heads/topic", ""),
            ],
            &objects,
        ),
    )
    .await;
    let (report, _) = parse_pkts(&reply.body);
    assert_eq!(
        report,
        vec![
            "unpack ok\n".to_string(),
            "ok refs/heads/main\n".to_string(),
            "ok refs/heads/topic\n".to_string()
        ],
        "both branches must be created, got {report:?}"
    );

    // `main` is chosen as the default branch, and cannot be deleted while it is.
    let fetched = ok(
        &router,
        "GetRepository",
        &json!({ "repositoryName": "deletions" }),
    )
    .await;
    assert_eq!(
        string_at(&fetched, "/repositoryMetadata/defaultBranch"),
        "main",
        "main is the branch a client checks out, so it is the default"
    );
    let cases: Vec<(&str, &str, &str)> = vec![
        ("the default branch", "refs/heads/main", "ng refs/heads/main the default branch cannot be deleted\n"),
        ("a non-default branch", "refs/heads/topic", "ok refs/heads/topic\n"),
        ("a branch that does not exist", "refs/heads/ghost", "ng refs/heads/ghost ref does not exist\n"),
    ];
    for (label, refname, expected) in cases {
        let old = if refname == "refs/heads/ghost" {
            NULL_ID.to_string()
        } else {
            tip_id.clone()
        };
        let reply = rpc(
            &router,
            &clone_url,
            "git-receive-pack",
            push_body(&[command(&old, NULL_ID, refname, "report-status")], &[]),
        )
        .await;
        let (report, _) = parse_pkts(&reply.body);
        assert_eq!(
            report.get(1).map(String::as_str),
            Some(expected),
            "case {label:?}: the verdict for deleting {refname}, got {report:?}"
        );
    }

    // The deleted branch is gone from both faces; the default branch is intact.
    let code = error_code(
        &router,
        "GetBranch",
        &json!({ "repositoryName": "deletions", "branchName": "topic" }),
        400,
    )
    .await;
    assert_eq!(
        code, "BranchDoesNotExistException",
        "a deleted branch must not resolve"
    );
    let reply = advertise(&router, &clone_url, "git-upload-pack").await;
    let (refs, _) = parse_advertisement("git-upload-pack", &reply);
    assert_eq!(
        refs.get("refs/heads/topic"),
        None,
        "a deleted branch must not be advertised, got {refs:?}"
    );
    assert_eq!(
        refs.get("refs/heads/main").map(String::as_str),
        Some(tip_id.as_str()),
        "the default branch must be untouched, got {refs:?}"
    );
}

#[tokio::test]
async fn the_transport_answers_the_clone_url_in_every_form_a_client_writes_it() {
    let router = emulator();
    let created = ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "url-forms" }),
    )
    .await;
    let clone_url = string_at(&created, "/repositoryMetadata/cloneUrlHttp");

    let files = snapshot(&[("100644", "f.txt", b"content\n".to_vec())]);
    let (root, mut objects) = snapshot_objects(&files);
    let tip = commit(&root, &[], "base");
    let tip_id = id_of(&tip);
    objects.push(tip);
    let reply = rpc(
        &router,
        &clone_url,
        "git-receive-pack",
        push_body(
            &[command(NULL_ID, &tip_id, "refs/heads/main", "report-status")],
            &objects,
        ),
    )
    .await;
    let (report, _) = parse_pkts(&reply.body);
    assert_eq!(
        report.get(1).map(String::as_str),
        Some("ok refs/heads/main\n"),
        "the branch must be created, got {report:?}"
    );

    // A client writes the clone URL as the API gave it, with the trailing slash a
    // copied URL carries, or with the conventional `.git` suffix. All name the
    // same repository.
    for suffix in ["", "/", ".git", ".git/"] {
        let reply = git(
            &router,
            "GET",
            &format!("{clone_url}{suffix}/info/refs?service=git-upload-pack"),
            None,
            Vec::new(),
        )
        .await;
        let (refs, _) = parse_advertisement("git-upload-pack", &reply);
        assert_eq!(
            refs.get("refs/heads/main").map(String::as_str),
            Some(tip_id.as_str()),
            "case {suffix:?}: the URL must address the same repository, got {refs:?}"
        );
    }
}

#[tokio::test]
async fn a_push_of_a_thin_delta_compressed_pack_is_resolved() {
    // What a real `git push` sends: objects compressed as deltas, against a base
    // the server already holds (a thin pack) and against a base earlier in the
    // same pack — including a delta whose own base is a delta.
    let router = emulator();
    let created = ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "deltas" }),
    )
    .await;
    let clone_url = string_at(&created, "/repositoryMetadata/cloneUrlHttp");

    let held: Vec<u8> = b"line one\n".to_vec();
    let appended: Vec<u8> = b"line one\nline two\n".to_vec();
    // Longer than one insert instruction can carry, so the delta uses several.
    let twice_appended: Vec<u8> = {
        let mut content = appended.clone();
        content.extend_from_slice("tail ".repeat(60).as_bytes());
        content
    };

    // PUSH ONE — the base object, whole.
    let first_files = snapshot(&[("100644", "f1.txt", held.clone())]);
    let (first_root, mut first_objects) = snapshot_objects(&first_files);
    let first_commit = commit(&first_root, &[], "base");
    let first_id = id_of(&first_commit);
    first_objects.push(first_commit);
    let reply = rpc(
        &router,
        &clone_url,
        "git-receive-pack",
        push_body(
            &[command(NULL_ID, &first_id, "refs/heads/main", "report-status")],
            &first_objects,
        ),
    )
    .await;
    let (report, _) = parse_pkts(&reply.body);
    assert_eq!(
        report.get(1).map(String::as_str),
        Some("ok refs/heads/main\n"),
        "the base push must be accepted, got {report:?}"
    );

    // PUSH TWO — two new blobs, both as deltas.
    let second_files = snapshot(&[
        ("100644", "f1.txt", held.clone()),
        ("100644", "f2.txt", appended.clone()),
        ("100644", "f3.txt", twice_appended.clone()),
    ]);
    let (second_root, snapshot_made) = snapshot_objects(&second_files);
    let second_commit = commit(&second_root, std::slice::from_ref(&first_id), "deltas");
    let second_id = id_of(&second_commit);
    let mut entries = vec![
        // A thin-pack delta: its base is the blob the server already holds.
        Entry::Reference(id_of(&blob(&held)), delta(&held, &appended)),
        // A delta against the entry above, which is itself a delta.
        Entry::Offset(0, delta(&appended, &twice_appended)),
    ];
    // The trees, whole — a pack mixes both forms.
    for object in snapshot_made {
        if object.kind == "tree" {
            entries.push(Entry::Whole(object));
        }
    }
    entries.push(Entry::Whole(second_commit));
    let mut body = command(&first_id, &second_id, "refs/heads/main", "report-status");
    body.extend_from_slice(FLUSH);
    body.extend_from_slice(&pack_entries(&entries));
    let reply = rpc(&router, &clone_url, "git-receive-pack", body).await;
    let (report, _) = parse_pkts(&reply.body);
    assert_eq!(
        report,
        vec!["unpack ok\n".to_string(), "ok refs/heads/main\n".to_string()],
        "a delta-compressed thin pack must be resolved and accepted, got {report:?}"
    );

    // The delta-compressed content is readable, which can only be true if the
    // deltas were applied to the right bases.
    for (path, content) in [
        ("f1.txt", &held),
        ("f2.txt", &appended),
        ("f3.txt", &twice_appended),
    ] {
        let got = ok(
            &router,
            "GetFile",
            &json!({ "repositoryName": "deltas", "filePath": path }),
        )
        .await;
        assert_eq!(
            content_of(&got),
            *content,
            "case {path:?}: the file a delta carried must read back whole"
        );
        assert_eq!(
            string_at(&got, "/blobId"),
            id_of(&blob(content)),
            "case {path:?}: blobId must be the object name of the resolved blob"
        );
    }

    // And a clone serves them back as whole objects.
    let reply = rpc(
        &router,
        &clone_url,
        "git-upload-pack",
        fetch_body(std::slice::from_ref(&second_id), &[]),
    )
    .await;
    let (_, packfile) = parse_fetch(&reply.body);
    let served: BTreeMap<String, Object> = unpack(&packfile)
        .into_iter()
        .map(|object| (id_of(&object), object))
        .collect();
    for content in [&held, &appended, &twice_appended] {
        assert_eq!(
            served.get(&id_of(&blob(content))).map(|object| &object.data),
            Some(content),
            "the clone must serve the resolved blob"
        );
    }
}

#[tokio::test]
async fn the_transport_refuses_a_pack_that_would_cost_more_than_its_size_suggests() {
    // A request body is small; what it ASKS the server to produce need not be.
    // Each row is a pack of at most a few kilobytes that would cost the emulator
    // gigabytes of memory or exponential time if the bound it names were missing.
    let router = emulator();
    let created = ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "budgets" }),
    )
    .await;
    let clone_url = string_at(&created, "/repositoryMetadata/cloneUrlHttp");

    // ROW 1 — an entry header that declares megabytes and carries nothing: the
    // declared size must be checked against what inflates, for a whole object
    // and a delta alike, and must never be the size of a buffer.
    let tiny = blob(b"tiny\n");
    let mut overstated = command(
        NULL_ID,
        &id_of(&commit(&id_of(&tree(&[])), &[], "x")),
        "refs/heads/main",
        "report-status",
    );
    overstated.extend_from_slice(FLUSH);
    overstated.extend_from_slice(&pack_with_declared_size(&tiny, 5 * 1024 * 1024));

    // ROW 2 — forty entries of a megabyte of zeros each: every entry is inside
    // the per-object bound, the body is a few kilobytes, and the SUM is what has
    // to be refused.
    let zeros: Vec<Object> = (0..40u32)
        .map(|index| {
            let mut content = vec![0u8; 1024 * 1024];
            content.extend_from_slice(&index.to_be_bytes());
            blob(&content)
        })
        .collect();
    let mut inflating = command(
        NULL_ID,
        &id_of(&commit(&id_of(&tree(&[])), &[], "x")),
        "refs/heads/main",
        "report-status",
    );
    inflating.extend_from_slice(FLUSH);
    inflating.extend_from_slice(&pack(&zeros));

    // ROW 3 — a tree DAG: forty tree objects, each naming the one below it TWICE,
    // so the snapshot has 2^40 paths and inserts no file at all. Neither the
    // depth bound nor the file-count bound trips; the entries-walked bound does.
    let mut dag: Vec<Object> = Vec::new();
    let mut level = tree(&[]);
    let mut level_id = id_of(&level);
    dag.push(level);
    for _ in 0..40 {
        level = tree(&[
            ("40000".to_string(), "a".to_string(), level_id.clone()),
            ("40000".to_string(), "b".to_string(), level_id.clone()),
        ]);
        level_id = id_of(&level);
        dag.push(level);
    }
    let dag_commit = commit(&level_id, &[], "wide");
    let dag_commit_id = id_of(&dag_commit);
    dag.push(dag_commit);
    let dag_push = push_body(
        &[command(
            NULL_ID,
            &dag_commit_id,
            "refs/heads/main",
            "report-status",
        )],
        &dag,
    );

    let cases: Vec<(&str, Vec<u8>, &str)> = vec![
        (
            "an entry header that overstates its size",
            overstated,
            "but declared 5242880",
        ),
        (
            "a pack whose entries inflate to more in total than the emulator accepts",
            inflating,
            "bytes this emulator accepts in total",
        ),
        (
            "a tree DAG that presents exponentially many paths",
            dag_push,
            // Either of the projection's own bounds may trip first — the entries it
            // walks per request, or its deadline — and both refuse the ref with the
            // same prefix; what must never happen is the walk running to the end.
            "ng refs/heads/main the commit could not be read back:",
        ),
    ];

    for (label, body, expected) in cases {
        let reply = rpc(&router, &clone_url, "git-receive-pack", body).await;
        assert_eq!(reply.status, 200, "case {label:?}: the push is answered");
        let report = String::from_utf8_lossy(&reply.body).to_string();
        assert!(
            report.contains(expected),
            "case {label:?}: the push must be refused with {expected:?}, got {report:?}"
        );
        // Nothing was stored: the branch does not exist and the advertisement is
        // still empty.
        let code = error_code(
            &router,
            "GetBranch",
            &json!({ "repositoryName": "budgets", "branchName": "main" }),
            400,
        )
        .await;
        assert_eq!(
            code, "BranchDoesNotExistException",
            "case {label:?}: a refused push must leave the repository empty"
        );
    }
}

#[tokio::test]
async fn an_object_no_ref_reaches_is_released_and_cannot_be_fetched() {
    // A pack may carry more than the refs it updates need. What no ref reaches is
    // released — git's own garbage-collection rule — and a client cannot fetch it
    // by naming its id, which is what `git-upload-pack` requires of a `want`.
    let (router, engine) = emulator_with_engine();
    let created = ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "unreferenced" }),
    )
    .await;
    let clone_url = string_at(&created, "/repositoryMetadata/cloneUrlHttp");

    let files = snapshot(&[("100644", "kept.txt", b"kept\n".to_vec())]);
    let (root, mut objects) = snapshot_objects(&files);
    let tip = commit(&root, &[], "kept");
    let tip_id = id_of(&tip);
    objects.push(tip);
    // An extra blob no tree names.
    let stray = blob(b"nothing points at me\n");
    let stray_id = id_of(&stray);
    objects.push(stray);

    let reply = rpc(
        &router,
        &clone_url,
        "git-receive-pack",
        push_body(
            &[command(NULL_ID, &tip_id, "refs/heads/main", "report-status")],
            &objects,
        ),
    )
    .await;
    let (report, _) = parse_pkts(&reply.body);
    assert_eq!(
        report,
        vec!["unpack ok\n".to_string(), "ok refs/heads/main\n".to_string()],
        "the push itself must be accepted, got {report:?}"
    );

    let reply = rpc(
        &router,
        &clone_url,
        "git-upload-pack",
        fetch_body(std::slice::from_ref(&stray_id), &[]),
    )
    .await;
    let body = String::from_utf8_lossy(&reply.body).to_string();
    assert!(
        body.contains(&format!("ERR upload-pack: not our ref {stray_id}")),
        "an object no ref reaches is not fetchable, got {body:?}"
    );

    let state = engine
        .state::<service_com_amazonaws_codecommit_codecommit_20150413::ComAmazonawsCodecommitCodecommit20150413State>();
    let store = state.lock();
    let repository = store
        .repositories
        .get("unreferenced")
        .expect("the repository above");
    assert!(
        !repository.objects.contains_key(&stray_id),
        "an object no ref reaches must be released from the store"
    );
    assert!(
        repository.objects.contains_key(&tip_id),
        "the commit a ref reaches must be kept"
    );
}

#[tokio::test]
async fn one_push_cannot_multiply_the_projection_by_carrying_many_ref_updates() {
    // The projection's cost is a PRODUCT: every ref update names a commit whose
    // ancestors each carry a whole tree snapshot. One budget covers the whole
    // request, so a push that carries two disjoint histories cannot project twice
    // the allowance, and a push that carries more ref updates than the emulator
    // accepts is refused before any of it runs.
    let router = emulator();
    let created = ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "projection" }),
    )
    .await;
    let clone_url = string_at(&created, "/repositoryMetadata/cloneUrlHttp");

    // Two chains of 200 commits that share one tree and one blob, so the pack is
    // a few hundred kilobytes while the history it names is long. Together they
    // are past the 256 commits one request projects.
    let files = snapshot(&[("100644", "f.txt", b"shared\n".to_vec())]);
    let (root, mut objects) = snapshot_objects(&files);
    let mut tips: Vec<String> = Vec::new();
    for chain in 0..2u32 {
        let mut parents: Vec<String> = Vec::new();
        for index in 0..200u32 {
            let object = commit(&root, &parents, &format!("chain {chain} commit {index}"));
            let id = id_of(&object);
            objects.push(object);
            parents = vec![id];
        }
        tips.push(parents.pop().expect("each chain has a tip"));
    }
    let reply = rpc(
        &router,
        &clone_url,
        "git-receive-pack",
        push_body(
            &[
                command(NULL_ID, &tips[0], "refs/heads/chain-0", "report-status"),
                command(NULL_ID, &tips[1], "refs/heads/chain-1", ""),
            ],
            &objects,
        ),
    )
    .await;
    let (report, _) = parse_pkts(&reply.body);
    assert_eq!(
        report.first().map(String::as_str),
        Some("unpack ok\n"),
        "the pack itself is within every bound, got {report:?}"
    );
    assert_eq!(
        report.get(1).map(String::as_str),
        Some("ok refs/heads/chain-0\n"),
        "the first history is inside the request's projection budget, got {report:?}"
    );
    let second = report.get(2).cloned().unwrap_or_default();
    assert!(
        second.starts_with("ng refs/heads/chain-1 ")
            && second.contains("commits this emulator projects at once"),
        "the second history must be refused by the REQUEST's budget, got {second:?}"
    );
    let branch = ok(
        &router,
        "GetBranch",
        &json!({ "repositoryName": "projection", "branchName": "chain-0" }),
    )
    .await;
    assert_eq!(
        string_at(&branch, "/branch/commitId"),
        tips[0],
        "the accepted ref must be readable"
    );
    let code = error_code(
        &router,
        "GetBranch",
        &json!({ "repositoryName": "projection", "branchName": "chain-1" }),
        400,
    )
    .await;
    assert_eq!(
        code, "BranchDoesNotExistException",
        "the refused ref must not have moved"
    );

    // And more ref updates than one push may carry is refused outright.
    let many: Vec<Vec<u8>> = (0..257u32)
        .map(|index| {
            command(
                NULL_ID,
                &tips[0],
                &format!("refs/heads/many-{index}"),
                if index == 0 { "report-status" } else { "" },
            )
        })
        .collect();
    let reply = rpc(
        &router,
        &clone_url,
        "git-receive-pack",
        push_body(&many, &[]),
    )
    .await;
    assert_eq!(
        reply.status, 400,
        "a push with more ref updates than the emulator accepts is refused, got {:?}",
        String::from_utf8_lossy(&reply.body)
    );
    assert!(
        String::from_utf8_lossy(&reply.body).contains("refs this emulator accepts at once"),
        "the refusal names the bound, got {:?}",
        String::from_utf8_lossy(&reply.body)
    );
}

#[tokio::test]
async fn a_repository_holds_no_more_refs_than_the_emulator_keeps() {
    // A ref is a root of reachability, so it is never pruned: repeated pushes that
    // create refs are what could grow this service's state without end. The bound
    // is the STATE's own, the same one `CreateBranch` applies, so the two faces
    // cannot disagree about how many branches a repository may hold.
    const MAX_BRANCHES: usize =
        service_com_amazonaws_codecommit_codecommit_20150413::state::MAX_BRANCHES_PER_REPOSITORY;
    let router = emulator();
    let created = ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "ref-quota" }),
    )
    .await;
    let clone_url = string_at(&created, "/repositoryMetadata/cloneUrlHttp");

    let files = snapshot(&[("100644", "f.txt", b"one commit\n".to_vec())]);
    let (root, mut objects) = snapshot_objects(&files);
    let tip = commit(&root, &[], "base");
    let tip_id = id_of(&tip);
    objects.push(tip);

    // One push that asks for more branches than the repository may hold, all
    // pointing at the one commit — so the object store never grows and only the
    // branch bound can stop it.
    let asked = MAX_BRANCHES + 4;
    let commands: Vec<Vec<u8>> = (0..asked)
        .map(|index| {
            command(
                NULL_ID,
                &tip_id,
                &format!("refs/heads/b{index}"),
                if index == 0 { "report-status" } else { "" },
            )
        })
        .collect();
    let reply = rpc(
        &router,
        &clone_url,
        "git-receive-pack",
        push_body(&commands, &objects),
    )
    .await;
    let (report, _) = parse_pkts(&reply.body);
    let accepted = report.iter().filter(|line| line.starts_with("ok ")).count();
    assert_eq!(
        accepted, MAX_BRANCHES,
        "exactly the branches the emulator keeps must be created, got {accepted} of {asked}"
    );
    let refused = report
        .iter()
        .find(|line| line.starts_with("ng "))
        .cloned()
        .unwrap_or_default();
    assert!(
        refused.contains("branches this emulator keeps"),
        "the refusal must name the branch bound, got {refused:?}"
    );

    // The accepted refs are all readable, and a refused one is not.
    let branch = ok(
        &router,
        "GetBranch",
        &json!({ "repositoryName": "ref-quota", "branchName": "b0" }),
    )
    .await;
    assert_eq!(
        string_at(&branch, "/branch/commitId"),
        tip_id,
        "an accepted ref must point at the pushed commit"
    );
    let code = error_code(
        &router,
        "GetBranch",
        &json!({ "repositoryName": "ref-quota", "branchName": format!("b{}", asked - 1) }),
        400,
    )
    .await;
    assert_eq!(
        code, "BranchDoesNotExistException",
        "a ref refused by the bound must not exist"
    );
}

#[tokio::test]
async fn the_transport_is_claimed_by_path_whatever_host_the_client_was_given() {
    let (router, engine) = emulator_with_engine();
    engine
        .state::<service_com_amazonaws_codecommit_codecommit_20150413::ComAmazonawsCodecommitCodecommit20150413State>()
        .lock()
        .repositories
        .insert(
            "hosts".to_string(),
            service_com_amazonaws_codecommit_codecommit_20150413::state::Repository::new(
                "hosts",
                None,
                None,
                BTreeMap::new(),
                "us-west-2",
                std::time::SystemTime::now(),
            ),
        );

    // A client reaches the emulator at whatever host the clone URL named: the
    // emulator's own endpoint, the CodeCommit API host, or — when the URL was
    // used verbatim with DNS or a proxy pointing it here — the public git host.
    // A request that names a DIFFERENT service is left to that service, which is
    // why the last row is not served.
    let cases: Vec<(&str, Option<&str>, bool)> = vec![
        ("the CodeCommit API host", Some("codecommit.us-west-2.amazonaws.com"), true),
        ("the public git host", Some("git-codecommit.us-west-2.amazonaws.com"), true),
        ("a loopback endpoint", Some("127.0.0.1:9297"), true),
        ("a localhost endpoint", Some("localhost:9297"), true),
        ("no host at all", None, true),
        ("a host naming another service", Some("s3.us-west-2.amazonaws.com"), false),
    ];

    for (label, host, served) in cases {
        let mut request = http::Request::builder()
            .method("GET")
            .uri("/v1/repos/hosts/info/refs?service=git-upload-pack")
            .header("user-agent", "git/2.43.0")
            .header("authorization", basic(GIT_USER));
        if let Some(host) = host {
            request = request.header("host", host);
        }
        let response = router
            .dispatch(request.body(bytes::Bytes::new()).expect("a request"))
            .await;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        if served {
            assert_eq!(
                status, 200,
                "case {label:?}: the transport must answer, got {status} {:?}",
                String::from_utf8_lossy(response.body())
            );
            assert_eq!(
                content_type, "application/x-git-upload-pack-advertisement",
                "case {label:?}: the advertisement's content-type"
            );
        } else {
            assert_ne!(
                status, 200,
                "case {label:?}: a request naming another service must not be claimed here, got {:?}",
                String::from_utf8_lossy(response.body())
            );
        }
    }
}

#[tokio::test]
async fn the_announced_clone_url_names_whichever_endpoint_the_request_arrived_at() {
    let (router, engine) = emulator_with_engine();
    // This store was never written through `CreateRepository`, so this emulator opened
    // no git address of its own (see `crate::state::git_endpoint`): the announced URLs
    // name the endpoint the ASKING request arrived at, which is the fallback — the
    // address this emulator is reached on is still the only one it answers git at.
    //
    // Rows: a host-style AWS endpoint, a loopback address with a port, an IPv6
    // literal with a port, a container endpoint, and a request with no Host at
    // all (only an in-process caller, which falls back to loopback — the address
    // the emulator binds by default, never a host it does not serve).
    let cases: Vec<(&str, Option<&str>, &str, &str)> = vec![
        (
            "an AWS-style endpoint",
            Some("codecommit.eu-west-1.amazonaws.com"),
            "https://codecommit.eu-west-1.amazonaws.com/v1/repos/endpoints",
            "ssh://codecommit.eu-west-1.amazonaws.com/v1/repos/endpoints",
        ),
        (
            "a loopback endpoint with a port",
            Some("127.0.0.1:9297"),
            "https://127.0.0.1:9297/v1/repos/endpoints",
            "ssh://127.0.0.1/v1/repos/endpoints",
        ),
        (
            "an IPv6 endpoint with a port",
            Some("[::1]:9297"),
            "https://[::1]:9297/v1/repos/endpoints",
            "ssh://[::1]/v1/repos/endpoints",
        ),
        (
            "a container endpoint",
            Some("emulator.internal:8080"),
            "https://emulator.internal:8080/v1/repos/endpoints",
            "ssh://emulator.internal/v1/repos/endpoints",
        ),
        (
            "no endpoint at all",
            None,
            "https://127.0.0.1/v1/repos/endpoints",
            "ssh://127.0.0.1/v1/repos/endpoints",
        ),
    ];

    // One repository, created once: the URLs are not stored, so every read
    // reports the endpoint of the request that asked.
    engine
        .state::<service_com_amazonaws_codecommit_codecommit_20150413::ComAmazonawsCodecommitCodecommit20150413State>()
        .lock()
        .repositories
        .insert(
            "endpoints".to_string(),
            service_com_amazonaws_codecommit_codecommit_20150413::state::Repository::new(
                "endpoints",
                None,
                None,
                BTreeMap::new(),
                "us-west-2",
                std::time::SystemTime::now(),
            ),
        );

    for (label, host, expected_http, expected_ssh) in cases {
        let mut request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", "CodeCommit_20150413.GetRepository")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDTESTKEY/20260101/us-west-2/codecommit/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000",
            );
        if let Some(host) = host {
            request = request.header("host", host);
        }
        let response = router
            .dispatch(
                request
                    .body(bytes::Bytes::from(
                        serde_json::to_vec(&json!({ "repositoryName": "endpoints" }))
                            .expect("the request body serializes"),
                    ))
                    .expect("a well-formed request"),
            )
            .await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {label:?}: GetRepository, body {:?}",
            String::from_utf8_lossy(response.body())
        );
        let payload: Value = serde_json::from_slice(response.body()).expect("a JSON response");
        assert_eq!(
            string_at(&payload, "/repositoryMetadata/cloneUrlHttp"),
            expected_http,
            "case {label:?}: cloneUrlHttp"
        );
        assert_eq!(
            string_at(&payload, "/repositoryMetadata/cloneUrlSsh"),
            expected_ssh,
            "case {label:?}: cloneUrlSsh"
        );
    }
}

#[tokio::test]
async fn a_git_client_is_served_without_a_sigv4_signature() {
    // A git client signs nothing with SigV4: it authenticates with HTTPS git
    // credentials, which are not a SigV4 credential. The transport is therefore
    // served on a router that verifies signatures, while a control-plane call
    // with no signature on that same router is still refused.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(true);
    router.register_service(
        service_com_amazonaws_codecommit_codecommit_20150413::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    engine
        .state::<service_com_amazonaws_codecommit_codecommit_20150413::ComAmazonawsCodecommitCodecommit20150413State>()
        .lock()
        .repositories
        .insert(
            "unsigned".to_string(),
            service_com_amazonaws_codecommit_codecommit_20150413::state::Repository::new(
                "unsigned",
                None,
                None,
                BTreeMap::new(),
                "us-west-2",
                std::time::SystemTime::now(),
            ),
        );

    let reply = advertise(
        &router,
        &format!("https://{ENDPOINT}/v1/repos/unsigned"),
        "git-receive-pack",
    )
    .await;
    let (_, capabilities) = parse_advertisement("git-receive-pack", &reply);
    assert!(
        capabilities.iter().any(|cap| cap == "report-status"),
        "an unsigned git request must still be served, got {capabilities:?}"
    );

    let (status, payload) = call(&router, "GetRepository", &json!({ "repositoryName": "unsigned" })).await;
    assert_ne!(
        status, 200,
        "a control-plane call with an unverifiable signature must still be refused, got {payload}"
    );
}

#[tokio::test]
async fn a_reference_name_one_face_accepts_is_one_the_other_accepts() {
    // One rule for both faces: a push creates exactly the references
    // `CreateBranch` accepts, so the transport and the control plane cannot
    // disagree about which references a repository may hold. The rows span git's
    // own `check-ref-format` — every form it forbids — and the modelled length of
    // a branch name (256), one row at the maximum and one past it.
    let long_name = "a".repeat(256);
    let too_long = "a".repeat(257);
    let cases: Vec<(&str, String, bool)> = vec![
        ("plain", "feature".to_string(), true),
        ("slash-separated", "team/feature/one".to_string(), true),
        ("dotted", "release.1.2".to_string(), true),
        ("with an at sign", "user@host".to_string(), true),
        ("digits", "2".to_string(), true),
        ("at the modelled maximum", long_name, true),
        ("past the modelled maximum", too_long, false),
        ("a bare at sign", "@".to_string(), false),
        ("a reflog selector", "main@{0}".to_string(), false),
        ("an empty component", "a//b".to_string(), false),
        ("two dots", "a..b".to_string(), false),
        ("a leading dot", ".hidden".to_string(), false),
        ("a lock suffix", "busy.lock".to_string(), false),
        ("a trailing slash", "trailing/".to_string(), false),
        ("a trailing dot", "trailing.".to_string(), false),
        ("a space", "two words".to_string(), false),
        ("a tilde", "a~1".to_string(), false),
        ("a caret", "a^1".to_string(), false),
        ("a colon", "a:b".to_string(), false),
        ("a question mark", "a?b".to_string(), false),
        ("a star", "a*b".to_string(), false),
        ("a bracket", "a[b".to_string(), false),
        ("a backslash", "a\\b".to_string(), false),
        ("a control character", "a\u{1}b".to_string(), false),
    ];

    let router = emulator();
    let pushed = ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "ref-rule-push" }),
    )
    .await;
    let clone_url = string_at(&pushed, "/repositoryMetadata/cloneUrlHttp");
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "ref-rule-api" }),
    )
    .await;

    // One complete commit, carried by every push below so the closure is never
    // what refuses a command.
    let files = snapshot(&[("100644", "f.txt", b"one commit\n".to_vec())]);
    let (root, mut objects) = snapshot_objects(&files);
    let tip = commit(&root, &[], "base");
    let tip_id = id_of(&tip);
    objects.push(tip);
    // The control plane needs that commit to exist in its own repository too,
    // which is what `PutFile` writes there.
    let seeded = ok(
        &router,
        "PutFile",
        &json!({
            "repositoryName": "ref-rule-api",
            "branchName": "seed",
            "filePath": "f.txt",
            "fileContent": aws_smithy_types::base64::encode(b"one commit\n"),
        }),
    )
    .await;
    let api_commit = string_at(&seeded, "/commitId");

    for (label, name, acceptable) in cases {
        let reply = rpc(
            &router,
            &clone_url,
            "git-receive-pack",
            push_body(
                &[command(
                    NULL_ID,
                    &tip_id,
                    &format!("refs/heads/{name}"),
                    "report-status",
                )],
                &objects,
            ),
        )
        .await;
        let (report, _) = parse_pkts(&reply.body);
        let verdict = report.get(1).cloned().unwrap_or_default();
        let (status, payload) = call(
            &router,
            "CreateBranch",
            &json!({
                "repositoryName": "ref-rule-api",
                "branchName": name,
                "commitId": api_commit,
            }),
        )
        .await;
        if acceptable {
            assert!(
                verdict.starts_with("ok refs/heads/"),
                "case {label:?}: the push must create the reference, got {verdict:?}"
            );
            assert_eq!(
                status, 200,
                "case {label:?}: the control plane must create it too, got {status} with {payload}"
            );
        } else {
            assert!(
                verdict.contains("funny refname"),
                "case {label:?}: the push must refuse the reference, got {verdict:?}"
            );
            assert_eq!(
                status, 400,
                "case {label:?}: the control plane must refuse it too, got {status} with {payload}"
            );
            let code = payload
                .get("__type")
                .and_then(Value::as_str)
                .map(|shape| shape.rsplit('#').next().unwrap_or(shape).to_string())
                .unwrap_or_default();
            assert_eq!(
                code, "InvalidBranchNameException",
                "case {label:?}: the control plane's refusal names the branch name, got {payload}"
            );
        }
    }
}

/// The ref line a one-command push reports (`parse_pkts` gives `unpack …` first).
async fn one_command_verdict(
    router: &MultiServiceRouter,
    clone_url: &str,
    new: &str,
    name: &str,
    objects: &[Object],
) -> String {
    let reply = rpc(
        router,
        clone_url,
        "git-receive-pack",
        push_body(&[command(NULL_ID, new, name, "report-status")], objects),
    )
    .await;
    let (report, _) = parse_pkts(&reply.body);
    report.get(1).cloned().unwrap_or_default()
}

#[tokio::test]
async fn a_refusal_a_lookup_decides_is_answered_before_the_closure_walk() {
    // Walking a tip's whole object closure is the one expensive check a ref
    // command drives, and the number of commands is the CALLER's to choose. So
    // every refusal a lookup can decide — what kind of object the new value is,
    // how many refs the repository already holds — is answered first, and a
    // command that will be refused never pays for the walk.
    //
    // The rows all name a tip whose closure is INCOMPLETE, so the walk would
    // refuse each of them: what differs is only which check the server reaches
    // first, which is what makes the order observable rather than a claim about
    // timing. The last row is the control — with nothing cheaper to refuse it,
    // the walk runs and reports what is missing.
    const MAX_BRANCHES: usize =
        service_com_amazonaws_codecommit_codecommit_20150413::state::MAX_BRANCHES_PER_REPOSITORY;

    // A complete history, for the commands that are meant to be accepted.
    let files = snapshot(&[("100644", "f.txt", b"base\n".to_vec())]);
    let (root, base_objects) = snapshot_objects(&files);
    let base = commit(&root, &[], "base");
    let base_id = id_of(&base);
    let mut complete = base_objects.clone();
    complete.push(base);

    // An incomplete tip: a commit whose tree was never pushed. And a tag of a
    // commit that was never pushed either.
    let orphan = commit(&"0".repeat(40), &[], "its tree is not here");
    let orphan_id = id_of(&orphan);
    let dangling = annotated_tag(&orphan_id, "v9", "a tag of an absent commit");
    let dangling_id = id_of(&dangling);

    let router = emulator();

    // Row 1: the object KIND refuses it — a branch cannot point at a tag — and
    // that is a lookup, so it is answered rather than the walk's verdict.
    let created = ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "walk-order-kind" }),
    )
    .await;
    let clone_url = string_at(&created, "/repositoryMetadata/cloneUrlHttp");
    let verdict = one_command_verdict(
        &router,
        &clone_url,
        &dangling_id,
        "refs/heads/tagged",
        std::slice::from_ref(&dangling),
    )
    .await;
    assert_eq!(
        verdict, "ng refs/heads/tagged a branch cannot point at a tag\n",
        "case \"kind\": the lookup must decide it, got {verdict:?}"
    );

    // Row 2: the repository is at its branch bound — also a lookup, and also
    // answered before the walk.
    let created = ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "walk-order-bound" }),
    )
    .await;
    let clone_url = string_at(&created, "/repositoryMetadata/cloneUrlHttp");
    let commands: Vec<Vec<u8>> = (0..MAX_BRANCHES)
        .map(|index| {
            command(
                NULL_ID,
                &base_id,
                &format!("refs/heads/b{index}"),
                if index == 0 { "report-status" } else { "" },
            )
        })
        .collect();
    let reply = rpc(
        &router,
        &clone_url,
        "git-receive-pack",
        push_body(&commands, &complete),
    )
    .await;
    let (report, _) = parse_pkts(&reply.body);
    let accepted = report.iter().filter(|line| line.starts_with("ok ")).count();
    assert_eq!(
        accepted, MAX_BRANCHES,
        "case \"bound\": the repository must first be filled to its bound, got {accepted}"
    );
    let verdict = one_command_verdict(
        &router,
        &clone_url,
        &orphan_id,
        "refs/heads/one-more",
        std::slice::from_ref(&orphan),
    )
    .await;
    assert!(
        verdict.contains("branches this emulator keeps"),
        "case \"bound\": the ref bound must decide it, got {verdict:?}"
    );

    // Row 3, the control: nothing cheaper refuses this command, so the walk runs
    // and names the object the closure is missing.
    let created = ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "walk-order-walk" }),
    )
    .await;
    let clone_url = string_at(&created, "/repositoryMetadata/cloneUrlHttp");
    let verdict = one_command_verdict(
        &router,
        &clone_url,
        &orphan_id,
        "refs/heads/incomplete",
        std::slice::from_ref(&orphan),
    )
    .await;
    assert!(
        verdict.starts_with("ng refs/heads/incomplete missing necessary objects"),
        "case \"walk\": with nothing cheaper to refuse it, the walk must decide it, got {verdict:?}"
    );
    let code = error_code(
        &router,
        "GetBranch",
        &json!({ "repositoryName": "walk-order-walk", "branchName": "incomplete" }),
        400,
    )
    .await;
    assert_eq!(
        code, "BranchDoesNotExistException",
        "case \"walk\": a refused command must move no ref, got {code}"
    );
}
