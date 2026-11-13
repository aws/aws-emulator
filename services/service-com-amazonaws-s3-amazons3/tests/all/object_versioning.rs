// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for S3 OBJECT versioning: what a write mints, what a read can
//! address, and what a delete takes away.
//!
//! Every assertion drives a real HTTP request through the router (the harness is the one
//! [`super::bucket_lifecycle`] and [`super::object_and_tagging_roundtrip`] already own),
//! so what is pinned is the wire behaviour — status, bound headers, payload, error code —
//! rather than any handler's internals.
//!
//! The rule under test, asserted over a SET of inputs rather than one request:
//!
//! > On a bucket whose versioning status is `Enabled`, every object write stores a new
//! > version under a unique version id, reports that id to the writer, and every stored
//! > version stays independently readable by its id — and listable, newest first — until
//! > that version is deleted. A bucket whose status is not `Enabled` mints no id: its
//! > object is the `null` version.
//!
//! Its sources: "Enabled—Enables versioning for the objects in the bucket. All objects
//! added to the bucket receive a unique version ID. Suspended—Disables versioning for the
//! objects in the bucket. All objects added to the bucket receive the version ID null"
//! (`PutBucketVersioning`); "If you enable versioning for a bucket, Amazon S3
//! automatically generates a unique version ID for the object being stored. Amazon S3
//! returns this ID in the response" (`PutObjectOutput$VersionId`); "To retrieve a specific
//! version, you must be the bucket owner and you must use the versionId subresource"
//! (`GetObject`); "NextKeyMarker specifies the first key not returned that satisfies the
//! search criteria. Use this value for the key-marker request parameter in a subsequent
//! request" (`ListObjectVersionsOutput$NextKeyMarker`); "To permanently delete an object
//! in a versioned bucket, you must include the object's versionId in the request"
//! (`DeleteObject`).
//!
//! The version id itself is a value the SERVICE generates per write — `ObjectVersionId` is
//! an unconstrained string in the model — so nothing here asserts a literal id. What is
//! asserted is every invariant the rule gives it: it is non-empty, it is different for
//! every version in the store, it does not move between reads, and it is what addresses
//! the bytes that write stored.

use std::collections::{BTreeMap, BTreeSet};

use quick_xml::events::Event;
use quick_xml::Reader;
use service_com_amazonaws_s3_amazons3::state::MAX_VERSION_ID_CHARS;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, send_bytes, Answer};
use super::bucket_tagging::unescaped;
use super::object_and_tagging_roundtrip::{bucket, expected_etag, get_object, put_object, text};

/// The header S3 reports a stored version's id in.
const VERSION_ID_HEADER: &str = "x-amz-version-id";

/// `PUT /<bucket>?versioning` with the given `BucketVersioningStatus`, failing the test
/// when the service did not accept it.
///
/// `pub(crate)`, like [`list_object_versions`], [`version_listing_of`] and
/// [`VersionListing`]: the bucket-deletion-emptiness suite in this same test crate builds
/// its versioned buckets through these rather than standing up a second set.
pub(crate) async fn set_versioning(router: &MultiServiceRouter, name: &str, status: &str) {
    let payload = format!(
        "<VersioningConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
         <Status>{status}</Status></VersioningConfiguration>"
    );
    let answer = send(router, "PUT", &format!("/{name}?versioning"), &[], &payload).await;
    assert_eq!(
        answer.status,
        200,
        "the bucket {name:?} must take versioning status {status:?}, got {} with body {:?}",
        answer.status,
        text(&answer)
    );
}

/// `HEAD /<bucket>/<key><query>` — `HeadObject`, with whatever query the case carries.
pub(crate) async fn head_object(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    query: &str,
) -> Answer {
    send_bytes(router, "HEAD", &format!("/{name}/{key}{query}"), &[], b"").await
}

/// `DELETE /<bucket>/<key><query>` — `DeleteObject`, with whatever query the case carries.
pub(crate) async fn delete_object(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    query: &str,
) -> Answer {
    send_bytes(router, "DELETE", &format!("/{name}/{key}{query}"), &[], b"").await
}

/// `GET /<bucket>?versions<query>` — `ListObjectVersions`.
pub(crate) async fn list_object_versions(
    router: &MultiServiceRouter,
    name: &str,
    query: &str,
) -> Answer {
    send(router, "GET", &format!("/{name}?versions{query}"), &[], "").await
}

/// One `ListObjectVersions` response, as a client's XML reader sees it: the top-level
/// elements, one map per `Version` entry and the `CommonPrefixes` prefixes, each in
/// document order.
#[derive(Debug, Default)]
pub(crate) struct VersionListing {
    /// The elements directly under the result: `Name`, `IsTruncated`, `MaxKeys`, the
    /// markers and the echoed request parameters.
    top: BTreeMap<String, String>,
    /// One map per `Version` or `DeleteMarker` entry, keyed by element name — a member of
    /// the nested `Owner` structure under the container-qualified name `Owner.ID` — in
    /// response order.
    versions: Vec<BTreeMap<String, String>>,
    /// Every `CommonPrefixes/Prefix`, in response order.
    common_prefixes: Vec<String>,
}

impl VersionListing {
    /// A top-level element's text, or the empty string when the response carries none —
    /// which is what "absent" looks like to a client reading a bound member.
    ///
    /// `pub(crate)`, like [`VersionListing::carries`]: the suite that pins the echoed
    /// elements and the owner of every entry reads them through these.
    pub(crate) fn field(&self, name: &str) -> String {
        self.top.get(name).cloned().unwrap_or_default()
    }

    /// Whether the result document carries this top-level element AT ALL — which is what
    /// [`VersionListing::field`] cannot tell apart from an element carrying the empty
    /// string, and exactly the distinction an echoed marker turns on.
    pub(crate) fn carries(&self, name: &str) -> bool {
        self.top.contains_key(name)
    }

