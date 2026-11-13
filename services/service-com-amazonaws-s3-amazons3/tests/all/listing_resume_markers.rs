// SPDX-License-Identifier: CC0-1.0

//! The position a truncated listing hands back, in the rendering the request asked for.
//!
//! RULE: a listing that stopped short reports where the next page resumes, and that position
//! is written out of the same elements the page is — so a marker that is a KEY NAME carries
//! the encoding the request asked for, while a marker that is not a key name is reported as
//! the service minted it, and handing either back resumes the walk losslessly whichever
//! rendering was asked for. `ListMultipartUploads` "returns encoded key name values in the
//! following response elements: `Delimiter`, `KeyMarker`, `Prefix`, `NextKeyMarker`, `Key`"
//! (`ListMultipartUploadsOutput$EncodingType`) — `UploadIdMarker` and `NextUploadIdMarker`
//! are not among them, an upload id being no key name — and `ListObjects` encodes "the object
//! keys in the response" (`ListObjectsOutput$EncodingType`), which for that revision are
//! `Prefix`, `Delimiter`, `Marker`, `NextMarker` and each entry's `Key`. Resuming is what the
//! markers are for: "you can use the key name in this field as the `marker` parameter in the
//! subsequent request to get the next set of objects" (`ListObjectsOutput$NextMarker`), and
//! "this element specifies the value that should be used for the `upload-id-marker` request
//! parameter in a subsequent request" (`ListMultipartUploadsOutput$NextUploadIdMarker`).
//!
//! The rows differ along what the encoder has to do to a key name: nothing at all, the
//! parentheses of the model's own example, a space, punctuation a URI reserves, and
//! multi-byte UTF-8. Each row is walked twice end to end, one entry per page, once in each
//! rendering, and the two walks are compared with each other and with what was stored — so an
//! encoding applied to the wrong element, or a marker that does not resume, fails here
//! whatever the encoder writes. [`super::listing_encoding_type`] is where the written form of
//! each byte is pinned value by value; this suite pins what the markers are good for.

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::router;
use super::multipart_upload_listing::{initiate, list_uploads, upload_listing_of};
use super::object_and_tagging_roundtrip::{bucket, text};
use super::object_listing_encoding::{decoded, in_path};
use super::object_listing_v1::list_objects;
use super::object_storage::{listing_of, store_keys};

/// `(case, keys)`: the key sets every walk below is run over, each row differing in what
/// `encoding-type=url` has to do to a key name — and so to the marker that names one.
const ROWS: &[(&str, &[&str])] = &[
    (
        "key names the encoder leaves alone",
        &["alpha.bin", "beta~1", "gamma_2.csv"],
    ),
    (
        "parentheses, the documented example",
        &["test_file(1).png", "test_file(2).png", "test_file(3).png"],
    ),
    (
        "a space between words",
        &["two words.bin", "two words.csv", "zz last one.bin"],
    ),
    (
        "punctuation a URI reserves",
        &["at@sign", "comma,separated", "semi;colon"],
    ),
    ("multi-byte UTF-8", &["donnée.txt", "über.csv", "記録.bin"]),
];

/// The `delimiter` the object walk carries, so that revision owes a `NextMarker` at all —
/// "this element is returned only if you have the delimiter request parameter specified"
/// (`ListObjectsOutput$NextMarker`). No key in [`ROWS`] contains it, so nothing rolls up and
/// each page reports a key; it is neither of the two characters a key name's encoding leaves
/// alone, so the echoed element also shows the encoding reached it.
const DELIMITER: &str = "|";

/// [`DELIMITER`] as a key-name element of an `encoding-type=url` listing.
const DELIMITER_ENCODED: &str = "%7C";

/// Whether `encoding-type=url` changes how `key` is written: the encoding escapes every
/// character outside RFC 3986's unreserved set, which is what [`in_path`] escapes too.
///
/// `pub(crate)`, like [`Rendering`]: the version listing's own walk
/// ([`super::version_listing_resume_markers`]) judges its key names by the same predicate
/// rather than keeping a second copy that could drift from this one.
pub(crate) fn touched_by_the_encoder(key: &str) -> bool {
    in_path(key) != key
}

/// The two renderings of ONE listing: the request asks for an encoding type, or it does not.
///
/// `pub(crate)`: every suite that walks a listing in both renderings asks for them through
/// this one type.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Rendering {
    Verbatim,
    Encoded,
}

