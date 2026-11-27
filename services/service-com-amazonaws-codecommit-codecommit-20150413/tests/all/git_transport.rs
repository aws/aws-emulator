// SPDX-License-Identifier: CC0-1.0

//! The git smart-HTTP transport behind a CodeCommit repository, driven over the
//! wire through the router that serves the real emulator.
//!
//! The rules under test:
//!
//! 1. `RepositoryMetadata.cloneUrlHttp` is the origin THIS emulator serves the git
//!    transport at, published as the half of that one address a client that was
//!    given none of this emulator's trust material completes a request against — a
//!    clone URL is handed to a client and DIALLED with the configuration it already
//!    has — and that URL, not a rewritten one, is the address the git transport
//!    answers on (`info/refs`, `git-upload-pack`, `git-receive-pack`), dialled as it
//!    stands. The same address answers the TLS half in band for a client that holds
//!    this emulator's certificate.
//! 2. A push is stored as git objects and projected into the control plane: the
//!    commits and tags a client pushes are readable through `GetBranch`/`GetFile`
//!    at the ids the CLIENT computed.
//! 3. A fetch serves the complete object closure of the refs asked for,
//!    whichever face wrote them — a push or `PutFile`.
//! 4. A push whose declared old value is not where the ref now stands is refused
//!    by the server and leaves the ref unmoved.
//! 5. A reference a push wrote into the TAG namespace is a tag for both faces: the
//!    control plane resolves it as a commit SPECIFIER — `CommitName` is "the
//!    branch, tag, HEAD, or other fully qualified reference used to identify a
//!    commit" — and refuses it as a BRANCH name wherever an operation needs the
//!    branch it changes. A push is the only way a tag exists, so this is the one
//!    face that can create the state those refusals are about.
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
use crate::harness::error_with_message;
use crate::repository_lifecycle::{
    base64, call, emulator, emulator_with_engine, error as error_code, from_base64, ok, string_at,
};

/// The authority the shared harness addresses the emulator on, and so the one
/// every URL the emulator announces for itself must name (`crate::wire`).
const ENDPOINT: &str = "codecommit.us-west-2.amazonaws.com";

/// The all-zero object name: "no object" in a ref update command.
pub(crate) const NULL_ID: &str = "0000000000000000000000000000000000000000";

// ----------------------------------------------------------------- the emulator

/// The base64 blob content of a response, read through the shared decoder.
fn content_of(payload: &Value) -> Vec<u8> {
    from_base64(&string_at(payload, "/fileContent"))
}

/// One git transport response: status, `content-type` and body.
///
/// The git wire builders and parsers in this file are `pub(crate)` where a sibling
/// suite drives the same transport — [`crate::commit_date_across_faces`] reads the
/// commit object the transport serves — so there is ONE definition of a push, a
/// fetch and a packfile in this target rather than a second copy of git's wire
/// format.
pub(crate) struct GitReply {
    pub(crate) status: u16,
    pub(crate) content_type: String,
    pub(crate) body: Vec<u8>,
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
    git_as(router, method, url, content_type, body, Some(&git_user())).await
}

/// The `ServiceUserName` of the git credentials the cases here present: the IAM
/// service-specific credential for `codecommit.amazonaws.com` the shared harness holds
/// ([`crate::wire::hold_git_credential`]), for a user the account grants
/// `codecommit:GitPull` and `codecommit:GitPush`. The transport serves only credentials
/// the account ISSUED, so a suite driving it presents one it holds.
fn git_user() -> String {
    crate::wire::git_service_user_name()
}