    /// Every listed entry, in response order — a `Version` or a `DeleteMarker`, whichever
    /// container carried it.
    pub(crate) fn listed(&self) -> &[BTreeMap<String, String>] {
        &self.versions
    }

    /// Every `CommonPrefixes/Prefix` the page rolled up, in response order.
    pub(crate) fn rolled_up(&self) -> &[String] {
        &self.common_prefixes
    }

    /// One entry's element, or the empty string when it carries none.
    pub(crate) fn entry(index: &BTreeMap<String, String>, name: &str) -> String {
        index.get(name).cloned().unwrap_or_default()
    }

    /// Every listed version as `(key, version id)`, in response order.
    pub(crate) fn entries(&self) -> Vec<(String, String)> {
        self.versions
            .iter()
            .map(|entry| {
                (
                    Self::entry(entry, "Key"),
                    Self::entry(entry, "VersionId"),
                )
            })
            .collect()
    }

    /// The listed entry whose `VersionId` is `version_id` — a `Version` or a
    /// `DeleteMarker`, whichever container carried it — or `None` when the listing reports
    /// no such version.
    pub(crate) fn entry_for(&self, version_id: &str) -> Option<BTreeMap<String, String>> {
        self.versions
            .iter()
            .find(|entry| Self::entry(entry, "VersionId") == version_id)
            .cloned()
    }
}

/// The pseudo-element [`version_listing_of`] records an entry's CONTAINER under: `Version`
/// or `DeleteMarker`. No element of the result document is named this, so it collides with
/// nothing a response carries.
pub(crate) const CONTAINER: &str = "@container";