impl Rendering {
    /// What a request carries to be answered in this rendering.
    pub(crate) fn query(self) -> &'static str {
        match self {
            Self::Verbatim => "",
            Self::Encoded => "&encoding-type=url",
        }
    }

    /// How a failing case names this rendering.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Verbatim => "no encoding type",
            Self::Encoded => "encoding-type=url",
        }
    }

    /// A key-name element of this rendering as the key a client stored: the decode step a
    /// client applies to every key-name element of an encoded page, and nothing at all where
    /// the request asked for no encoding.
    pub(crate) fn as_stored(self, written: &str) -> String {
        match self {
            Self::Verbatim => written.to_string(),
            Self::Encoded => decoded(written),
        }
    }
}

/// What one walk of `ListMultipartUploads` reported: every entry as `(key, upload id)` with
/// the key as STORED, and the `(NextKeyMarker, NextUploadIdMarker)` each TRUNCATED page
/// handed back, as written on the wire — the walk resumes from those; the position the page
/// that ends the walk reports is asserted where it is read.
#[derive(Debug, Default)]
struct UploadWalk {
    entries: Vec<(String, String)>,
    markers: Vec<(String, String)>,
}

/// The same for `ListObjects`, whose resume position is one element: every key as STORED, and
/// every `NextMarker` as written on the wire.
#[derive(Debug, Default)]
struct ObjectWalk {
    keys: Vec<String>,
    markers: Vec<String>,
}

/// Walk every multipart upload of `bucket_name` one entry per page, resuming from the markers
/// each truncated page hands back, in `rendering`.
///
/// `uploads` is how many uploads the bucket holds, which bounds the walk: a listing that never
/// stops being truncated fails the case instead of looping forever.
async fn walk_uploads(
    router: &MultiServiceRouter,
    bucket_name: &str,
    case: &str,
    rendering: Rendering,
    uploads: usize,
) -> UploadWalk {
    let mut walk = UploadWalk::default();
    let mut resume = String::new();
    let mut handed = (String::new(), String::new());
    let mut pages_left = uploads + 1;
    loop {
        assert!(
            pages_left > 0,
            "case {case:?} ({}): a walk of {uploads} uploads must end within {} pages, got \
             {walk:?}",
            rendering.label(),
            uploads + 1
        );
        pages_left -= 1;

        let query = format!("&max-uploads=1{}{resume}", rendering.query());
        let answer = list_uploads(router, bucket_name, &query, &[]).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?} ({}): a listing of a bucket that exists is 200, got {} with body \
             {:?}",
            rendering.label(),
            answer.status,
            text(&answer)
        );
        let listing = upload_listing_of(&answer);

        // The markers the request carried come back as they were handed: `KeyMarker` is a key
        // name, so it is echoed in this rendering, and `UploadIdMarker` is not, so it is
        // echoed as minted.
        assert_eq!(
            rendering.as_stored(&listing.field("KeyMarker")),
            handed.0,
            "case {case:?} ({}): the echoed KeyMarker must be the key the request resumed \
             after, got {:?}",
            rendering.label(),
            listing.field("KeyMarker")
        );
        assert_eq!(
            listing.field("UploadIdMarker"),
            handed.1,
            "case {case:?} ({}): an upload id is no key name, so the echoed UploadIdMarker is \
             the id the request carried, got {:?}",
            rendering.label(),
            listing.field("UploadIdMarker")
        );

        let reported = listing.entries();
        assert!(
            reported.len() <= 1,
            "case {case:?} ({}): a page of max-uploads=1 carries at most one upload, got \
             {reported:?}",
            rendering.label()
        );
        for (key, upload_id) in &reported {
            walk.entries
                .push((rendering.as_stored(key), upload_id.clone()));
        }

        let truncated = listing.field("IsTruncated");
        let next_key = listing.field("NextKeyMarker");
        let next_id = listing.field("NextUploadIdMarker");
        // The position a next page resumes after is reported on EVERY page — the last entry
        // this one reported, in the same rendering as its `Key` for the key name and as
        // minted for the upload id — whatever `IsTruncated` says; it is empty only for a
        // page that reported no entry at all.
        let (last_key, last_id) = reported.last().cloned().unwrap_or_default();
        assert_eq!(
            (next_key.as_str(), next_id.as_str()),
            (last_key.as_str(), last_id.as_str()),
            "case {case:?} ({}): the next markers must name the last entry this page \
             reported, got {:?}",
            rendering.label(),
            (&next_key, &next_id)
        );
        if truncated != "true" {
            assert_eq!(
                truncated,
                "false",
                "case {case:?} ({}): IsTruncated is a boolean on every page, got \
                 {truncated:?}",
                rendering.label()
            );
            break;
        }
        assert!(
            !reported.is_empty(),
            "case {case:?} ({}): a truncated page must report the entry it resumes after",
            rendering.label()
        );
        walk.markers.push((next_key.clone(), next_id.clone()));

        // What a client sends next: the position it was handed, as a query value carries it.
        handed = (rendering.as_stored(&next_key), next_id);
        resume = format!(
            "&key-marker={}&upload-id-marker={}",
            in_path(&handed.0),
            in_path(&handed.1)
        );
    }
    walk
}