/// The `Authorization` header a git client sends once it has been challenged: Basic
/// credentials whose user name is the git credential's and whose password is the
/// `ServicePassword` minted with it.
fn basic(user: &str) -> String {
    crate::wire::git_basic_credentials(user, crate::wire::GIT_SERVICE_PASSWORD)
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

pub(crate) async fn rpc(
    router: &MultiServiceRouter,
    clone_url: &str,
    service: &str,
    body: Vec<u8>,
) -> GitReply {
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
/// listener's own loopback authority under the half of that address a client dials as
/// it stands, with the repository on the path — and that `GetRepository` reports
/// the same value.
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
        "case {label:?}: cloneUrlHttp is dialled by a client holding none of this \
         emulator's trust material, so the announced origin is the half such a client \
         completes a request against: {clone_url:?}"
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
pub(crate) struct Object {
    pub(crate) kind: &'static str,
    pub(crate) data: Vec<u8>,
}

/// git's object name function: `SHA-1("<kind> <len>\0" + payload)`.
pub(crate) fn id_of(object: &Object) -> String {
    let mut hasher = sha1::Sha1::new();
    hasher.update(format!("{} {}\0", object.kind, object.data.len()).as_bytes());
    hasher.update(&object.data);
    hex::encode(hasher.finalize())
}

pub(crate) fn blob(content: &[u8]) -> Object {
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

/// A commit object over a tree, its parents and a message, with the two identity lines a
/// client chose for it — each `"<name> <<email>> <seconds> <±hhmm>"`, as git writes one.
///
/// `pub(crate)` so the sibling suite that reads WHICH DATE a commit carries on every face of
/// the service ([`crate::commit_date_across_faces`]) builds its pushed commits with this one
/// encoder rather than a second copy of git's commit encoding.
pub(crate) fn commit_with_identities(
    tree_id: &str,
    parents: &[String],
    message: &str,
    author: &str,
    committer: &str,
) -> Object {
    let mut text = format!("tree {tree_id}\n");
    for parent in parents {
        text.push_str(&format!("parent {parent}\n"));
    }
    text.push_str(&format!("author {author}\n"));
    text.push_str(&format!("committer {committer}\n"));
    text.push('\n');
    text.push_str(message);
    Object {
        kind: "commit",
        data: text.into_bytes(),
    }
}

/// A commit object over a tree, its parents and a message, authored by the pusher these
/// cases speak for at the instants they all use.
fn commit(tree_id: &str, parents: &[String], message: &str) -> Object {
    commit_with_identities(
        tree_id,
        parents,
        message,
        "Pusher <pusher@example.com> 1770000000 +0000",
        "Pusher <pusher@example.com> 1770000060 +0000",
    )
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
pub(crate) fn snapshot_objects(
    files: &BTreeMap<String, (String, Vec<u8>)>,
) -> (String, Vec<Object>) {
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
pub(crate) fn pack(objects: &[Object]) -> Vec<u8> {
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
pub(crate) fn unpack(bytes: &[u8]) -> Vec<Object> {
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
pub(crate) fn parse_pkts(body: &[u8]) -> (Vec<String>, Vec<u8>) {
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
pub(crate) fn parse_fetch(body: &[u8]) -> (Vec<String>, Vec<u8>) {
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
pub(crate) fn command(old: &str, new: &str, name: &str, capabilities: &str) -> Vec<u8> {
    if capabilities.is_empty() {
        pkt(format!("{old} {new} {name}\n").as_bytes())
    } else {
        pkt(format!("{old} {new} {name}\0{capabilities}").as_bytes())
    }
}

/// A whole push request: the commands, a flush, and the packfile.
pub(crate) fn push_body(commands: &[Vec<u8>], objects: &[Object]) -> Vec<u8> {
    let mut body: Vec<u8> = commands.concat();
    body.extend_from_slice(FLUSH);
    if !objects.is_empty() {
        body.extend_from_slice(&pack(objects));
    }
    body
}

/// A whole fetch request: the wants, a flush, and `done`.
pub(crate) fn fetch_body(wants: &[String], haves: &[String]) -> Vec<u8> {
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
    let seeded_key = crate::harness::seeded_encryption_key(&engine, "us-west-2");
    engine
        .state::<service_com_amazonaws_codecommit_codecommit_20150413::ComAmazonawsCodecommitCodecommit20150413State>()
        .lock()
        .repositories
        .insert(
            "hosts".to_string(),
            service_com_amazonaws_codecommit_codecommit_20150413::state::Repository::new(
                "hosts",
                None,
                seeded_key,
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
            .header("authorization", basic(&git_user()));
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
    let seeded_key = crate::harness::seeded_encryption_key(&engine, "us-west-2");
    engine
        .state::<service_com_amazonaws_codecommit_codecommit_20150413::ComAmazonawsCodecommitCodecommit20150413State>()
        .lock()
        .repositories
        .insert(
            "endpoints".to_string(),
            service_com_amazonaws_codecommit_codecommit_20150413::state::Repository::new(
                "endpoints",
                None,
                seeded_key,
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
    // The git credentials this request presents are ones the ACCOUNT issued, exactly as
    // on the shared harness: a git request carries no SigV4 signature, but it does carry
    // a credential, and the transport serves only one the account holds.
    crate::wire::hold_git_credential(&engine);
    let seeded_key = crate::harness::seeded_encryption_key(&engine, "us-west-2");
    engine
        .state::<service_com_amazonaws_codecommit_codecommit_20150413::ComAmazonawsCodecommitCodecommit20150413State>()
        .lock()
        .repositories
        .insert(
            "unsigned".to_string(),
            service_com_amazonaws_codecommit_codecommit_20150413::state::Repository::new(
                "unsigned",
                None,
                seeded_key,
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

// ------------------------------------------------- the tag namespace a push wrote

/// The tag names the cases below push: a lightweight tag on the base commit, an
/// annotated tag on the same commit, and a lightweight tag on the branch that is
/// one commit ahead.
const LIGHTWEIGHT_TAG: &str = "release-1";
const ANNOTATED_TAG: &str = "release-2";
const AHEAD_TAG: &str = "feature-1";

/// The repository the two cases below share, the branch names in it, and the
/// commits its references name.
const TAGGED_REPOSITORY: &str = "tagged-refs";
const TAGGED_DESTINATION: &str = "main";
const TAGGED_SOURCE: &str = "feature";

/// `BranchNameIsTagNameException`'s whole text, asserted as a literal rather than
/// read from the crate: a fixed sentence for the error class, naming neither the
/// reference, nor the repository, nor the member that carried it. The model
/// documents the class as "The specified branch name is not valid because it is a
/// tag name" (`smithy_query(query="shape",shape="BranchNameIsTagNameException")`).
const BRANCH_NAME_IS_TAG_NAME: &str = "The specified branch name is not valid. Branch names \
     cannot be tag names. Provide a valid branch name, and then try again.";

/// A repository whose `main` holds one commit, whose `feature` is one commit ahead
/// of it, and whose tag namespace holds the three tags above — each created the
/// only way one can be, by a push of `refs/tags/<tag>`.
struct Tagged {
    router: MultiServiceRouter,
    /// The commit `main` points at, which the lightweight and the annotated tag
    /// both name.
    base: String,
    /// The commit `feature` points at, which [`AHEAD_TAG`] names.
    ahead: String,
}

/// Build that repository, asserting the push's per-reference verdicts so a later
/// row cannot read as passing on a tag that was never created.
async fn tagged() -> Tagged {
    let router = emulator();
    let created = ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": TAGGED_REPOSITORY }),
    )
    .await;
    let clone_url = string_at(&created, "/repositoryMetadata/cloneUrlHttp");

    let written = ok(
        &router,
        "CreateCommit",
        &tagged_commit_body(TAGGED_DESTINATION, None, "base.txt", b"base\n"),
    )
    .await;
    let base = string_at(&written, "/commitId");
    ok(
        &router,
        "CreateBranch",
        &json!({
            "repositoryName": TAGGED_REPOSITORY,
            "branchName": TAGGED_SOURCE,
            "commitId": base,
        }),
    )
    .await;
    let written = ok(
        &router,
        "CreateCommit",
        &tagged_commit_body(TAGGED_SOURCE, Some(&base), "ahead.txt", b"ahead\n"),
    )
    .await;
    let ahead = string_at(&written, "/commitId");

    // One push creating all three tags. The annotated tag's object travels in the
    // pack; the commits the references name are already in the object store,
    // because the API face wrote them there.
    let tag_object = annotated_tag(&base, ANNOTATED_TAG, "release two\n");
    let annotated_id = id_of(&tag_object);
    let reply = rpc(
        &router,
        &clone_url,
        "git-receive-pack",
        push_body(
            &[
                command(
                    NULL_ID,
                    &base,
                    &format!("refs/tags/{LIGHTWEIGHT_TAG}"),
                    "report-status",
                ),
                command(
                    NULL_ID,
                    &annotated_id,
                    &format!("refs/tags/{ANNOTATED_TAG}"),
                    "",
                ),
                command(NULL_ID, &ahead, &format!("refs/tags/{AHEAD_TAG}"), ""),
            ],
            std::slice::from_ref(&tag_object),
        ),
    )
    .await;
    let (report, _) = parse_pkts(&reply.body);
    let expected: Vec<String> = vec![
        "unpack ok\n".to_string(),
        format!("ok refs/tags/{LIGHTWEIGHT_TAG}\n"),
        format!("ok refs/tags/{ANNOTATED_TAG}\n"),
        format!("ok refs/tags/{AHEAD_TAG}\n"),
    ];
    assert_eq!(
        report, expected,
        "the push must create all three tags, got {report:?}"
    );

    Tagged {
        router,
        base,
        ahead,
    }
}

/// A `CreateCommit` body writing one file on `branch` of that repository.
fn tagged_commit_body(branch: &str, parent: Option<&str>, path: &str, content: &[u8]) -> Value {
    let mut body = json!({
        "repositoryName": TAGGED_REPOSITORY,
        "branchName": branch,
        "commitMessage": format!("{branch}: {path}"),
        "putFiles": [{ "filePath": path, "fileContent": base64(content) }],
    });
    if let Some(parent) = parent {
        body["parentCommitId"] = json!(parent);
    }
    body
}

/// A merge body naming the two specifiers, and a `targetBranch` only where a row
/// asks for one.
fn tagged_merge_body(source: &str, destination: &str, target: Option<&str>) -> Value {
    let mut body = json!({
        "repositoryName": TAGGED_REPOSITORY,
        "sourceCommitSpecifier": source,
        "destinationCommitSpecifier": destination,
    });
    if let Some(target) = target {
        body["targetBranch"] = json!(target);
    }
    body
}

#[tokio::test]
async fn a_pushed_tag_resolves_as_a_commit_specifier() {
    let repository = tagged().await;

    // Every row names a commit through the tag namespace: by the tag's own name and
    // by its fully qualified reference, for a tag pointing straight at a commit and
    // for one pointing at a tag object that names it.
    let rows: Vec<(&str, String, &str, String, &[u8])> = vec![
        (
            "a lightweight tag by name",
            LIGHTWEIGHT_TAG.to_string(),
            "base.txt",
            repository.base.clone(),
            b"base\n",
        ),
        (
            "a lightweight tag by reference",
            format!("refs/tags/{LIGHTWEIGHT_TAG}"),
            "base.txt",
            repository.base.clone(),
            b"base\n",
        ),
        (
            "an annotated tag by name",
            ANNOTATED_TAG.to_string(),
            "base.txt",
            repository.base.clone(),
            b"base\n",
        ),
        (
            "an annotated tag by reference",
            format!("refs/tags/{ANNOTATED_TAG}"),
            "base.txt",
            repository.base.clone(),
            b"base\n",
        ),
        (
            "a tag on a commit the default branch does not hold",
            AHEAD_TAG.to_string(),
            "ahead.txt",
            repository.ahead.clone(),
            b"ahead\n",
        ),
        (
            "that tag by reference",
            format!("refs/tags/{AHEAD_TAG}"),
            "ahead.txt",
            repository.ahead.clone(),
            b"ahead\n",
        ),
    ];

    for (label, specifier, path, commit, content) in rows {
        let read = ok(
            &repository.router,
            "GetFile",
            &json!({
                "repositoryName": TAGGED_REPOSITORY,
                "commitSpecifier": specifier,
                "filePath": path,
            }),
        )
        .await;
        let read_commit = string_at(&read, "/commitId");
        assert_eq!(
            read_commit, commit,
            "case {label:?}: reading at {specifier:?} must report the commit that tag names, \
             got {read_commit:?}"
        );
        let read_content = content_of(&read);
        assert_eq!(
            read_content, content,
            "case {label:?}: the file at {specifier:?}, got {read_content:?}"
        );

        // The same resolution through another member the model types `CommitName`,
        // so the rule is the repository's rather than one operation's.
        let folder = ok(
            &repository.router,
            "GetFolder",
            &json!({
                "repositoryName": TAGGED_REPOSITORY,
                "commitSpecifier": specifier,
                "folderPath": "/",
            }),
        )
        .await;
        let folder_commit = string_at(&folder, "/commitId");
        assert_eq!(
            folder_commit, commit,
            "case {label:?}: GetFolder at {specifier:?} must report the same commit, got \
             {folder_commit:?}"
        );
    }

    // The contrast: a reference in the tag namespace this repository does NOT hold
    // still names no commit at all.
    let code = error_code(
        &repository.router,
        "GetFile",
        &json!({
            "repositoryName": TAGGED_REPOSITORY,
            "commitSpecifier": "refs/tags/never-pushed",
            "filePath": "base.txt",
        }),
        400,
    )
    .await;
    assert_eq!(
        code, "CommitDoesNotExistException",
        "case \"a tag the repository does not hold\": expected CommitDoesNotExistException, \
         got {code}"
    );

    // And the source side of a merge is a commit specifier like any other, so the
    // tag of the commit `feature` holds names it — and the branch really advances
    // there, which is what makes the refusals in the next case about the NAME.
    let merged = ok(
        &repository.router,
        "MergeBranchesByFastForward",
        &tagged_merge_body(
            &format!("refs/tags/{AHEAD_TAG}"),
            TAGGED_DESTINATION,
            None,
        ),
    )
    .await;
    let reported = string_at(&merged, "/commitId");
    assert_eq!(
        reported, repository.ahead,
        "case \"a tag as the source of a merge\": the merge must report the commit that tag \
         names, got {reported:?}"
    );
    let branch = ok(
        &repository.router,
        "GetBranch",
        &json!({
            "repositoryName": TAGGED_REPOSITORY,
            "branchName": TAGGED_DESTINATION,
        }),
    )
    .await;
    let tip = string_at(&branch, "/branch/commitId");
    assert_eq!(
        tip, repository.ahead,
        "case \"a tag as the source of a merge\": the target branch must be left at that \
         commit, got {tip:?}"
    );
    // The merge advanced a BRANCH: the tag namespace is only ever written by a push,
    // so the tag still names what it was pushed at.
    let read = ok(
        &repository.router,
        "GetFile",
        &json!({
            "repositoryName": TAGGED_REPOSITORY,
            "commitSpecifier": format!("refs/tags/{LIGHTWEIGHT_TAG}"),
            "filePath": "base.txt",
        }),
    )
    .await;
    let still = string_at(&read, "/commitId");
    assert_eq!(
        still, repository.base,
        "case \"a tag after a merge\": a tag must still name the commit it was pushed at, got \
         {still:?}"
    );
}

#[tokio::test]
async fn a_pushed_tag_is_no_branch_name_for_any_operation_that_needs_one() {
    let repository = tagged().await;
    let lightweight_reference = format!("refs/tags/{LIGHTWEIGHT_TAG}");
    let annotated_reference = format!("refs/tags/{ANNOTATED_TAG}");

    // The rows span the two members a merge can carry the name in — the explicit
    // target branch, and the destination specifier the target comes from when the
    // request names no target — each merge strategy, both forms of the reference,
    // both forms of the tag, and each write that commits on a branch the request
    // names.
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a fast-forward whose destination specifier is a tag name",
            "MergeBranchesByFastForward",
            tagged_merge_body(TAGGED_SOURCE, LIGHTWEIGHT_TAG, None),
        ),
        (
            "a fast-forward whose destination specifier is a tag reference",
            "MergeBranchesByFastForward",
            tagged_merge_body(TAGGED_SOURCE, &lightweight_reference, None),
        ),
        (
            "a fast-forward whose destination specifier is an annotated tag reference",
            "MergeBranchesByFastForward",
            tagged_merge_body(TAGGED_SOURCE, &annotated_reference, None),
        ),
        (
            "a fast-forward whose target branch is a tag name",
            "MergeBranchesByFastForward",
            tagged_merge_body(TAGGED_SOURCE, TAGGED_DESTINATION, Some(LIGHTWEIGHT_TAG)),
        ),
        (
            "a squash merge whose destination specifier is a tag reference",
            "MergeBranchesBySquash",
            tagged_merge_body(TAGGED_SOURCE, &lightweight_reference, None),
        ),
        (
            "a squash merge whose target branch is a tag name",
            "MergeBranchesBySquash",
            tagged_merge_body(TAGGED_SOURCE, TAGGED_DESTINATION, Some(ANNOTATED_TAG)),
        ),
        (
            "a three-way merge whose destination specifier is a tag reference",
            "MergeBranchesByThreeWay",
            tagged_merge_body(TAGGED_SOURCE, &lightweight_reference, None),
        ),
        (
            "a three-way merge whose target branch is a tag name",
            "MergeBranchesByThreeWay",
            tagged_merge_body(TAGGED_SOURCE, TAGGED_DESTINATION, Some(ANNOTATED_TAG)),
        ),
        (
            "a file written on a tag name",
            "PutFile",
            json!({
                "repositoryName": TAGGED_REPOSITORY,
                "branchName": LIGHTWEIGHT_TAG,
                "filePath": "written.txt",
                "fileContent": base64(b"written\n"),
            }),
        ),
        (
            "a file deleted from a tag name",
            "DeleteFile",
            json!({
                "repositoryName": TAGGED_REPOSITORY,
                "branchName": ANNOTATED_TAG,
                "filePath": "base.txt",
                "parentCommitId": repository.base.clone(),
            }),
        ),
        (
            "a commit made on a tag name",
            "CreateCommit",
            tagged_commit_body(LIGHTWEIGHT_TAG, None, "made.txt", b"made\n"),
        ),
    ];

    for (label, operation, body) in rows {
        let (code, message) = error_with_message(&repository.router, operation, &body, 400).await;
        assert_eq!(
            code, "BranchNameIsTagNameException",
            "case {label:?}: expected BranchNameIsTagNameException, got {code} with {message:?}"
        );
        assert_eq!(
            message, BRANCH_NAME_IS_TAG_NAME,
            "case {label:?}: the refusal's message is fixed for the class, got {message:?}"
        );
    }

    // The contrasts: a name the repository holds in NEITHER namespace is simply no
    // branch of it, and a destination specifier naming a commit without naming a
    // reference leaves the merge no target to be applied to. Neither is a tag name.
    let contrasts: Vec<(&str, Value, &str)> = vec![
        (
            "a target branch the repository does not hold",
            tagged_merge_body(TAGGED_SOURCE, TAGGED_DESTINATION, Some("never-created")),
            "BranchDoesNotExistException",
        ),
        (
            "a destination specifier naming a commit id and no target branch",
            tagged_merge_body(TAGGED_SOURCE, &repository.base, None),
            "InvalidTargetBranchException",
        ),
    ];
    for (label, body, expected) in contrasts {
        let code = error_code(
            &repository.router,
            "MergeBranchesByFastForward",
            &body,
            400,
        )
        .await;
        assert_eq!(
            code, expected,
            "case {label:?}: expected {expected}, got {code}"
        );
    }
}

// ------------------------------------------- the scheme the clone URL is announced on

/// The announced clone URL, read for its parts: its origin, with the repository
/// asserted on its path and the half of the one announced address a client holding no
/// trust material of this emulator's completes a request against asserted on its
/// origin. The returned value is `<scheme>://<authority>`.
fn assert_dialable_git_origin(label: &str, announced: &str, repository: &str) -> String {
    let (origin, path) = announced.split_once("/v1/repos/").unwrap_or_else(|| {
        panic!("case {label:?}: cloneUrlHttp carries the repository on a /v1/repos/ path, got {announced:?}")
    });
    assert_eq!(
        path, repository,
        "case {label:?}: cloneUrlHttp must carry this repository on its path, got \
         {announced:?}"
    );
    let authority = origin.strip_prefix("http://").unwrap_or_else(|| {
        panic!(
            "case {label:?}: cloneUrlHttp is dialled by a client that was given none of \
             this emulator's trust material, so the announced origin is the half such a \
             client completes a request against, got {announced:?}"
        )
    });
    let (host, port) = authority.rsplit_once(':').unwrap_or_else(|| {
        panic!("case {label:?}: the announced origin names no port: {announced:?}")
    });
    assert_eq!(
        host, "127.0.0.1",
        "case {label:?}: the announced origin is this emulator's own address, got \
         {announced:?}"
    );
    assert!(
        port.parse::<u16>().is_ok_and(|port| port > 0),
        "case {label:?}: the announced origin names the port the git listener bound, got \
         {announced:?}"
    );
    origin.to_string()
}

#[tokio::test]
async fn every_face_announces_the_clone_url_on_the_origin_the_transport_serves() {
    let router = emulator();
    // The rows span what the announced value varies with: the repository name's length
    // and the characters `RepositoryName` admits — `[\w\.-]{1,100}`, read from the
    // model's own constraint (smithy_query shape=RepositoryName) — including both of
    // its published bounds.
    let cases: Vec<(&str, String)> = vec![
        ("a one-character name", "a".to_string()),
        ("a plain name", "announced-repo".to_string()),
        (
            "every punctuation the name pattern admits",
            "Announced_repo.v2-3".to_string(),
        ),
        ("a name at the 100-character maximum", "n".repeat(100)),
    ];

    let mut origins: Vec<(String, String)> = Vec::new();
    for (label, name) in &cases {
        let created = ok(
            &router,
            "CreateRepository",
            &json!({ "repositoryName": name }),
        )
        .await;
        let announced = string_at(&created, "/repositoryMetadata/cloneUrlHttp");
    let origin = assert_dialable_git_origin(label, &announced, name);

        // The value comes from the announced git endpoint, not from a serializer, so
        // every face that reports a RepositoryMetadata reports the same URL.
        let read = ok(&router, "GetRepository", &json!({ "repositoryName": name })).await;
        assert_eq!(
            string_at(&read, "/repositoryMetadata/cloneUrlHttp"),
            announced,
            "case {label:?}: GetRepository announces the one origin this emulator serves \
             git at"
        );
        let batch = ok(
            &router,
            "BatchGetRepositories",
            &json!({ "repositoryNames": [name] }),
        )
        .await;
        assert_eq!(
            string_at(&batch, "/repositories/0/cloneUrlHttp"),
            announced,
            "case {label:?}: BatchGetRepositories announces the same origin"
        );

        // The SSH clone URL is the ssh form of the same path, on the endpoint the
        // asking request arrived at: this emulator serves no SSH transport of its own.
        for (face, payload, pointer) in [
            ("CreateRepository", &created, "/repositoryMetadata/cloneUrlSsh"),
            ("GetRepository", &read, "/repositoryMetadata/cloneUrlSsh"),
            ("BatchGetRepositories", &batch, "/repositories/0/cloneUrlSsh"),
        ] {
            assert_eq!(
                string_at(payload, pointer),
                format!("ssh://{ENDPOINT}/v1/repos/{name}"),
                "case {label:?}: {face} cloneUrlSsh"
            );
        }
        origins.push((label.to_string(), origin));
    }

    // ONE listener serves every repository, because the repository is named by the
    // path: the origin every row announces is therefore the same one.
    let (first_label, first) = origins
        .first()
        .cloned()
        .expect("the table holds at least one row");
    for (label, origin) in &origins {
        assert_eq!(
            origin, &first,
            "case {label:?}: every repository is announced on the origin case \
             {first_label:?} named, got {origin:?}"
        );
    }

    // A renamed repository is announced on the same origin, under its new name: the
    // URL is derived on every read rather than stored.
    let renamed = "announced-repo-renamed";
    ok(
        &router,
        "UpdateRepositoryName",
        &json!({ "oldName": "announced-repo", "newName": renamed }),
    )
    .await;
    let read = ok(
        &router,
        "GetRepository",
        &json!({ "repositoryName": renamed }),
    )
    .await;
    let announced = string_at(&read, "/repositoryMetadata/cloneUrlHttp");
    let origin = assert_dialable_git_origin("a renamed repository", &announced, renamed);
    assert_eq!(
        origin, first,
        "case \"a renamed repository\": the announced origin is unchanged by a rename, \
         got {origin:?}"
    );
}

/// What a dialled connection has to be: a plaintext socket, or the TLS stream over one.
trait Dialled: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> Dialled for T {}

/// The client side of the TLS the same address answers in band: the certificate is the
/// one the core generated for its own listeners, so this models a client that WAS given
/// this emulator's certificate — and this client is the emulator's own test dialling a
/// port this very process opened. A client given none of it verifies by default and is
/// refused in the handshake, which is what
/// [`the_tls_half_of_the_announced_address_is_refused_by_a_client_given_no_trust_material`]
/// pins and why the announced half is the clear one.
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

/// Dial `authority` — opening with a TLS handshake when `encrypted`, and in the clear
/// otherwise — send `request` as it stands and read everything the address answers.
///
/// The address dialled is this emulator's OWN: the announced authority is asserted to be
/// the loopback address the core binds on before the socket is opened on it.
///
/// Bytes rather than text, because a git request and its answer carry a packfile: the
/// text form below is the same exchange read as a string.
async fn exchange_bytes(
    label: &str,
    authority: &str,
    encrypted: bool,
    request: &[u8],
) -> Vec<u8> {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let (host, port) = authority
        .rsplit_once(':')
        .unwrap_or_else(|| panic!("case {label:?}: the origin names no port: {authority:?}"));
    assert_eq!(
        host, "127.0.0.1",
        "case {label:?}: the announced origin is this machine's own address, got \
         {authority:?}"
    );
    let port: u16 = port
        .parse()
        .unwrap_or_else(|error| panic!("case {label:?}: the port {port:?}: {error}"));
    let socket = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap_or_else(|error| {
            panic!("case {label:?}: {authority} must accept a connection: {error}")
        });
    let mut conn: Box<dyn Dialled> = if encrypted {
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
        Box::new(
            connector
                .connect(server_name, socket)
                .await
                .unwrap_or_else(|error| {
                    panic!(
                        "case {label:?}: the announced https origin must complete a TLS \
                         handshake: {error}"
                    )
                }),
        )
    } else {
        Box::new(socket)
    };
    conn.write_all(request)
        .await
        .unwrap_or_else(|error| panic!("case {label:?}: the request must be sent: {error}"));
    conn.flush()
        .await
        .unwrap_or_else(|error| panic!("case {label:?}: the request must be flushed: {error}"));
    let mut answer = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match conn.read(&mut chunk).await {
            Ok(0) => break,
            Ok(read) => answer.extend_from_slice(&chunk[..read]),
            // A server that closes the connection without a TLS close_notify ends the
            // answer too, which is what `Connection: close` asked it to do.
            Err(_) if !answer.is_empty() => break,
            Err(error) => panic!("case {label:?}: the answer must be read: {error}"),
        }
    }
    answer
}

/// [`exchange_bytes`] for a request and an answer that are text.
async fn exchange(label: &str, authority: &str, encrypted: bool, request: &str) -> String {
    let answer = exchange_bytes(label, authority, encrypted, request.as_bytes()).await;
    String::from_utf8_lossy(&answer).to_string()
}

#[tokio::test]
async fn the_announced_origin_serves_the_git_transport_it_names() {
    let router = emulator();
    let created = ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "dialled-as-announced" }),
    )
    .await;
    let announced = string_at(&created, "/repositoryMetadata/cloneUrlHttp");
    let origin =
        assert_dialable_git_origin("the announced origin", &announced, "dialled-as-announced");
    let authority = origin
        .strip_prefix("http://")
        .expect("the announced origin is the half a client dials as it stands")
        .to_string();

    // The rows span which half of the ONE announced address the client dials (the
    // origin the announcement names, which a client reaches with the configuration it
    // already has, and the TLS half the same port answers for a client that holds this
    // emulator's certificate), which git service it asks for, and whether it presents
    // git credentials yet — the three things the answer to a dialled clone URL varies
    // with.
    let cases: [(&str, bool, &str, bool, u16); 5] = [
        (
            "a clone over the announced origin",
            false,
            "git-upload-pack",
            true,
            200,
        ),
        (
            "a push over the announced origin",
            false,
            "git-receive-pack",
            true,
            200,
        ),
        (
            "a clone over it presenting no credential",
            false,
            "git-upload-pack",
            false,
            401,
        ),
        (
            "a clone over the TLS half of the same address",
            true,
            "git-upload-pack",
            true,
            200,
        ),
        (
            "a push over the TLS half of the same address",
            true,
            "git-receive-pack",
            true,
            200,
        ),
    ];
    for (label, encrypted, service, authenticated, expected) in cases {
        let mut request = format!(
            "GET /v1/repos/dialled-as-announced/info/refs?service={service} HTTP/1.1\r\n\
             Host: {authority}\r\nUser-Agent: git/2.43.0\r\nConnection: close\r\n"
        );
        if authenticated {
            request.push_str(&format!("Authorization: {}\r\n", basic(&git_user())));
        }
        request.push_str("\r\n");
        let answer = exchange(label, &authority, encrypted, &request).await;
        assert!(
            answer.starts_with(&format!("HTTP/1.1 {expected} ")),
            "case {label:?}: expected HTTP {expected} from the announced origin, got \
             {answer:?}"
        );
        if expected == 200 {
            assert!(
                answer.contains(&format!("# service={service}")),
                "case {label:?}: the answer must be the ref advertisement of {service}, got \
                 {answer:?}"
            );
        } else {
            assert!(
                answer.to_ascii_lowercase().contains("www-authenticate: basic"),
                "case {label:?}: a request presenting no credential must be challenged, got \
                 {answer:?}"
            );
        }
    }
}

// ------------------- a whole git session over the address the announcement names

#[tokio::test]
async fn the_tls_half_of_the_announced_address_is_refused_by_a_client_given_no_trust_material() {
    let router = emulator();
    let repository = "no-trust-material";
    let authority = announced_authority(&router, "the announced origin", repository).await;

    // WHY the announced half is the one dialled in the clear: a client is handed the
    // clone URL and nothing else, and it verifies a certificate the way it does by
    // default — against the anchors it already holds, none of which is this emulator's
    // own. That client is modelled here by the library's own verification over an empty
    // anchor set, and the TLS half of the address must refuse its handshake, before any
    // request is written. An origin it cannot complete a request against would not be
    // an address at all, which is what the announcement may not name.
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .expect("the provider supports the default protocol versions")
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    let port: u16 = authority
        .rsplit_once(':')
        .and_then(|(_host, port)| port.parse().ok())
        .unwrap_or_else(|| panic!("the announced authority names no port: {authority:?}"));
    let socket = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap_or_else(|error| panic!("{authority} must accept a connection: {error}"));
    let server_name =
        rustls::pki_types::ServerName::try_from("localhost").expect("localhost is a server name");
    let handshake = tokio::time::timeout(ANSWER_DEADLINE, connector.connect(server_name, socket))
        .await
        .unwrap_or_else(|_| {
            panic!("case \"a client given no trust material\": the handshake must be answered")
        });
    assert!(
        handshake.is_err(),
        "case \"a client given no trust material\": the TLS half of this address presents \
         this emulator's own certificate, which such a client holds no anchor for, so its \
         handshake cannot complete — had it completed, the announced half could have been \
         the TLS one"
    );

    // And the half the announcement DOES name serves that same client: it dials the
    // origin as it stands, with nothing but the credentials the account issued.
    let answer = dialled(
        "the announced origin",
        &authority,
        false,
        DialledRequest::advertisement(repository, "git-upload-pack", Some(basic(&git_user()))),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "the announced origin must serve the client the TLS half refuses, got {} with {:?}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert!(
        String::from_utf8_lossy(&answer.body).contains("# service=git-upload-pack"),
        "the announced origin must answer the fetch advertisement, got {:?}",
        String::from_utf8_lossy(&answer.body)
    );
}

/// How long one dialled exchange may take before the announced address counts as
/// having served nothing.
///
/// Generous for a loopback request this very process answers — the bound is here so
/// an address that answers NOTHING fails the case instead of leaving a client
/// waiting, which is the whole difference this suite is about; no case depends on
/// how fast the host runs.
const ANSWER_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);

/// One request a client writes on a connection it dialled the announced address with.
struct DialledRequest {
    method: &'static str,
    /// The request target: the repository's path under `/v1/repos/`, plus the
    /// transport endpoint and any query.
    target: String,
    content_type: Option<&'static str>,
    /// The `Authorization` header the request carries, `None` for one that presents
    /// no credential at all.
    authorization: Option<String>,
    body: Vec<u8>,
}

impl DialledRequest {
    /// The ref advertisement a fetch or a push starts with.
    fn advertisement(repository: &str, service: &str, authorization: Option<String>) -> Self {
        Self {
            method: "GET",
            target: format!("/v1/repos/{repository}/info/refs?service={service}"),
            content_type: None,
            authorization,
            body: Vec::new(),
        }
    }

    /// The pack request a fetch or a push continues with.
    fn pack(
        repository: &str,
        service: &str,
        body: Vec<u8>,
        authorization: Option<String>,
    ) -> Self {
        Self {
            method: "POST",
            target: format!("/v1/repos/{repository}/{service}"),
            content_type: Some(match service {
                "git-receive-pack" => "application/x-git-receive-pack-request",
                _ => "application/x-git-upload-pack-request",
            }),
            authorization,
            body,
        }
    }
}

/// One answer read off a dialled connection: its status, its header fields by
/// lower-cased name, and its body bytes.
struct DialledAnswer {
    status: u16,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

impl DialledAnswer {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }

    /// The same answer as a transport reply, so a dialled advertisement, fetch or
    /// push is read by the same parsers as one driven through the router.
    fn as_reply(&self) -> GitReply {
        GitReply {
            status: self.status,
            content_type: self.header("content-type").unwrap_or_default().to_string(),
            body: self.body.clone(),
        }
    }
}

/// Read one HTTP/1.1 answer: the status line, the header fields, and the body after
/// the empty line that ends them.
///
/// The body is taken as the bytes after the headers, which is what this address's
/// answers carry: every one of them is framed by `Content-Length`. A chunked answer is
/// refused here rather than handed back with its chunk framing still in it, so a change
/// of framing fails the case instead of corrupting what it parsed. Only the last value
/// of a repeated field is kept, which no field read here is sent more than once.
fn parse_answer(label: &str, answer: &[u8]) -> DialledAnswer {
    let split = answer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap_or_else(|| {
            panic!(
                "case {label:?}: the announced address must answer an HTTP response, got {:?}",
                String::from_utf8_lossy(answer)
            )
        });
    let head = String::from_utf8_lossy(&answer[..split]).to_string();
    let mut lines = head.lines();
    let status_line = lines
        .next()
        .unwrap_or_else(|| panic!("case {label:?}: the answer carries no status line"));
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| {
            panic!("case {label:?}: the status line {status_line:?} names no status")
        });
    let headers: BTreeMap<String, String> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    assert!(
        !headers.contains_key("transfer-encoding"),
        "case {label:?}: this reader takes the body as the bytes after the headers, so an \
         answer framed any other way must fail the case: {headers:?}"
    );
    DialledAnswer {
        status,
        headers,
        body: answer[split + 4..].to_vec(),
    }
}

/// Send `request` on a connection dialled at `authority` — opening with a TLS
/// handshake when `encrypted`, and in the clear, as the announcement names it,
/// otherwise — and read the answer.
///
/// The request is written as a stock client writes one: the `Host` of the address it
/// was handed, its own user agent, one request per connection, and the credentials it
/// has (a git client sends none until it has been challenged).
async fn dialled(
    label: &str,
    authority: &str,
    encrypted: bool,
    request: DialledRequest,
) -> DialledAnswer {
    let mut head = format!(
        "{} {} HTTP/1.1\r\nHost: {authority}\r\nUser-Agent: git/2.43.0\r\nConnection: close\r\n",
        request.method, request.target
    );
    if let Some(content_type) = request.content_type {
        head.push_str(&format!("Content-Type: {content_type}\r\n"));
    }
    if let Some(authorization) = &request.authorization {
        head.push_str(&format!("Authorization: {authorization}\r\n"));
    }
    head.push_str(&format!("Content-Length: {}\r\n\r\n", request.body.len()));
    let mut wire = head.into_bytes();
    wire.extend_from_slice(&request.body);
    let answer = tokio::time::timeout(
        ANSWER_DEADLINE,
        exchange_bytes(label, authority, encrypted, &wire),
    )
    .await
    .unwrap_or_else(|_| {
        panic!(
            "case {label:?}: the announced address must answer {} {} rather than leave a \
             client waiting",
            request.method, request.target
        )
    });
    parse_answer(label, &answer)
}

/// The authority of the clone URL `repository` is announced under, with the form of
/// that URL asserted: a client is handed this value and dials it as it stands.
async fn announced_authority(
    router: &MultiServiceRouter,
    label: &str,
    repository: &str,
) -> String {
    let created = ok(
        router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;
    let announced = string_at(&created, "/repositoryMetadata/cloneUrlHttp");
    let origin = assert_dialable_git_origin(label, &announced, repository);
    origin
        .split_once("://")
        .map(|(_scheme, authority)| authority.to_string())
        .unwrap_or_else(|| panic!("case {label:?}: the announced origin {origin:?} names none"))
}

/// Write `content` at `path` on `branch` through the API, on `parent` when the branch
/// already stands somewhere, and answer the commit the write made.
async fn put_file(
    router: &MultiServiceRouter,
    repository: &str,
    branch: &str,
    path: &str,
    content: &[u8],
    parent: Option<&str>,
) -> String {
    let mut input = json!({
        "repositoryName": repository,
        "branchName": branch,
        "filePath": path,
        "fileContent": base64(content),
        "commitMessage": format!("write {path}"),
    });
    if let Some(parent) = parent {
        input["parentCommitId"] = json!(parent);
    }
    string_at(&ok(router, "PutFile", &input).await, "/commitId")
}

/// The refs the announced address advertises for `service`, dialled as a client does.
async fn dialled_refs(
    label: &str,
    authority: &str,
    encrypted: bool,
    repository: &str,
    service: &str,
    credentials: &str,
) -> BTreeMap<String, String> {
    let answer = dialled(
        label,
        authority,
        encrypted,
        DialledRequest::advertisement(repository, service, Some(credentials.to_string())),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {label:?}: the announced address must serve the {service} advertisement, got \
         {} with {:?}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    parse_advertisement(service, &answer.as_reply()).0
}

#[tokio::test]
async fn a_whole_git_session_on_the_dialled_announced_address_round_trips_through_the_api() {
    let router = emulator();
    // The rows span which half of the ONE announced address a client dials: the half
    // the announcement names, which a client reaches with the configuration it already
    // has, and the TLS half the same port answers for a client that was given this
    // emulator's certificate. Everything else about the session is identical, because
    // WHICH scheme a request arrived under decides nothing about what the transport
    // serves.
    let halves: [(&str, bool, &str); 2] = [
        ("dialled as the announcement names it", false, "session.as-announced"),
        (
            "dialled over the TLS half of the same address",
            true,
            "session-tls",
        ),
    ];
    for (label, encrypted, repository) in halves {
        let authority = announced_authority(&router, label, repository).await;
        let credentials = basic(&git_user());

        // The API seeds the repository, exactly as a caller does before cloning it.
        let seeded: Vec<u8> = b"seed\n".to_vec();
        let seed_tip = put_file(&router, repository, "main", "README.md", &seeded, None).await;

        // 1. A CLONE: the advertisement names the commit the API wrote, and the pack
        //    request serves that commit's whole closure, so the content a client
        //    checks out is the content the API seeded.
        let refs = dialled_refs(
            label,
            &authority,
            encrypted,
            repository,
            "git-upload-pack",
            &credentials,
        )
        .await;
        assert_eq!(
            refs.get("refs/heads/main").map(String::as_str),
            Some(seed_tip.as_str()),
            "case {label:?}: the dialled advertisement must name the tip the API wrote, got \
             {refs:?}"
        );
        let branch = ok(
            &router,
            "GetBranch",
            &json!({ "repositoryName": repository, "branchName": "main" }),
        )
        .await;
        assert_eq!(
            string_at(&branch, "/branch/commitId"),
            seed_tip,
            "case {label:?}: the branch the API reports and the ref the address advertises \
             are one reference"
        );
        let answer = dialled(
            label,
            &authority,
            encrypted,
            DialledRequest::pack(
                repository,
                "git-upload-pack",
                fetch_body(std::slice::from_ref(&seed_tip), &[]),
                Some(credentials.clone()),
            ),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: a clone over the announced address must be served, got {} with \
             {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let (lines, packfile) = parse_fetch(&answer.body);
        assert_eq!(
            lines,
            vec!["NAK\n".to_string()],
            "case {label:?}: a NAK precedes the pack, got {lines:?}"
        );
        let served = unpack(&packfile);
        assert!(
            served.iter().any(|object| id_of(object) == seed_tip),
            "case {label:?}: the clone must serve the commit the API seeded"
        );
        assert!(
            served
                .iter()
                .any(|object| object.kind == "blob" && object.data == seeded),
            "case {label:?}: the clone must serve the file content the API seeded"
        );

        // 2. A FETCH after a further API write: the advertisement moves to the new
        //    commit, and the fetch that already has the old one is served only what it
        //    is missing.
        let appended: Vec<u8> = b"seed\nsecond\n".to_vec();
        let next_tip = put_file(
            &router,
            repository,
            "main",
            "README.md",
            &appended,
            Some(&seed_tip),
        )
        .await;
        let refs = dialled_refs(
            label,
            &authority,
            encrypted,
            repository,
            "git-upload-pack",
            &credentials,
        )
        .await;
        assert_eq!(
            refs.get("refs/heads/main").map(String::as_str),
            Some(next_tip.as_str()),
            "case {label:?}: a fetch must see the commit the later write created, got {refs:?}"
        );
        let answer = dialled(
            label,
            &authority,
            encrypted,
            DialledRequest::pack(
                repository,
                "git-upload-pack",
                fetch_body(
                    std::slice::from_ref(&next_tip),
                    std::slice::from_ref(&seed_tip),
                ),
                Some(credentials.clone()),
            ),
        )
        .await;
        let (lines, packfile) = parse_fetch(&answer.body);
        assert_eq!(
            lines,
            vec![format!("ACK {seed_tip}\n")],
            "case {label:?}: the commit the client already has is acknowledged, got {lines:?}"
        );
        let served = unpack(&packfile);
        assert!(
            served
                .iter()
                .any(|object| object.kind == "blob" && object.data == appended),
            "case {label:?}: the fetch must serve the content the later write created"
        );

        // 3. A PUSH of a new branch and an annotated tag, built by the CLIENT: both
        //    refs are accepted and are then visible through the API and in the next
        //    advertisement, at the ids the client computed.
        let files = snapshot(&[("100644", "pushed.txt", b"pushed\n".to_vec())]);
        let (root, mut objects) = snapshot_objects(&files);
        let pushed = commit(&root, std::slice::from_ref(&next_tip), "pushed");
        let pushed_id = id_of(&pushed);
        objects.push(pushed);
        let tag = annotated_tag(&pushed_id, "v1.0.0", "released\n");
        let tag_id = id_of(&tag);
        objects.push(tag);
        let answer = dialled(
            label,
            &authority,
            encrypted,
            DialledRequest::pack(
                repository,
                "git-receive-pack",
                push_body(
                    &[
                        command(NULL_ID, &pushed_id, "refs/heads/pushed", "report-status"),
                        command(NULL_ID, &tag_id, "refs/tags/v1.0.0", ""),
                    ],
                    &objects,
                ),
                Some(credentials.clone()),
            ),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: a push over the announced address must be answered with a status \
             report, got {} with {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let (report, _) = parse_pkts(&answer.body);
        assert_eq!(
            report,
            vec![
                "unpack ok\n".to_string(),
                "ok refs/heads/pushed\n".to_string(),
                "ok refs/tags/v1.0.0\n".to_string(),
            ],
            "case {label:?}: both refs must be accepted, got {report:?}"
        );
        let pushed_branch = ok(
            &router,
            "GetBranch",
            &json!({ "repositoryName": repository, "branchName": "pushed" }),
        )
        .await;
        assert_eq!(
            string_at(&pushed_branch, "/branch/commitId"),
            pushed_id,
            "case {label:?}: the branch the push created must stand at the commit the client \
             computed"
        );
        let listed = ok(
            &router,
            "ListBranches",
            &json!({ "repositoryName": repository }),
        )
        .await;
        let branches: Vec<String> = listed["branches"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .filter_map(|name| name.as_str().map(str::to_string))
            .collect();
        assert!(
            branches.contains(&"pushed".to_string()),
            "case {label:?}: a pushed branch must be listed, got {branches:?}"
        );
        let file = ok(
            &router,
            "GetFile",
            &json!({
                "repositoryName": repository,
                "commitSpecifier": "pushed",
                "filePath": "pushed.txt",
            }),
        )
        .await;
        assert_eq!(
            content_of(&file),
            b"pushed\n".to_vec(),
            "case {label:?}: the file the push carried must be readable through the API"
        );
        let refs = dialled_refs(
            label,
            &authority,
            encrypted,
            repository,
            "git-upload-pack",
            &credentials,
        )
        .await;
        assert_eq!(
            refs.get("refs/tags/v1.0.0").map(String::as_str),
            Some(tag_id.as_str()),
            "case {label:?}: the annotated tag must be advertised at the tag object the \
             client built, got {refs:?}"
        );

        // 4. A PUSH that DELETES that branch: accepted, and the reference is then gone
        //    from both faces.
        let answer = dialled(
            label,
            &authority,
            encrypted,
            DialledRequest::pack(
                repository,
                "git-receive-pack",
                push_body(
                    &[command(&pushed_id, NULL_ID, "refs/heads/pushed", "report-status")],
                    &[],
                ),
                Some(credentials.clone()),
            ),
        )
        .await;
        let (report, _) = parse_pkts(&answer.body);
        assert_eq!(
            report,
            vec![
                "unpack ok\n".to_string(),
                "ok refs/heads/pushed\n".to_string(),
            ],
            "case {label:?}: the deletion must be accepted, got {report:?}"
        );
        let code = error_code(
            &router,
            "GetBranch",
            &json!({ "repositoryName": repository, "branchName": "pushed" }),
            400,
        )
        .await;
        assert_eq!(
            code, "BranchDoesNotExistException",
            "case {label:?}: a branch a push deleted must not resolve"
        );
        let refs = dialled_refs(
            label,
            &authority,
            encrypted,
            repository,
            "git-upload-pack",
            &credentials,
        )
        .await;
        assert_eq!(
            refs.get("refs/heads/pushed"),
            None,
            "case {label:?}: a deleted branch must not be advertised, got {refs:?}"
        );
        assert_eq!(
            refs.get("refs/heads/main").map(String::as_str),
            Some(next_tip.as_str()),
            "case {label:?}: every other reference must stand where it stood, got {refs:?}"
        );
    }
}

/// The challenge the `401` of this transport carries, which is what makes a git client
/// ask its credential helper for credentials and send the request again: the scheme a
/// client presents git credentials under, and the realm CodeCommit names.
const BASIC_CHALLENGE: &str = "Basic realm=\"CodeCommit\"";

#[tokio::test]
async fn the_dialled_announced_address_challenges_a_credential_the_account_did_not_issue() {
    let (router, engine) = emulator_with_engine();
    let repository = "challenged-on-the-wire";
    let label = "the announced address";
    let authority = announced_authority(&router, label, repository).await;
    let tip = put_file(&router, repository, "main", "README.md", b"seed\n", None).await;

    // POSITIVE CONTROL FIRST: the pair the account issued is served on this very
    // address, dialled exactly as the announcement names it, so every refusal below is
    // attributable to the half its row changed rather than to the address serving
    // nothing at all.
    let answer = dialled(
        label,
        &authority,
        false,
        DialledRequest::advertisement(repository, "git-upload-pack", Some(basic(&git_user()))),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "the pair the account issued must be served on the announced address, got {} with {:?}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );

    // The rows span HOW a presentation is not the pair the account issued: none at
    // all, the right user name with a wrong password (longer, shorter and empty), and
    // a user name the account holds no credential for.
    let issued = crate::wire::GIT_SERVICE_PASSWORD;
    let rows: Vec<(&str, Option<String>)> = vec![
        ("no credential at all", None),
        (
            "the password with one more character",
            Some(crate::wire::git_basic_credentials(
                &git_user(),
                &format!("{issued}x"),
            )),
        ),
        (
            "the password one character short",
            Some(crate::wire::git_basic_credentials(
                &git_user(),
                &issued[..issued.len() - 1],
            )),
        ),
        (
            "no password at all",
            Some(crate::wire::git_basic_credentials(&git_user(), "")),
        ),
        (
            "a user name the account holds no credential for",
            Some(crate::wire::git_basic_credentials(
                &format!("Nobody-at-{}", engine::emulator_account_id()),
                issued,
            )),
        ),
    ];
    for (case, authorization) in rows {
        // Both endpoints of the transport: the advertisement a client starts with, and
        // the pack request it would continue with.
        for (endpoint, request) in [
            (
                "the fetch advertisement",
                DialledRequest::advertisement(
                    repository,
                    "git-upload-pack",
                    authorization.clone(),
                ),
            ),
            (
                "a push",
                DialledRequest::pack(
                    repository,
                    "git-receive-pack",
                    push_body(
                        &[command(&tip, NULL_ID, "refs/heads/main", "report-status")],
                        &[],
                    ),
                    authorization.clone(),
                ),
            ),
        ] {
            let case = format!("{case} at {endpoint}");
            let answer = dialled(&case, &authority, false, request).await;
            assert_eq!(
                answer.status, 401,
                "case {case:?}: a request the transport authenticates nobody for must be \
                 refused 401, got {} with {:?}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.header("www-authenticate"),
                Some(BASIC_CHALLENGE),
                "case {case:?}: the 401 must carry the challenge a client re-presents \
                 credentials under, got {:?}",
                answer.header("www-authenticate")
            );
            assert!(
                !String::from_utf8_lossy(&answer.body).contains("# service="),
                "case {case:?}: nothing of the advertisement may be served, got {:?}",
                String::from_utf8_lossy(&answer.body)
            );
        }
    }

    // Nothing any of those requests asked for happened: the reference stands where the
    // API left it.
    let branch = ok(
        &router,
        "GetBranch",
        &json!({ "repositoryName": repository, "branchName": "main" }),
    )
    .await;
    assert_eq!(
        string_at(&branch, "/branch/commitId"),
        tip,
        "a refused push must leave the reference where it stood"
    );

    // And a credential the account no longer holds is in the same position as one it
    // never issued, on the same address. The account's record is removed here because
    // this suite's subject is the ADDRESS; that the MODELLED revocations really remove
    // it — `DeleteServiceSpecificCredential`, a deactivation and a password reset,
    // driven through IAM's own API — is pinned by
    // `core/router/tests/all/codecommit_git_credentials_authenticate_only_what_iam_issued.rs`.
    {
        use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;

        let state = iam::service_state(&*engine).expect("the engine carries the IAM store");
        let mut store = state.lock();
        let held: Vec<String> = store
            .service_specific_credentials
            .iter()
            .filter(|(_, credential)| {
                credential.service_user_name.as_deref() == Some(git_user().as_str())
            })
            .map(|(id, _)| id.clone())
            .collect();
        assert!(
            !held.is_empty(),
            "the account must hold the credential this suite presents before it is deleted"
        );
        for id in held {
            store.service_specific_credentials.remove(&id);
            store.service_specific_credential_secrets.remove(&id);
        }
    }
    let case = "the credential the account deleted";
    let answer = dialled(
        case,
        &authority,
        false,
        DialledRequest::advertisement(repository, "git-upload-pack", Some(basic(&git_user()))),
    )
    .await;
    assert_eq!(
        answer.status, 401,
        "case {case:?}: a deleted credential authenticates nobody, got {} with {:?}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        answer.header("www-authenticate"),
        Some(BASIC_CHALLENGE),
        "case {case:?}: the 401 must carry the challenge, got {:?}",
        answer.header("www-authenticate")
    );
}