/// Read a version listing with an XML reader, rather than by slicing the body — the same
/// accumulate-per-element walk [`super::object_storage::listing_of`] does for
/// `ListObjectsV2`, over this operation's `Version` and `DeleteMarker` containers.
pub(crate) fn version_listing_of(answer: &Answer) -> VersionListing {
    let body = String::from_utf8_lossy(&answer.body).into_owned();
    let mut reader = Reader::from_str(&body);

    let mut listing = VersionListing::default();
    let mut element = String::new();
    let mut current = String::new();
    let mut entry: Option<BTreeMap<String, String>> = None;
    let mut nested: Option<String> = None;
    let mut common_prefix: Option<String> = None;
    loop {
        match reader
            .read_event()
            .unwrap_or_else(|error| panic!("the listing body must be well-formed XML: {error}"))
        {
            Event::Start(start) => {
                element = String::from_utf8_lossy(start.name().as_ref()).into_owned();
                current.clear();
                match element.as_str() {
                    // The container an entry arrived in is recorded under a name no element
                    // of the document carries, so a reader can tell a version from a delete
                    // marker without re-reading the body.
                    "Version" | "DeleteMarker" => {
                        entry = Some(BTreeMap::from([(
                            CONTAINER.to_string(),
                            element.clone(),
                        )]))
                    }
                    "Owner" => nested = Some(element.clone()),
                    "CommonPrefixes" => common_prefix = Some(String::new()),
                    _ => {}
                }
            }
            // A self-closing element carries the empty string and is PRESENT: an echoed
            // marker the request named nothing for is written one way or the other
            // (`<KeyMarker></KeyMarker>` or `<KeyMarker/>`), and a client reading the
            // document sees the same element either way.
            Event::Empty(empty) => {
                let name = String::from_utf8_lossy(empty.name().as_ref()).into_owned();
                let field = match &nested {
                    Some(container) => format!("{container}.{name}"),
                    None => name.clone(),
                };
                match (entry.as_mut(), common_prefix.as_mut()) {
                    (Some(entry), _) => {
                        entry.insert(field, String::new());
                    }
                    // An empty element inside `CommonPrefixes` is that rolled-up prefix, not
                    // the listing's own echoed `Prefix` — the same split the `End` arm makes.
                    (None, Some(prefix)) if name == "Prefix" => prefix.clear(),
                    _ => {
                        listing.top.insert(field, String::new());
                    }
                }
                element.clear();
                current.clear();
            }
            Event::Text(text) => {
                let decoded = text
                    .xml10_content()
                    .unwrap_or_else(|error| panic!("the listing body must decode: {error}"));
                current.push_str(&unescaped(&decoded));
            }
            Event::GeneralRef(reference) => {
                let name = reference
                    .decode()
                    .unwrap_or_else(|error| panic!("the listing body must decode: {error}"));
                current.push_str(&unescaped(&format!("&{name};")));
            }
            Event::End(end) => {
                let closing = String::from_utf8_lossy(end.name().as_ref()).into_owned();
                match closing.as_str() {
                    "Version" | "DeleteMarker" => {
                        if let Some(done) = entry.take() {
                            listing.versions.push(done);
                        }
                    }
                    "Owner" => nested = None,
                    "CommonPrefixes" => {
                        if let Some(done) = common_prefix.take() {
                            listing.common_prefixes.push(done);
                        }
                    }
                    // Every other element's text belongs to whichever container is open,
                    // under the container-qualified name when it is a member of the `Owner`
                    // structure.
                    name if name == element => {
                        let value = std::mem::take(&mut current);
                        let field = match &nested {
                            Some(container) => format!("{container}.{name}"),
                            None => name.to_string(),
                        };
                        match (entry.as_mut(), common_prefix.as_mut()) {
                            (Some(entry), _) => {
                                entry.insert(field, value);
                            }
                            (None, Some(prefix)) if name == "Prefix" => *prefix = value,
                            _ => {
                                listing.top.insert(field, value);
                            }
                        }
                    }
                    _ => {}
                }
                element.clear();
                current.clear();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    listing
}

/// Write `revisions` to `key` in order, and hand back the version ids NEWEST FIRST —
/// which is the order a version listing reports them in.
async fn write_revisions(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    revisions: &[&[u8]],
) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for (index, body) in revisions.iter().enumerate() {
        let case = format!("{key} revision {index}");
        let id = write_version(router, name, key, &case, body)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: a versioned write must mint an id"));
        ids.insert(0, id);
    }
    ids
}

/// `length` bytes spanning the whole 0..=255 range, from a fixed seed, so a payload is
/// large and byte-varied without anything random: a 32-bit LCG (the "ANSI C" multiplier
/// and increment), taking the high byte of each step.
fn generated_bytes(length: usize, seed: u32) -> Vec<u8> {
    let mut state = seed;
    (0..length)
        .map(|_| {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            (state >> 16) as u8
        })
        .collect()
}

/// Write `body` to `key` and hand back the version id the write reported, asserting the
/// write was accepted and that it reported the payload's entity tag.
async fn write_version(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    case: &str,
    body: &[u8],
) -> Option<String> {
    let written = put_object(router, name, key, body, &[]).await;
    assert_eq!(
        written.status,
        200,
        "case {case:?}: PutObject must store the version, got {} with body {:?}",
        written.status,
        text(&written)
    );
    assert_eq!(
        written.header("etag"),
        Some(expected_etag(body)),
        "case {case:?}: expected the payload's entity tag, got {:?}",
        written.header("etag")
    );
    written.header(VERSION_ID_HEADER)
}

#[tokio::test]
async fn a_versioned_write_mints_a_unique_id_and_every_version_stays_readable() {
    let router = router();
    let name = "versioned-source";
    bucket(&router, name).await;
    set_versioning(&router, name, "Enabled").await;

    // (case, key, the revisions written to that key in order). The rows differ along what
    // the rule generalises over: how many versions a key accumulates (one, two, three),
    // the payload's size (empty, one byte, a kilobyte) and type (ASCII, UTF-8, bytes
    // including NUL and 0xFF), and the key's own shape (flat, prefixed, deep).
    let kilobyte = generated_bytes(1_024, 0x1234_5678);
    let other_kilobyte = generated_bytes(1_024, 0x8765_4321);
    let rows: [(&str, &str, Vec<&[u8]>); 5] = [
        ("one revision of a flat key", "source.zip", vec![b"first"]),
        (
            "two text revisions of a prefixed key",
            "builds/app/source.zip",
            vec![b"revision one", b"revision two"],
        ),
        (
            "three revisions of a deep key, growing from empty",
            "a/b/c/artifact.bin",
            vec![b"", b"x", kilobyte.as_slice()],
        ),
        (
            "two binary revisions including NUL and 0xFF",
            "binary.bin",
            vec![&[0u8, 0xFF, 0x80, 0x00][..], &[0xFF, 0xFE, 0u8][..]],
        ),
        (
            "two UTF-8 revisions of the same size",
            "notes/ünïcode.txt",
            vec![
                "héllo wörld".as_bytes(),
                other_kilobyte.as_slice(),
            ],
        ),
    ];

    // Every id this bucket mints, so the uniqueness claim is over the whole store rather
    // than over one key.
    let mut minted: BTreeSet<String> = BTreeSet::new();

    for (case, key, revisions) in &rows {
        let mut stored: Vec<(String, &[u8])> = Vec::new();
        for (index, body) in revisions.iter().enumerate() {
            let case = format!("{case} (revision {index})");
            let version_id = write_version(&router, name, key, &case, body).await;
            let Some(version_id) = version_id else {
                panic!(
                    "case {case:?}: a write into a versioning-enabled bucket must report \
                     {VERSION_ID_HEADER}, got none"
                );
            };
            assert!(
                !version_id.is_empty()
                    && version_id != "null"
                    && !version_id.contains(char::is_whitespace),
                "case {case:?}: expected a non-empty, header-safe version id that is not \
                 the null version, got {version_id:?}"
            );
            assert!(
                minted.insert(version_id.clone()),
                "case {case:?}: expected a version id no other version carries, got the \
                 already-minted {version_id:?}"
            );
            stored.push((version_id, body));
        }

        // Every version written above is still readable BY ITS ID, with exactly the bytes
        // that write carried — including the ones a later write superseded.
        for (index, (version_id, body)) in stored.iter().enumerate() {
            let case = format!("{case} (revision {index} read back by id)");
            let read = get_object(&router, name, key, &format!("?versionId={version_id}"), &[]).await;
            assert_eq!(
                read.status,
                200,
                "case {case:?}: expected HTTP 200 for version {version_id:?}, got {} with \
                 body {:?}",
                read.status,
                text(&read)
            );
            assert_eq!(
                read.body.as_ref(),
                *body,
                "case {case:?}: expected the {} bytes that version stored, got {} bytes",
                body.len(),
                read.body.len()
            );
            assert_eq!(
                read.header(VERSION_ID_HEADER).as_deref(),
                Some(version_id.as_str()),
                "case {case:?}: a version read must report its own id, got {:?}",
                read.header(VERSION_ID_HEADER)
            );
            assert_eq!(
                read.header("etag"),
                Some(expected_etag(body)),
                "case {case:?}: expected that version's entity tag, got {:?}",
                read.header("etag")
            );

            // A HEAD of the same version agrees with the GET, and neither read moves the
            // id: a second read answers exactly what the first did.
            let headed = head_object(&router, name, key, &format!("?versionId={version_id}")).await;
            assert_eq!(
                headed.status, 200,
                "case {case:?}: HeadObject must answer for version {version_id:?}, got {}",
                headed.status
            );
            assert_eq!(
                headed.header("content-length"),
                Some(body.len().to_string()),
                "case {case:?}: expected content-length {}, got {:?}",
                body.len(),
                headed.header("content-length")
            );
            assert_eq!(
                headed.header(VERSION_ID_HEADER).as_deref(),
                Some(version_id.as_str()),
                "case {case:?}: HeadObject must report the version's own id, got {:?}",
                headed.header(VERSION_ID_HEADER)
            );
        }

        // A read that names no version answers with the CURRENT one: the last write's
        // bytes, under the last write's id.
        let (current_id, current_body) = stored
            .last()
            .expect("every row writes at least one revision");
        let current = get_object(&router, name, key, "", &[]).await;
        assert_eq!(
            current.status, 200,
            "case {case:?}: a read without a versionId must answer, got {}",
            current.status
        );
        assert_eq!(
            current.body.as_ref(),
            *current_body,
            "case {case:?}: expected the newest revision's bytes from the current version"
        );
        assert_eq!(
            current.header(VERSION_ID_HEADER).as_deref(),
            Some(current_id.as_str()),
            "case {case:?}: expected the current version id {current_id:?}, got {:?}",
            current.header(VERSION_ID_HEADER)
        );
    }
}

#[tokio::test]
async fn a_bucket_whose_versioning_is_not_enabled_mints_no_version_id() {
    let router = router();

    // (case, bucket, the versioning statuses applied in order). The rows span every state
    // a bucket's versioning can be in other than plain `Enabled`: never set at all, set
    // straight to `Suspended`, and `Enabled` then suspended again.
    let rows: [(&str, &str, &[&str]); 3] = [
        ("a bucket whose versioning was never set", "plain-bucket", &[]),
        ("a bucket suspended from the start", "suspended-bucket", &["Suspended"]),
        (
            "a bucket enabled and then suspended",
            "resuspended-bucket",
            &["Enabled", "Suspended"],
        ),
    ];

    for (case, name, statuses) in rows {
        bucket(&router, name).await;

        // On the row that enables versioning first, one version IS minted; it must stay
        // addressable after the bucket is suspended, which is what makes suspension a
        // state of the BUCKET rather than of the versions it already holds.
        let mut enabled_version: Option<(String, &[u8])> = None;
        for status in statuses {
            if *status == "Enabled" {
                set_versioning(&router, name, status).await;
                let body: &[u8] = b"written while enabled";
                let minted = write_version(&router, name, "object.txt", case, body).await;
                let minted = minted.unwrap_or_else(|| {
                    panic!("case {case:?}: an enabled bucket must mint a version id")
                });
                enabled_version = Some((minted, body));
                continue;
            }
            set_versioning(&router, name, status).await;
        }

        // The write made while the bucket is not versioning-enabled mints nothing.
        let body: &[u8] = b"written while not enabled";
        let written = write_version(&router, name, "object.txt", case, body).await;
        assert_eq!(
            written, None,
            "case {case:?}: a write into a bucket that is not versioning-enabled must \
             report no {VERSION_ID_HEADER}, got {written:?}"
        );

        // And the object it stored is the `null` version: a read naming no version and a
        // read naming `null` answer the same bytes, and neither reports an id.
        for (label, query) in [("no versionId", ""), ("versionId=null", "?versionId=null")] {
            let read = get_object(&router, name, "object.txt", query, &[]).await;
            assert_eq!(
                read.status,
                200,
                "case {case:?} with {label}: expected HTTP 200, got {} with body {:?}",
                read.status,
                text(&read)
            );
            assert_eq!(
                read.body.as_ref(),
                body,
                "case {case:?} with {label}: expected the stored bytes back"
            );
            assert_eq!(
                read.header(VERSION_ID_HEADER),
                None,
                "case {case:?} with {label}: the null version carries no id, got {:?}",
                read.header(VERSION_ID_HEADER)
            );
        }

        // The version minted while the bucket WAS enabled is still there, by its id, with
        // the bytes it stored — the suspension did not take it away.
        if let Some((version_id, enabled_body)) = enabled_version {
            let read = get_object(
                &router,
                name,
                "object.txt",
                &format!("?versionId={version_id}"),
                &[],
            )
            .await;
            assert_eq!(
                read.status,
                200,
                "case {case:?}: the version minted while enabled must still be readable, \
                 got {} with body {:?}",
                read.status,
                text(&read)
            );
            assert_eq!(
                read.body.as_ref(),
                enabled_body,
                "case {case:?}: expected the bytes stored while the bucket was enabled"
            );
        }
    }
}

#[tokio::test]
async fn a_version_id_the_bucket_holds_no_version_for_is_not_found() {
    let router = router();
    let name = "versioned-source";
    bucket(&router, name).await;
    set_versioning(&router, name, "Enabled").await;
    let minted = write_version(&router, name, "object.txt", "the one stored version", b"held")
        .await
        .expect("an enabled bucket mints a version id");

    // Ids that are SHAPED like a version id and name no stored version, differing in how:
    // one of the minted shape this store never minted, one differing from the stored id in
    // its last character, and `null` — which names the version a write makes while the bucket
    // is NOT enabled, and this key has none, every write to it having been versioned. An id
    // that is not shaped like one at all is a different answer; the table below this one
    // covers it.
    let mut almost = minted.clone();
    let last = almost.pop().expect("a minted version id is not empty");
    // Another hexadecimal digit, so the id stays shaped like one the store could have minted.
    almost.push(if last == '0' { '1' } else { '0' });
    let rows: [(&str, String); 3] = [
        (
            "an id of the minted shape that was never minted",
            "0123456789abcdef0123456789abcdef".to_string(),
        ),
        ("an id differing in one character", almost),
        ("the null version of a key that has none", "null".to_string()),
    ];

    for (case, version_id) in rows {
        let read = get_object(
            &router,
            name,
            "object.txt",
            &format!("?versionId={version_id}"),
            &[],
        )
        .await;
        assert_eq!(
            read.status,
            404,
            "case {case:?}: expected HTTP 404 for {version_id:?}, got {} with body {:?}",
            read.status,
            text(&read)
        );
        assert_eq!(
            read.error_code(),
            "NoSuchVersion",
            "case {case:?}: expected NoSuchVersion, got {:?}",
            read.error_code()
        );

        let headed = head_object(&router, name, "object.txt", &format!("?versionId={version_id}")).await;
        assert_eq!(
            headed.status, 404,
            "case {case:?}: HeadObject must answer 404 for {version_id:?}, got {}",
            headed.status
        );
        assert_eq!(
            headed.error_code(),
            "NoSuchVersion",
            "case {case:?}: expected NoSuchVersion from HeadObject, got {:?}",
            headed.error_code()
        );

        // A refused read changes nothing: the stored version is still there, by its id.
        let still = get_object(
            &router,
            name,
            "object.txt",
            &format!("?versionId={minted}"),
            &[],
        )
        .await;
        assert_eq!(
            still.status, 200,
            "case {case:?}: the stored version must survive a refused read, got {}",
            still.status
        );
    }
}

#[tokio::test]
async fn a_version_id_that_is_not_shaped_like_one_is_a_malformed_argument() {
    let router = router();
    let name = "versioned-source";
    bucket(&router, name).await;
    set_versioning(&router, name, "Enabled").await;
    let minted = write_version(&router, name, "object.txt", "the one stored version", b"held")
        .await
        .expect("an enabled bucket mints a version id");

    // A `versionId` is either the literal `null` or a token of the published shape — a
    // width between the published bounds, each character an alphanumeric or one of `-`,
    // `_`, `.` — so anything else cannot name a version of any object in any bucket. That
    // is a malformed ARGUMENT, which S3 answers `InvalidArgument` with HTTP 400, never the
    // `NoSuchVersion`/404 it reserves for a well-formed id that names no stored version.
    //
    // The rows differ along what the shape rule generalises over: LENGTH outside the
    // published range at both ends (one character, one short of the minimum, one past the
    // maximum) and the ALPHABET at a width inside it (a character outside the published
    // class, and the two characters standard Base64 uses, which are not in it).
    let short = minted[..minted.len() - 1].to_string();
    let plus = format!("{}+", &minted[..minted.len() - 1]);
    let slash = format!("{}/", &minted[..minted.len() - 1]);
    let bang = format!("{}!", &minted[..minted.len() - 1]);
    let rows: [(&str, String); 6] = [
        ("a one-character id", "x".to_string()),
        ("one character short of the minimum width", short),
        (
            "one character past the maximum width",
            "v".repeat(MAX_VERSION_ID_CHARS + 1),
        ),
        ("a word that is not an id at all", "not-a-version".to_string()),
        (
            "standard Base64's plus, which the published class does not hold",
            plus,
        ),
        (
            "a character outside the published class entirely",
            bang,
        ),
    ];

    for (case, version_id) in rows.into_iter().chain([(
        "standard Base64's slash, which would end the path segment",
        slash,
    )]) {
        for (operation, answer) in [
            (
                "GetObject",
                get_object(
                    &router,
                    name,
                    "object.txt",
                    &format!("?versionId={version_id}"),
                    &[],
                )
                .await,
            ),
            (
                "HeadObject",
                head_object(&router, name, "object.txt", &format!("?versionId={version_id}"))
                    .await,
            ),
            (
                "DeleteObject",
                delete_object(&router, name, "object.txt", &format!("?versionId={version_id}"))
                    .await,
            ),
        ] {
            assert_eq!(
                answer.status, 400,
                "case {case:?}/{operation}: a versionId that is not shaped like one is a \
                 malformed argument, got {} for {version_id:?} with body {:?}",
                answer.status,
                text(&answer)
            );
            if operation != "HeadObject" {
                // A HEAD carries no body, so there is no error document to read a code
                // out of; the status is the whole answer.
                assert_eq!(
                    answer.error_code(),
                    "InvalidArgument",
                    "case {case:?}/{operation}: expected InvalidArgument, got {:?}",
                    answer.error_code()
                );
            }
        }

        // None of the refusals touched the stored version.
        let still = get_object(
            &router,
            name,
            "object.txt",
            &format!("?versionId={minted}"),
            &[],
        )
        .await;
        assert_eq!(
            still.status, 200,
            "case {case:?}: the stored version must survive a refused call, got {}",
            still.status
        );
    }
}

#[tokio::test]
async fn deleting_a_version_takes_away_only_that_version() {
    let router = router();
    let name = "versioned-source";
    bucket(&router, name).await;
    set_versioning(&router, name, "Enabled").await;

    // Three versions of one key, so a delete has a version before it AND after it to
    // leave alone.
    let bodies: [&[u8]; 3] = [b"first", b"second", b"third"];
    let mut versions: Vec<(String, &[u8])> = Vec::new();
    for (index, body) in bodies.iter().enumerate() {
        let case = format!("revision {index}");
        let id = write_version(&router, name, "object.txt", &case, body)
            .await
            .expect("an enabled bucket mints a version id");
        versions.push((id, body));
    }

    // Deleting the MIDDLE version leaves both the one before it and the current one.
    let (middle_id, _) = versions[1].clone();
    let deleted = delete_object(
        &router,
        name,
        "object.txt",
        &format!("?versionId={middle_id}"),
    )
    .await;
    assert_eq!(
        deleted.status,
        204,
        "case \"delete the middle version\": expected HTTP 204, got {} with body {:?}",
        deleted.status,
        text(&deleted)
    );
    for (case, version_id, expected) in [
        ("the version before it", versions[0].0.clone(), Some(bodies[0])),
        ("the deleted version itself", middle_id.clone(), None),
        ("the current version", versions[2].0.clone(), Some(bodies[2])),
    ] {
        let read = get_object(
            &router,
            name,
            "object.txt",
            &format!("?versionId={version_id}"),
            &[],
        )
        .await;
        match expected {
            Some(body) => {
                assert_eq!(
                    read.status,
                    200,
                    "case {case:?}: expected HTTP 200, got {} with body {:?}",
                    read.status,
                    text(&read)
                );
                assert_eq!(
                    read.body.as_ref(),
                    body,
                    "case {case:?}: expected that version's own bytes back"
                );
            }
            None => {
                assert_eq!(
                    read.status, 404,
                    "case {case:?}: a deleted version must be gone, got {}",
                    read.status
                );
                assert_eq!(
                    read.error_code(),
                    "NoSuchVersion",
                    "case {case:?}: expected NoSuchVersion, got {:?}",
                    read.error_code()
                );
            }
        }
    }

    // Deleting the same version again answers the same 204: this operation models no error
    // for an address that holds nothing, so a repeated delete is a no-op success rather than
    // a refusal — and it still takes nothing else away (asserted below).
    let again = delete_object(
        &router,
        name,
        "object.txt",
        &format!("?versionId={middle_id}"),
    )
    .await;
    assert_eq!(
        again.status, 204,
        "case \"delete the middle version twice\": expected HTTP 204, got {} with body {:?}",
        again.status,
        text(&again)
    );
    assert!(
        again.header("x-amz-version-id").is_none(),
        "case \"delete the middle version twice\": a delete that removed no version reports \
         none, got {:?}",
        again.header("x-amz-version-id")
    );

    // Deleting the CURRENT version promotes the newest version left: a read naming no
    // version answers with the first revision's bytes and its id.
    let (current_id, _) = versions[2].clone();
    let dropped = delete_object(
        &router,
        name,
        "object.txt",
        &format!("?versionId={current_id}"),
    )
    .await;
    assert_eq!(
        dropped.status, 204,
        "case \"delete the current version\": expected HTTP 204, got {}",
        dropped.status
    );
    let promoted = get_object(&router, name, "object.txt", "", &[]).await;
    assert_eq!(
        promoted.status, 200,
        "case \"delete the current version\": the version left must become current, got {}",
        promoted.status
    );
    assert_eq!(
        promoted.body.as_ref(),
        bodies[0],
        "case \"delete the current version\": expected the remaining version's bytes"
    );
    assert_eq!(
        promoted.header(VERSION_ID_HEADER).as_deref(),
        Some(versions[0].0.as_str()),
        "case \"delete the current version\": expected the remaining version's id, got {:?}",
        promoted.header(VERSION_ID_HEADER)
    );

    // A delete that names NO version removes the key from the current view — "If bucket
    // versioning is enabled, the operation inserts a delete marker, which becomes the
    // current version of the object" — while the version it displaced stays addressable
    // by its id.
    let displaced = delete_object(&router, name, "object.txt", "").await;
    assert_eq!(
        displaced.status, 204,
        "case \"delete with no versionId\": expected HTTP 204, got {}",
        displaced.status
    );
    let absent = get_object(&router, name, "object.txt", "", &[]).await;
    assert_eq!(
        absent.status, 404,
        "case \"delete with no versionId\": the key must read as absent, got {}",
        absent.status
    );
    assert_eq!(
        absent.error_code(),
        "NoSuchKey",
        "case \"delete with no versionId\": expected NoSuchKey, got {:?}",
        absent.error_code()
    );
    let retained = get_object(
        &router,
        name,
        "object.txt",
        &format!("?versionId={}", versions[0].0),
        &[],
    )
    .await;
    assert_eq!(
        retained.status, 200,
        "case \"delete with no versionId\": the displaced version must stay addressable, \
         got {}",
        retained.status
    );
    assert_eq!(
        retained.body.as_ref(),
        bodies[0],
        "case \"delete with no versionId\": expected the displaced version's bytes"
    );

    // And naming that version explicitly is what permanently removes it.
    let purged = delete_object(
        &router,
        name,
        "object.txt",
        &format!("?versionId={}", versions[0].0),
    )
    .await;
    assert_eq!(
        purged.status, 204,
        "case \"permanently delete the displaced version\": expected HTTP 204, got {}",
        purged.status
    );
    let gone = get_object(
        &router,
        name,
        "object.txt",
        &format!("?versionId={}", versions[0].0),
        &[],
    )
    .await;
    assert_eq!(
        gone.status, 404,
        "case \"permanently delete the displaced version\": expected HTTP 404, got {}",
        gone.status
    );
}

#[tokio::test]
async fn a_version_listing_reports_every_stored_version_newest_first() {
    let router = router();
    let name = "versioned-source";
    bucket(&router, name).await;
    set_versioning(&router, name, "Enabled").await;

    // (key, its revisions). The rows differ in how many versions a key accumulates and in
    // the shape of the key, and they are WRITTEN out of key order, so a listing that
    // reported insertion order rather than key order would fail.
    let rows: [(&str, Vec<&[u8]>); 3] = [
        ("source.zip", vec![b"one", b"two", b"three"]),
        ("builds/app.zip", vec![b"only"]),
        ("a/deep/key.txt", vec![b"first", b"second"]),
    ];
    let mut expected: Vec<(String, String)> = Vec::new();
    let mut by_key: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for (key, revisions) in &rows {
        by_key.insert(key, write_revisions(&router, name, key, revisions).await);
    }
    // Ascending key order, and within a key newest version first.
    for (key, ids) in &by_key {
        for id in ids {
            expected.push(((*key).to_string(), id.clone()));
        }
    }

    let listed = list_object_versions(&router, name, "").await;
    assert_eq!(
        listed.status,
        200,
        "case \"every version of three keys\": expected HTTP 200, got {} with body {:?}",
        listed.status,
        text(&listed)
    );
    let listing = version_listing_of(&listed);
    assert_eq!(
        listing.entries(),
        expected,
        "case \"every version of three keys\": expected keys ascending and each key's \
         versions newest first, got {:?}",
        listing.entries()
    );
    assert_eq!(
        listing.field("Name"),
        name,
        "case \"every version of three keys\": the listing names its bucket, got {:?}",
        listing.field("Name")
    );
    assert_eq!(
        listing.field("IsTruncated"),
        "false",
        "case \"every version of three keys\": a complete page is not truncated, got {:?}",
        listing.field("IsTruncated")
    );

    // Only the newest version of each key is the latest one, and each entry reports that
    // version's own size and entity tag.
    let sizes: BTreeMap<(String, String), usize> = rows
        .iter()
        .flat_map(|(key, revisions)| {
            let ids = by_key[key].clone();
            revisions
                .iter()
                .rev()
                .zip(ids)
                .map(move |(body, id)| (((*key).to_string(), id), body.len()))
        })
        .collect();
    for entry in &listing.versions {
        let key = VersionListing::entry(entry, "Key");
        let version_id = VersionListing::entry(entry, "VersionId");
        let case = format!("{key} version {version_id}");
        let is_latest = by_key[key.as_str()].first() == Some(&version_id);
        assert_eq!(
            VersionListing::entry(entry, "IsLatest"),
            is_latest.to_string(),
            "case {case:?}: expected IsLatest {is_latest}, got {:?}",
            VersionListing::entry(entry, "IsLatest")
        );
        let size = sizes[&(key.clone(), version_id.clone())];
        assert_eq!(
            VersionListing::entry(entry, "Size"),
            size.to_string(),
            "case {case:?}: expected Size {size}, got {:?}",
            VersionListing::entry(entry, "Size")
        );
    }

    // A delete that names no version lays a DELETE MARKER: it becomes the key's current
    // version, every version it displaced is still listed, and none of THOSE is latest any
    // more.
    let deleted = delete_object(&router, name, "source.zip", "").await;
    assert_eq!(
        deleted.status, 204,
        "case \"delete the current version of one key\": expected HTTP 204, got {}",
        deleted.status
    );
    assert_eq!(
        deleted.header("x-amz-delete-marker").as_deref(),
        Some("true"),
        "case \"delete the current version of one key\": the delete reports that it \
         created a delete marker, got {:?}",
        deleted.header("x-amz-delete-marker")
    );
    let marker_id = deleted.header(VERSION_ID_HEADER).unwrap_or_default();
    assert!(
        !marker_id.is_empty(),
        "case \"delete the current version of one key\": the delete reports the version id \
         of the marker it created, got {marker_id:?}"
    );
    let listing = version_listing_of(&list_object_versions(&router, name, "").await);
    let mut listed = listing.entries();
    let marker_entry = ("source.zip".to_string(), marker_id.clone());
    let at = listed
        .iter()
        .position(|entry| *entry == marker_entry)
        .unwrap_or_else(|| {
            panic!(
                "case \"delete the current version of one key\": the delete marker must be \
                 listed, got {listed:?}"
            )
        });
    listed.remove(at);
    assert_eq!(
        listed,
        expected,
        "case \"delete the current version of one key\": every version is still listed \
         beside the marker, got {:?}",
        listing.entries()
    );
    for entry in &listing.versions {
        if VersionListing::entry(entry, "Key") != "source.zip" {
            continue;
        }
        // The marker is the key's current version, so it is the latest one and nothing
        // behind it is.
        let is_latest = VersionListing::entry(entry, "VersionId") == marker_id;
        assert_eq!(
            VersionListing::entry(entry, "IsLatest"),
            is_latest.to_string(),
            "case \"delete the current version of one key\": only the delete marker is \
             latest, got IsLatest {:?} for version {:?}",
            VersionListing::entry(entry, "IsLatest"),
            VersionListing::entry(entry, "VersionId")
        );
    }
}

#[tokio::test]
async fn a_version_listing_pages_through_max_keys_without_losing_or_repeating_an_entry() {
    let router = router();
    let name = "versioned-source";
    bucket(&router, name).await;
    set_versioning(&router, name, "Enabled").await;

    // Seven versions over three keys, so a page boundary falls inside a key as well as
    // between keys.
    let rows: [(&str, Vec<&[u8]>); 3] = [
        ("one.txt", vec![b"a", b"b", b"c"]),
        ("three.txt", vec![b"g"]),
        ("two.txt", vec![b"d", b"e", b"f"]),
    ];
    let mut expected: Vec<(String, String)> = Vec::new();
    for (key, revisions) in &rows {
        for id in write_revisions(&router, name, key, revisions).await {
            expected.push(((*key).to_string(), id));
        }
    }
    assert_eq!(expected.len(), 7, "the table writes seven versions");

    // Page sizes spanning the class: one entry at a time, a size that does not divide the
    // total, one that does, exactly the total, and more than the total.
    for max_keys in [1usize, 2, 3, 7, 11] {
        let mut collected: Vec<(String, String)> = Vec::new();
        let mut query = format!("&max-keys={max_keys}");
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case max-keys={max_keys}: paging must terminate, got {pages} pages"
            );
            let answer = list_object_versions(&router, name, &query).await;
            assert_eq!(
                answer.status,
                200,
                "case max-keys={max_keys}: expected HTTP 200, got {} with body {:?}",
                answer.status,
                text(&answer)
            );
            let listing = version_listing_of(&answer);
            assert!(
                listing.versions.len() <= max_keys,
                "case max-keys={max_keys}: a page must never carry more than it asked \
                 for, got {}",
                listing.versions.len()
            );
            assert_eq!(
                listing.field("MaxKeys"),
                max_keys.min(1000).to_string(),
                "case max-keys={max_keys}: the listing echoes the page size it applied, \
                 got {:?}",
                listing.field("MaxKeys")
            );
            collected.extend(listing.entries());
            if listing.field("IsTruncated") != "true" {
                break;
            }
            let next_key = listing.field("NextKeyMarker");
            let next_version = listing.field("NextVersionIdMarker");
            assert!(
                !next_key.is_empty(),
                "case max-keys={max_keys}: a truncated page must name its next key marker"
            );
            query = format!(
                "&max-keys={max_keys}&key-marker={next_key}&version-id-marker={next_version}"
            );
        }
        assert_eq!(
            collected, expected,
            "case max-keys={max_keys}: paging must report every version exactly once, in \
             order, got {collected:?}"
        );
    }

    // `max-keys=0` asks for no entries at all: an empty page, and NOT a truncated one —
    // a token that makes no progress would page forever.
    let empty = version_listing_of(&list_object_versions(&router, name, "&max-keys=0").await);
    assert!(
        empty.versions.is_empty(),
        "case max-keys=0: expected no entries, got {:?}",
        empty.entries()
    );
    assert_eq!(
        empty.field("IsTruncated"),
        "false",
        "case max-keys=0: an empty page is not truncated, got {:?}",
        empty.field("IsTruncated")
    );
}