/// Walk every object of `bucket_name` one key per page through `ListObjects`, resuming from
/// each truncated page's `NextMarker`, in `rendering`; `objects` bounds the walk.
async fn walk_objects(
    router: &MultiServiceRouter,
    bucket_name: &str,
    case: &str,
    rendering: Rendering,
    objects: usize,
) -> ObjectWalk {
    let mut walk = ObjectWalk::default();
    let mut resume = String::new();
    let mut handed = String::new();
    let mut pages_left = objects + 1;
    loop {
        assert!(
            pages_left > 0,
            "case {case:?} ({}): a walk of {objects} objects must end within {} pages, got \
             {walk:?}",
            rendering.label(),
            objects + 1
        );
        pages_left -= 1;

        let query = format!(
            "delimiter={}&max-keys=1{}{resume}",
            in_path(DELIMITER),
            rendering.query()
        );
        let answer = list_objects(router, bucket_name, &query).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?} ({}): a listing of a bucket that exists is 200, got {} with body \
             {:?}",
            rendering.label(),
            answer.status,
            text(&answer)
        );
        let listing = listing_of(&answer);

        let expected_delimiter = match rendering {
            Rendering::Verbatim => DELIMITER,
            Rendering::Encoded => DELIMITER_ENCODED,
        };
        assert_eq!(
            listing.field("Delimiter"),
            expected_delimiter,
            "case {case:?} ({}): the echoed Delimiter is a key-name element, got {:?}",
            rendering.label(),
            listing.field("Delimiter")
        );
        assert_eq!(
            rendering.as_stored(&listing.field("Marker")),
            handed,
            "case {case:?} ({}): the echoed Marker must be the key the request resumed after, \
             got {:?}",
            rendering.label(),
            listing.field("Marker")
        );
        assert!(
            listing.common_prefixes.is_empty(),
            "case {case:?} ({}): no key carries the delimiter, so nothing rolls up, got {:?}",
            rendering.label(),
            listing.common_prefixes
        );

        let reported = listing.keys();
        assert!(
            reported.len() <= 1,
            "case {case:?} ({}): a page of max-keys=1 carries at most one key, got \
             {reported:?}",
            rendering.label()
        );
        for key in &reported {
            walk.keys.push(rendering.as_stored(key));
        }

        let truncated = listing.field("IsTruncated");
        let next = listing.field("NextMarker");
        if truncated != "true" {
            assert_eq!(
                truncated,
                "false",
                "case {case:?} ({}): IsTruncated is a boolean on every page, got \
                 {truncated:?}",
                rendering.label()
            );
            assert_eq!(
                next,
                "",
                "case {case:?} ({}): the page that ends the walk hands back no NextMarker, \
                 got {next:?}",
                rendering.label()
            );
            break;
        }
        let last = reported.last().cloned().unwrap_or_else(|| {
            panic!(
                "case {case:?} ({}): a truncated page must report the key it resumes after",
                rendering.label()
            )
        });
        assert_eq!(
            next, last,
            "case {case:?} ({}): NextMarker names the last key the page reported, in the same \
             rendering as that Key, got {next:?}",
            rendering.label()
        );
        walk.markers.push(next.clone());

        handed = rendering.as_stored(&next);
        resume = format!("&marker={}", in_path(&handed));
    }
    walk
}

#[tokio::test]
async fn an_upload_listing_resumes_from_its_markers_in_either_rendering() {
    for (case, keys) in ROWS {
        let router = router();
        let bucket_name = "multipart-resume-markers";
        bucket(&router, bucket_name).await;
        let mut initiated: Vec<(String, String)> = Vec::new();
        for key in *keys {
            let upload_id = initiate(&router, bucket_name, key, &[]).await;
            initiated.push(((*key).to_string(), upload_id));
        }
        // Key order, which is the order a listing reports — no key here carries two
        // uploads, so the within-one-key order plays no part in these rows.
        initiated.sort();

        let verbatim =
            walk_uploads(&router, bucket_name, case, Rendering::Verbatim, keys.len()).await;
        let encoded = walk_uploads(&router, bucket_name, case, Rendering::Encoded, keys.len()).await;

        assert_eq!(
            verbatim.entries, initiated,
            "case {case:?}: a walk at one upload per page must report every initiated upload \
             exactly once, got {:?}",
            verbatim.entries
        );
        assert_eq!(
            encoded.entries, initiated,
            "case {case:?}: the encoded walk must report the same uploads once decoded, got \
             {:?}",
            encoded.entries
        );
        assert_eq!(
            verbatim.markers.len(),
            keys.len() - 1,
            "case {case:?}: every page but the last hands back a resume position, got {:?}",
            verbatim.markers
        );
        assert_eq!(
            encoded.markers.len(),
            verbatim.markers.len(),
            "case {case:?}: the encoding changes how a page is written, not how many there \
             are, got {:?}",
            encoded.markers
        );
        for (at, ((encoded_key, encoded_id), (verbatim_key, verbatim_id))) in
            encoded.markers.iter().zip(&verbatim.markers).enumerate()
        {
            assert_eq!(
                &decoded(encoded_key),
                verbatim_key,
                "case {case:?}, marker {at}: decoding NextKeyMarker must yield the key the \
                 verbatim walk reported, got {encoded_key:?}"
            );
            assert_eq!(
                encoded_id, verbatim_id,
                "case {case:?}, marker {at}: an upload id is no key name, so NextUploadIdMarker \
                 is the same in both renderings, got {encoded_id:?} against {verbatim_id:?}"
            );
            assert_eq!(
                encoded_key != verbatim_key,
                touched_by_the_encoder(verbatim_key),
                "case {case:?}, marker {at}: NextKeyMarker must be written encoded exactly \
                 where the encoding has something to escape in {verbatim_key:?}, got \
                 {encoded_key:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_object_listing_resumes_from_its_next_marker_in_either_rendering() {
    for (case, keys) in ROWS {
        let router = router();
        let bucket_name = "v1-resume-markers";
        bucket(&router, bucket_name).await;
        // A key with a space or a multi-byte character cannot go into a request line as it
        // is, so each is written through the escaping a request path carries; what reaches the
        // store is the key itself.
        let paths: Vec<String> = keys.iter().map(|key| in_path(key)).collect();
        let path_refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        store_keys(&router, bucket_name, &path_refs).await;
        let mut stored: Vec<String> = keys.iter().map(|key| (*key).to_string()).collect();
        stored.sort();

        let verbatim =
            walk_objects(&router, bucket_name, case, Rendering::Verbatim, keys.len()).await;
        let encoded = walk_objects(&router, bucket_name, case, Rendering::Encoded, keys.len()).await;

        assert_eq!(
            verbatim.keys, stored,
            "case {case:?}: a walk at one key per page must report every stored key exactly \
             once, in byte order, got {:?}",
            verbatim.keys
        );
        assert_eq!(
            encoded.keys, stored,
            "case {case:?}: the encoded walk must report the same keys once decoded, got {:?}",
            encoded.keys
        );
        assert_eq!(
            verbatim.markers.len(),
            keys.len() - 1,
            "case {case:?}: every page but the last hands back a NextMarker, got {:?}",
            verbatim.markers
        );
        assert_eq!(
            encoded.markers.len(),
            verbatim.markers.len(),
            "case {case:?}: the encoding changes how a page is written, not how many there \
             are, got {:?}",
            encoded.markers
        );
        for (at, (encoded_marker, verbatim_marker)) in
            encoded.markers.iter().zip(&verbatim.markers).enumerate()
        {
            assert_eq!(
                &decoded(encoded_marker),
                verbatim_marker,
                "case {case:?}, marker {at}: decoding NextMarker must yield the key the \
                 verbatim walk reported, got {encoded_marker:?}"
            );
            assert_eq!(
                encoded_marker != verbatim_marker,
                touched_by_the_encoder(verbatim_marker),
                "case {case:?}, marker {at}: NextMarker must be written encoded exactly where \
                 the encoding has something to escape in {verbatim_marker:?}, got \
                 {encoded_marker:?}"
            );
        }
    }
}