#[tokio::test]
async fn a_version_listing_selects_by_prefix_and_rolls_up_a_delimiter() {
    let router = router();
    let name = "versioned-source";
    bucket(&router, name).await;
    set_versioning(&router, name, "Enabled").await;

    let rows: [(&str, Vec<&[u8]>); 4] = [
        ("a/one.txt", vec![b"1", b"2"]),
        ("a/two.txt", vec![b"3"]),
        ("b/one.txt", vec![b"4", b"5"]),
        ("top.txt", vec![b"6"]),
    ];
    let mut by_key: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for (key, revisions) in &rows {
        by_key.insert(key, write_revisions(&router, name, key, revisions).await);
    }
    let versions_of = |keys: &[&str]| -> Vec<(String, String)> {
        keys.iter()
            .flat_map(|key| {
                by_key[*key]
                    .iter()
                    .map(move |id| ((*key).to_string(), id.clone()))
            })
            .collect()
    };

    // (case, query, the keys whose versions the page must carry, the common prefixes it
    // must roll up). The rows differ along both selection dimensions and their
    // combination.
    let cases: [(&str, &str, Vec<&str>, Vec<&str>); 5] = [
        ("no selection", "", vec!["a/one.txt", "a/two.txt", "b/one.txt", "top.txt"], vec![]),
        ("a prefix naming one group", "&prefix=a/", vec!["a/one.txt", "a/two.txt"], vec![]),
        ("a prefix naming one key", "&prefix=top", vec!["top.txt"], vec![]),
        ("a prefix naming nothing", "&prefix=zz", vec![], vec![]),
        ("a delimiter rolling up both groups", "&delimiter=/", vec!["top.txt"], vec!["a/", "b/"]),
    ];
    for (case, query, keys, prefixes) in cases {
        let answer = list_object_versions(&router, name, query).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: expected HTTP 200, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        let listing = version_listing_of(&answer);
        assert_eq!(
            listing.entries(),
            versions_of(&keys),
            "case {case:?}: expected the versions of {keys:?}, got {:?}",
            listing.entries()
        );
        assert_eq!(
            listing.common_prefixes,
            prefixes
                .iter()
                .map(|prefix| (*prefix).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected the common prefixes {prefixes:?}, got {:?}",
            listing.common_prefixes
        );
    }
}

#[tokio::test]
async fn a_version_listing_answers_for_an_unversioned_bucket_and_refuses_what_it_cannot_serve() {
    let router = router();
    let name = "plain-bucket";
    bucket(&router, name).await;

    // A bucket that holds nothing lists nothing, and is not truncated.
    let empty = version_listing_of(&list_object_versions(&router, name, "").await);
    assert!(
        empty.versions.is_empty(),
        "case \"an empty bucket\": expected no entries, got {:?}",
        empty.entries()
    );

    // A bucket whose versioning was never set holds `null` versions, which is the id a
    // listing reports for them and the one a read names them by.
    for key in ["object.txt", "nested/object.txt"] {
        let written = put_object(&router, name, key, b"stored", &[]).await;
        assert_eq!(
            written.status, 200,
            "the write of {key:?} must succeed, got {}",
            written.status
        );
    }
    let listing = version_listing_of(&list_object_versions(&router, name, "").await);
    assert_eq!(
        listing.entries(),
        vec![
            ("nested/object.txt".to_string(), "null".to_string()),
            ("object.txt".to_string(), "null".to_string()),
        ],
        "case \"a bucket whose versioning was never set\": expected the null version of \
         each key, got {:?}",
        listing.entries()
    );
    for entry in &listing.versions {
        assert_eq!(
            VersionListing::entry(entry, "ETag"),
            expected_etag(b"stored"),
            "case \"a bucket whose versioning was never set\": expected the payload's \
             entity tag, got {:?}",
            VersionListing::entry(entry, "ETag")
        );
        assert_eq!(
            VersionListing::entry(entry, "IsLatest"),
            "true",
            "case \"a bucket whose versioning was never set\": the only version of a key \
             is the latest one, got {:?}",
            VersionListing::entry(entry, "IsLatest")
        );
    }

    // A listing of a bucket this emulator does not hold is NoSuchBucket (404).
    let absent = list_object_versions(&router, "no-such-bucket-at-all", "").await;
    assert_eq!(
        absent.status,
        404,
        "case \"an absent bucket\": expected HTTP 404, got {} with body {:?}",
        absent.status,
        text(&absent)
    );
    assert_eq!(
        absent.error_code(),
        "NoSuchBucket",
        "case \"an absent bucket\": expected NoSuchBucket, got {:?}",
        absent.error_code()
    );

    // Members this listing cannot carry out keep the scaffold's 501, and a negative
    // page size — or an encoding type outside the one value the model declares — is the
    // InvalidArgument S3 answers rather than an empty or differently written page.
    // `encoding-type=url` is NOT among these: it is served, and `listing_encoding_type`
    // pins what it does to the key names.
    for (case, query, headers, expected_status) in [
        (
            "an encoding type the model does not declare",
            "&encoding-type=base64",
            &[][..],
            400,
        ),
        (
            "a requester-pays listing",
            "",
            &[("x-amz-request-payer", "requester")][..],
            501,
        ),
        (
            "optional object attributes",
            "",
            &[("x-amz-optional-object-attributes", "RestoreStatus")][..],
            501,
        ),
        ("a negative page size", "&max-keys=-1", &[][..], 400),
    ] {
        let answer = send(
            &router,
            "GET",
            &format!("/{name}?versions{query}"),
            headers,
            "",
        )
        .await;
        assert_eq!(
            answer.status,
            expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
    }
}
