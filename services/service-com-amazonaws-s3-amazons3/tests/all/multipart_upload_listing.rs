// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the BUCKET-WIDE read of the multipart upload loop: what
//! `CreateMultipartUpload` records is what `ListMultipartUploads` reports.
//!
//! Every assertion drives a real HTTP request through the router (the harness is the one
//! [`super::bucket_lifecycle`] already owns), so what is pinned is the wire behaviour —
//! status, payload elements, error code — rather than any handler's internals.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! 1. A listing reports every multipart upload the named bucket has IN PROGRESS and
//!    nothing else — one entry per upload, ordered by key and then by initiation time (the
//!    order [`super::multipart_upload_initiation_order`] spans case by case), each
//!    carrying the id the initiation minted, the key it was initiated for, the instant it
//!    was initiated, the storage class and checksum configuration the initiation settled,
//!    and the account's owner and initiator. "An in-progress multipart upload is a
//!    multipart upload that has been initiated by the CreateMultipartUpload request, but
//!    has not yet been completed or aborted" (`ListMultipartUploads`'s own documentation).
//! 2. `prefix` selects — "Lists in-progress uploads only for those keys that begin with the
//!    specified prefix" — and `delimiter` rolls the rest up: "the result returns each
//!    distinct key prefix containing the delimiter in a CommonPrefixes element", and "the
//!    keys that are grouped under CommonPrefixes result element are not returned elsewhere
//!    in the response".
//! 3. A page is at most `max-uploads` entries, bounded by the published 1,000, and paging
//!    with `key-marker`/`upload-id-marker` reports every upload exactly once whatever the
//!    page size: "Set the value of key-marker to the NextKeyMarker value from the previous
//!    response. Similarly, set the value of upload-id-marker to the NextUploadIdMarker
//!    value from the previous response."
//! 4. A read is a read: two listings report the same thing, and neither changes what a
//!    later reader of one upload sees.
//! 5. `encoding-type=url` percent-encodes the key-name elements of the response and nothing
//!    else: "returns encoded key name values in the following response elements: Delimiter,
//!    KeyMarker, Prefix, NextKeyMarker, Key".
//! 6. A request this emulator cannot serve as asked is refused rather than answered with an
//!    empty page, each with the status, error code and MESSAGE S3 publishes for that error
//!    class: a bucket it does not hold with 404 `NoSuchBucket`, a `max-uploads` that is not
//!    a count with 400 `InvalidArgument` naming the value the request carried, an encoding
//!    method that is not the one the model declares with 400 `InvalidArgument`,
//!    `x-amz-expected-bucket-owner` naming another account with 403, and a Requester Pays
//!    confirmation with 501.
//!
//! The upload id is a value the SERVICE mints per initiation — `MultipartUploadId` is an
//! unconstrained string in the model — so nothing here asserts a literal id; what is
//! asserted is that the listing reports the very id the initiation answered.

use std::collections::{BTreeMap, BTreeSet};

use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use quick_xml::events::Event;
use quick_xml::Reader;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, Answer};
use super::bucket_tagging::unescaped;
use super::object_and_tagging_roundtrip::{bucket, text};
use super::object_listing_encoding::decoded;

/// The most uploads one page may carry: "The ListMultipartUploads operation returns a
/// maximum of 1,000 multipart uploads in the response. The limit of 1,000 multipart
/// uploads is also the default value" (`ListMultipartUploads`'s own documentation).
const MAX_UPLOADS_PER_PAGE: i32 = 1000;

/// The `StorageClass` an initiation that names none settles on: "By default, Amazon S3
/// uses the STANDARD Storage Class to store newly created objects"
/// (`CreateMultipartUploadRequest$StorageClass`).
const DEFAULT_STORAGE_CLASS: &str = "STANDARD";

/// The bytes this suite leaves literal when it builds a REQUEST path: RFC 3986's
/// unreserved set plus `/`, the segment separator. A key goes into the request URI through
/// this (a raw space or `%` is not a legal URI character) and the framework percent-decodes
/// a path label, so what reaches the store is the key itself.
const PATH_LITERAL: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~')
    .remove(b'/');

/// `key` as a request path carries it.
fn in_path(key: &str) -> String {
    utf8_percent_encode(key, PATH_LITERAL).to_string()
}

/// `POST /<bucket>/<key>?uploads` — `CreateMultipartUpload`, answering the upload id the
/// store minted.
///
/// `pub(crate)`: the resume-marker suite ([`super::listing_resume_markers`]) initiates its
/// uploads through this one helper rather than keeping a second copy.
pub(crate) async fn initiate(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    headers: &[(&str, &str)],
) -> String {
    let initiated = send(
        router,
        "POST",
        &format!("/{name}/{}?uploads", in_path(key)),
        headers,
        "",
    )
    .await;
    assert_eq!(
        initiated.status,
        200,
        "the upload of {key:?} must be initiated first, got {} with body {:?}",
        initiated.status,
        text(&initiated)
    );
    initiated
        .element("UploadId")
        .unwrap_or_else(|| panic!("an initiation of {key:?} must report an UploadId"))
}

/// `GET /<bucket>?uploads<query>` — `ListMultipartUploads`.
pub(crate) async fn list_uploads(
    router: &MultiServiceRouter,
    name: &str,
    query: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(
        router,
        "GET",
        &format!("/{name}?uploads{query}"),
        headers,
        "",
    )
    .await
}

/// `GET /<bucket>/<key>?uploadId=<id>&x-id=ListParts` — `ListParts`, the read of ONE
/// upload, used here only to show a listing changed nothing about it.
async fn list_parts(router: &MultiServiceRouter, name: &str, key: &str, upload_id: &str) -> Answer {
    send(
        router,
        "GET",
        &format!(
            "/{name}/{}?uploadId={upload_id}&x-id=ListParts",
            in_path(key)
        ),
        &[],
        "",
    )
    .await
}

/// One `ListMultipartUploads` response, as a client's XML reader sees it: the top-level
/// elements, one map per `Upload` entry, and the `CommonPrefixes` prefixes, each in
/// document order.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct UploadListing {
    /// The elements directly under the result: `Bucket`, `IsTruncated`, `MaxUploads`, the
    /// markers and the echoed request parameters.
    top: BTreeMap<String, String>,
    /// One map per `Upload` entry, keyed by element name — a member of the nested `Owner`
    /// and `Initiator` containers under `Owner.ID` and `Initiator.ID` — in response order.
    uploads: Vec<BTreeMap<String, String>>,
    /// Every `CommonPrefixes/Prefix`, in response order.
    common_prefixes: Vec<String>,
}

impl UploadListing {
    /// A top-level element's text, or the empty string when the response carries none —
    /// which is what "absent" looks like to a client reading a bound member.
    pub(crate) fn field(&self, name: &str) -> String {
        self.top.get(name).cloned().unwrap_or_default()
    }

    /// One entry's element, or the empty string when it carries none.
    fn entry(index: &BTreeMap<String, String>, name: &str) -> String {
        index.get(name).cloned().unwrap_or_default()
    }

    /// Every reported upload as `(key, upload id)`, in response order.
    pub(crate) fn entries(&self) -> Vec<(String, String)> {
        self.uploads
            .iter()
            .map(|entry| (Self::entry(entry, "Key"), Self::entry(entry, "UploadId")))
            .collect()
    }

    /// Every reported upload as `(key, upload id, Initiated)`, in response order.
    ///
    /// `pub(crate)`: the initiation-order suite
    /// ([`super::multipart_upload_initiation_order`]) reads the instant each entry reports
    /// through this one reader rather than slicing the document a second way.
    pub(crate) fn dated_entries(&self) -> Vec<(String, String, String)> {
        self.uploads
            .iter()
            .map(|entry| {
                (
                    Self::entry(entry, "Key"),
                    Self::entry(entry, "UploadId"),
                    Self::entry(entry, "Initiated"),
                )
            })
            .collect()
    }
}

/// Read an upload listing with an XML reader rather than by slicing the body — the same
/// accumulate-per-element walk [`super::object_versioning::version_listing_of`] does for
/// `ListObjectVersions`, over this operation's `Upload` container.
pub(crate) fn upload_listing_of(answer: &Answer) -> UploadListing {
    let body = String::from_utf8_lossy(&answer.body).into_owned();
    let mut reader = Reader::from_str(&body);

    let mut listing = UploadListing::default();
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
                    "Upload" => entry = Some(BTreeMap::new()),
                    "Owner" | "Initiator" => nested = Some(element.clone()),
                    "CommonPrefixes" => common_prefix = Some(String::new()),
                    _ => {}
                }
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
                    "Upload" => {
                        if let Some(done) = entry.take() {
                            listing.uploads.push(done);
                        }
                    }
                    "Owner" | "Initiator" => nested = None,
                    "CommonPrefixes" => {
                        if let Some(done) = common_prefix.take() {
                            listing.common_prefixes.push(done);
                        }
                    }
                    // Every other element's text belongs to whichever container is open,
                    // under the container-qualified name when it is a member of the
                    // `Owner` or `Initiator` structure.
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

/// A listing that must have succeeded, read into [`UploadListing`].
///
/// `pub(crate)`: the initiation-order suite
/// ([`super::multipart_upload_initiation_order`]) reads its pages through this one helper
/// rather than keeping a second copy of the 200 assertion.
pub(crate) async fn listing(
    router: &MultiServiceRouter,
    name: &str,
    case: &str,
    query: &str,
) -> UploadListing {
    let answer = list_uploads(router, name, query, &[]).await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: expected HTTP 200 from ListMultipartUploads, got {} with body {:?}",
        answer.status,
        text(&answer)
    );
    upload_listing_of(&answer)
}

/// One row of the initiation table: the case name, the key the upload is initiated for,
/// and the headers that initiation carries.
struct UploadCase {
    case: &'static str,
    key: &'static str,
    headers: Vec<(&'static str, &'static str)>,
}

/// The initiation table. The rows differ along what the listing's rule generalises over:
/// the key (a flat name, several nesting depths, a name that is a PREFIX of another, one
/// key carrying TWO uploads, a non-ASCII name), the storage class and checksum
/// configuration the initiation settles, and the ORDER the uploads are initiated in, which
/// is deliberately not key order.
///
/// Each storage class is a value of the model's own `StorageClass` enum and each checksum
/// algorithm a value of its `ChecksumAlgorithm` enum
/// (`smithy_query(query="operation",operation="CreateMultipartUpload")`).
fn upload_cases() -> Vec<UploadCase> {
    vec![
        UploadCase {
            case: "a nested key under a second directory level",
            key: "nested/path/two.bin",
            headers: vec![("x-amz-storage-class", "STANDARD_IA")],
        },
        UploadCase {
            case: "a flat key, no header at all",
            key: "alpha.bin",
            headers: Vec::new(),
        },
        UploadCase {
            case: "a second upload of that same flat key",
            key: "alpha.bin",
            headers: vec![("x-amz-storage-class", "ONEZONE_IA")],
        },
        UploadCase {
            case: "a key that is a prefix of another key",
            key: "nested/path",
            headers: Vec::new(),
        },
        UploadCase {
            case: "a nested key with a checksum configuration",
            key: "nested/path/one.bin",
            headers: vec![
                ("x-amz-checksum-algorithm", "CRC32"),
                ("x-amz-checksum-type", "COMPOSITE"),
            ],
        },
        UploadCase {
            case: "a non-ASCII key",
            key: "über/größe.bin",
            headers: vec![("x-amz-storage-class", "REDUCED_REDUNDANCY")],
        },
        UploadCase {
            case: "a key sorting last",
            key: "zeta.bin",
            headers: Vec::new(),
        },
    ]
}

/// Initiate every row of [`upload_cases`] in the bucket `name` and hand back what each one
/// recorded, as `(key, upload id, storage class, checksum algorithm, checksum type)`.
async fn initiate_all(
    router: &MultiServiceRouter,
    name: &str,
) -> Vec<(String, String, String, String, String)> {
    let mut recorded = Vec::new();
    for row in upload_cases() {
        let upload_id = initiate(router, name, row.key, &row.headers).await;
        assert!(
            !upload_id.is_empty(),
            "case {:?}: the initiation of {:?} must mint a non-empty upload id, got {upload_id:?}",
            row.case,
            row.key
        );
        let header = |wanted: &str| {
            row.headers
                .iter()
                .find(|(header, _)| *header == wanted)
                .map(|(_, value)| (*value).to_string())
                .unwrap_or_default()
        };
        let storage_class = match header("x-amz-storage-class") {
            empty if empty.is_empty() => DEFAULT_STORAGE_CLASS.to_string(),
            named => named,
        };
        recorded.push((
            row.key.to_string(),
            upload_id,
            storage_class,
            header("x-amz-checksum-algorithm"),
            header("x-amz-checksum-type"),
        ));
    }
    recorded
}

#[tokio::test]
async fn every_upload_in_progress_reads_back_in_key_then_initiation_order() {
    let router = router();
    let name = "multipart-listing-bucket";
    bucket(&router, name).await;
    // A second bucket with an upload of its OWN, under a key that would sort FIRST: a
    // listing reports the uploads of the bucket it names and no other's.
    let other = "multipart-listing-other-bucket";
    bucket(&router, other).await;
    let elsewhere = initiate(&router, other, "aaa-elsewhere.bin", &[]).await;

    let recorded = initiate_all(&router, name).await;

    // Every id the initiations minted is distinct — that is what makes two uploads of one
    // key two entries of the listing rather than one.
    let minted: BTreeSet<&String> = recorded.iter().map(|(_, id, _, _, _)| id).collect();
    assert_eq!(
        minted.len(),
        recorded.len(),
        "every initiation must mint its own upload id, got {:?}",
        recorded
            .iter()
            .map(|(key, id, _, _, _)| (key, id))
            .collect::<Vec<_>>()
    );

    // The order the listing owes: by key, then by initiation time within one key — a
    // STABLE sort by key of the initiation sequence, which is the order `recorded` is in.
    let mut expected: Vec<(String, String)> = recorded
        .iter()
        .map(|(key, id, _, _, _)| (key.clone(), id.clone()))
        .collect();
    expected.sort_by(|left, right| left.0.cmp(&right.0));

    let page = listing(&router, name, "no selection at all", "").await;
    assert_eq!(
        page.entries(),
        expected,
        "case {:?}: expected the uploads in key then initiation order, got {:?}",
        "no selection at all",
        page.entries()
    );
    assert!(
        !page.entries().iter().any(|(_, id)| id == &elsewhere),
        "case {:?}: the other bucket's upload {elsewhere:?} must not be reported, got {:?}",
        "no selection at all",
        page.entries()
    );
    let (last_key, last_id) = expected
        .last()
        .cloned()
        .unwrap_or_else(|| panic!("the listing table must report at least one upload"));
    for (element, wanted) in [
        ("Bucket", name.to_string()),
        ("IsTruncated", "false".to_string()),
        ("MaxUploads", MAX_UPLOADS_PER_PAGE.to_string()),
        // The continuation markers name the last entry this page reported — the position a
        // next page would resume after — and are part of the answer even though nothing was
        // left for one; the request named no marker to echo.
        ("NextKeyMarker", last_key),
        ("NextUploadIdMarker", last_id),
        ("KeyMarker", String::new()),
        ("UploadIdMarker", String::new()),
        ("EncodingType", String::new()),
    ] {
        assert_eq!(
            page.field(element),
            wanted,
            "case {:?}: expected {element} {wanted:?}, got {:?}",
            "no selection at all",
            page.field(element)
        );
    }
    assert!(
        page.common_prefixes.is_empty(),
        "case {:?}: a listing with no delimiter rolls nothing up, got {:?}",
        "no selection at all",
        page.common_prefixes
    );

    // Each entry reports what its own initiation settled.
    for (key, upload_id, storage_class, checksum_algorithm, checksum_type) in &recorded {
        let case = format!("the upload {upload_id} of {key}");
        let entry = page
            .uploads
            .iter()
            .find(|entry| &UploadListing::entry(entry, "UploadId") == upload_id)
            .unwrap_or_else(|| panic!("case {case:?}: the listing must report this upload"));
        for (element, wanted) in [
            ("Key", key.clone()),
            ("StorageClass", storage_class.clone()),
            ("ChecksumAlgorithm", checksum_algorithm.clone()),
            ("ChecksumType", checksum_type.clone()),
        ] {
            assert_eq!(
                UploadListing::entry(entry, element),
                wanted,
                "case {case:?}: expected {element} {wanted:?}, got {:?}",
                UploadListing::entry(entry, element)
            );
        }
        // "Date and time at which the multipart upload was initiated"
        // (`MultipartUpload$Initiated`): an ISO 8601 instant, which is what the restXml
        // protocol writes a timestamp as.
        let initiated = UploadListing::entry(entry, "Initiated");
        assert!(
            initiated.contains('T') && initiated.ends_with('Z'),
            "case {case:?}: expected an ISO 8601 Initiated instant, got {initiated:?}"
        );
        // "If the principal is an Amazon Web Services account, it provides the Canonical
        // User ID" (`Initiator$ID`) — "a 64-digit hexadecimal encoded, 256-bit value" (AWS
        // General Reference, *AWS account identifiers*) — and this emulator serves one
        // account, so the owner and the initiator are the same principal.
        let owner = UploadListing::entry(entry, "Owner.ID");
        let initiator = UploadListing::entry(entry, "Initiator.ID");
        assert_eq!(
            owner, initiator,
            "case {case:?}: expected one account as owner and initiator, got {owner:?} and \
             {initiator:?}"
        );
        assert!(
            owner.len() == 64 && owner.chars().all(|digit| digit.is_ascii_hexdigit()),
            "case {case:?}: expected a 64-digit hexadecimal canonical user id, got {owner:?}"
        );
    }
}

#[tokio::test]
async fn a_listing_selects_by_prefix_and_rolls_up_a_delimiter() {
    let router = router();
    let name = "multipart-listing-selection";
    bucket(&router, name).await;
    let recorded = initiate_all(&router, name).await;

    // The uploads of `keys`, in the order the listing owes them: by key, then by initiation
    // time within one key, which a stable sort by key of the initiation sequence is.
    let uploads_of = |keys: &[&str]| -> Vec<(String, String)> {
        let mut wanted: Vec<(String, String)> = recorded
            .iter()
            .filter(|(key, _, _, _, _)| keys.contains(&key.as_str()))
            .map(|(key, id, _, _, _)| (key.clone(), id.clone()))
            .collect();
        wanted.sort_by(|left, right| left.0.cmp(&right.0));
        wanted
    };

    // (case, query, the keys whose uploads the page must carry, the prefixes it must roll
    // up). The rows differ along both selection dimensions and their combination, and along
    // what the delimiter is: the `/` of a key prefix, and a character that is not a
    // separator at all.
    let cases: [(&str, &str, Vec<&str>, Vec<&str>); 8] = [
        (
            "no selection",
            "",
            vec![
                "alpha.bin",
                "nested/path",
                "nested/path/one.bin",
                "nested/path/two.bin",
                "zeta.bin",
                "über/größe.bin",
            ],
            vec![],
        ),
        (
            "a prefix naming one directory level",
            "&prefix=nested/",
            vec!["nested/path", "nested/path/one.bin", "nested/path/two.bin"],
            vec![],
        ),
        (
            "a prefix naming one key exactly, which is a prefix of two others",
            "&prefix=nested/path",
            vec!["nested/path", "nested/path/one.bin", "nested/path/two.bin"],
            vec![],
        ),
        (
            "a prefix no key begins with",
            "&prefix=zzz",
            vec![],
            vec![],
        ),
        (
            "a prefix that is not ASCII",
            "&prefix=%C3%BCber/",
            vec!["über/größe.bin"],
            vec![],
        ),
        (
            "a delimiter rolling up every nested key",
            "&delimiter=/",
            vec!["alpha.bin", "zeta.bin"],
            vec!["nested/", "über/"],
        ),
        (
            "a prefix and a delimiter together",
            "&prefix=nested/&delimiter=/",
            vec!["nested/path"],
            vec!["nested/path/"],
        ),
        (
            "a delimiter that is no separator of these keys",
            "&delimiter=%23",
            vec![
                "alpha.bin",
                "nested/path",
                "nested/path/one.bin",
                "nested/path/two.bin",
                "zeta.bin",
                "über/größe.bin",
            ],
            vec![],
        ),
    ];

    for (case, query, keys, prefixes) in cases {
        let page = listing(&router, name, case, query).await;
        assert_eq!(
            page.entries(),
            uploads_of(&keys),
            "case {case:?}: expected the uploads of {keys:?}, got {:?}",
            page.entries()
        );
        assert_eq!(
            page.common_prefixes,
            prefixes
                .iter()
                .map(|prefix| (*prefix).to_string())
                .collect::<Vec<_>>(),
            "case {case:?}: expected the common prefixes {prefixes:?}, got {:?}",
            page.common_prefixes
        );
        // The echoed selection: "Contains the delimiter you specified in the request. If
        // you don't specify a delimiter in your request, this element is absent from the
        // response" (`ListMultipartUploadsOutput$Delimiter`), and the same for the prefix.
        let echoed = |parameter: &str| -> String {
            query
                .split('&')
                .filter_map(|field| field.split_once('='))
                .find(|(key, _)| *key == parameter)
                .map(|(_, value)| decoded(value))
                .unwrap_or_default()
        };
        for element in ["Prefix", "Delimiter"] {
            let wanted = echoed(&element.to_lowercase());
            assert_eq!(
                page.field(element),
                wanted,
                "case {case:?}: expected {element} {wanted:?}, got {:?}",
                page.field(element)
            );
        }
    }
}

#[tokio::test]
async fn paging_reports_every_upload_exactly_once() {
    let router = router();
    let name = "multipart-listing-paging";
    bucket(&router, name).await;
    let recorded = initiate_all(&router, name).await;
    let mut expected: Vec<(String, String)> = recorded
        .iter()
        .map(|(key, id, _, _, _)| (key.clone(), id.clone()))
        .collect();
    expected.sort_by(|left, right| left.0.cmp(&right.0));

    // The rows differ along the page size: one entry at a time, sizes that do not divide
    // the total, exactly the total, and far more than the total.
    for size in [1_usize, 2, 3, expected.len(), expected.len() + 1, 50] {
        let case = format!("pages of {size}");
        let mut seen: Vec<(String, String)> = Vec::new();
        let mut query = format!("&max-uploads={size}");
        // Bounded by the entries themselves: a page that makes no progress would loop, and
        // this cap is what turns that into a failed assertion instead.
        for _ in 0..=expected.len() {
            let page = listing(&router, name, &case, &query).await;
            assert!(
                page.entries().len() <= size,
                "case {case:?}: a page must carry at most {size} entries, got {:?}",
                page.entries()
            );
            assert_eq!(
                page.field("MaxUploads"),
                size.to_string(),
                "case {case:?}: expected the page bound {size} reported back, got {:?}",
                page.field("MaxUploads")
            );
            seen.extend(page.entries());
            let (key_marker, id_marker) = (
                page.field("NextKeyMarker"),
                page.field("NextUploadIdMarker"),
            );
            assert_eq!(
                (key_marker.as_str(), id_marker.as_str()),
                (
                    seen.last().map(|(key, _)| key.as_str()).unwrap_or_default(),
                    seen.last().map(|(_, id)| id.as_str()).unwrap_or_default()
                ),
                "case {case:?}: the next markers must name the last entry this page \
                 reported, got {key_marker:?} and {id_marker:?}"
            );
            if page.field("IsTruncated") != "true" {
                break;
            }
            query = format!(
                "&max-uploads={size}&key-marker={}&upload-id-marker={}",
                in_path(&key_marker),
                id_marker
            );
        }
        assert_eq!(
            seen, expected,
            "case {case:?}: paging must report every upload exactly once and in order, got {seen:?}"
        );
    }

    // "You can further limit the number of uploads in a response by specifying the
    // max-uploads request parameter", but never past the published ceiling: a larger value
    // is answered with that ceiling.
    let clamped = listing(&router, name, "max-uploads past the ceiling", "&max-uploads=5000").await;
    assert_eq!(
        clamped.field("MaxUploads"),
        MAX_UPLOADS_PER_PAGE.to_string(),
        "case {:?}: expected the published ceiling reported back, got {:?}",
        "max-uploads past the ceiling",
        clamped.field("MaxUploads")
    );
    assert_eq!(
        clamped.entries(),
        expected,
        "case {:?}: expected every upload on one page, got {:?}",
        "max-uploads past the ceiling",
        clamped.entries()
    );

    // `max-uploads=0` asks for no entries at all: an empty page, and NOT a truncated one —
    // markers that make no progress would page forever.
    let empty = listing(&router, name, "max-uploads=0", "&max-uploads=0").await;
    assert!(
        empty.entries().is_empty(),
        "case {:?}: expected no entries, got {:?}",
        "max-uploads=0",
        empty.entries()
    );
    assert_eq!(
        empty.field("IsTruncated"),
        "false",
        "case {:?}: an empty page is not truncated, got {:?}",
        "max-uploads=0",
        empty.field("IsTruncated")
    );

    // "If key-marker is not specified, the upload-id-marker parameter is ignored"
    // (`ListMultipartUploadsRequest$UploadIdMarker`).
    let ignored = listing(
        &router,
        name,
        "an upload-id-marker with no key-marker",
        &format!("&upload-id-marker={}", expected[0].1),
    )
    .await;
    assert_eq!(
        ignored.entries(),
        expected,
        "case {:?}: expected every upload, got {:?}",
        "an upload-id-marker with no key-marker",
        ignored.entries()
    );

    // A key-marker alone starts after the whole key: "only the keys lexicographically
    // greater than the specified key-marker will be included in the list".
    let after_key = &expected[0].0;
    let past = listing(
        &router,
        name,
        "a key-marker alone",
        &format!("&key-marker={}", in_path(after_key)),
    )
    .await;
    assert_eq!(
        past.entries(),
        expected
            .iter()
            .filter(|(key, _)| key > after_key)
            .cloned()
            .collect::<Vec<_>>(),
        "case {:?}: expected only the keys above {after_key:?}, got {:?}",
        "a key-marker alone",
        past.entries()
    );
}

#[tokio::test]
async fn a_listing_changes_nothing_a_later_reader_sees() {
    let router = router();
    let name = "multipart-listing-read-only";
    bucket(&router, name).await;
    let recorded = initiate_all(&router, name).await;

    let first = listing(&router, name, "the first read", "").await;
    let second = listing(&router, name, "the second read", "").await;
    assert_eq!(
        first, second,
        "case {:?}: two reads of one bucket must report the same uploads, got {:?} then {:?}",
        "two reads in a row",
        first.entries(),
        second.entries()
    );

    // The per-upload reader still answers for every upload the listing reported, with the
    // key the listing named: a listing takes no upload away and moves nothing.
    for (key, upload_id, _, _, _) in &recorded {
        let case = format!("the upload {upload_id} of {key} after two listings");
        let parts = list_parts(&router, name, key, upload_id).await;
        assert_eq!(
            parts.status,
            200,
            "case {case:?}: expected HTTP 200 from ListParts, got {} with body {:?}",
            parts.status,
            text(&parts)
        );
        assert_eq!(
            parts.element("Key").unwrap_or_default(),
            *key,
            "case {case:?}: expected the key {key:?} read back, got {:?}",
            parts.element("Key")
        );
    }

    let third = listing(&router, name, "the read after the per-upload reads", "").await;
    assert_eq!(
        first, third,
        "case {:?}: reading the uploads must not change the listing, got {:?} then {:?}",
        "a listing after the per-upload reads",
        first.entries(),
        third.entries()
    );
}

#[tokio::test]
async fn an_encoded_listing_decodes_back_to_the_keys_that_were_initiated() {
    let router = router();
    let name = "multipart-listing-encoding";
    bucket(&router, name).await;

    // Keys chosen along what the encoder decides: characters a URI reserves, the escape
    // character itself, a space, and two- and three-byte UTF-8.
    let keys = [
        ("the unreserved set only", "a~b-c_d.9"),
        ("punctuation a URI reserves", "semi;colon:at@sign,comma"),
        ("the escape character itself", "100%-done.csv"),
        ("a space between words", "two words.bin"),
        ("parentheses, the documented example", "test_file(3).png"),
        ("three-byte UTF-8", "記録/データ.bin"),
    ];
    let mut initiated: Vec<(String, String)> = Vec::new();
    for (case, key) in keys {
        let upload_id = initiate(&router, name, key, &[]).await;
        assert!(
            !upload_id.is_empty(),
            "case {case:?}: the initiation of {key:?} must mint an id, got {upload_id:?}"
        );
        initiated.push((key.to_string(), upload_id));
    }
    initiated.sort();

    let plain = listing(&router, name, "no encoding type", "").await;
    assert_eq!(
        plain.entries(),
        initiated,
        "case {:?}: expected the keys verbatim, got {:?}",
        "no encoding type",
        plain.entries()
    );

    let encoded = listing(&router, name, "encoding-type=url", "&encoding-type=url").await;
    assert_eq!(
        encoded.field("EncodingType"),
        "url",
        "case {:?}: the response must report the encoding it applied, got {:?}",
        "encoding-type=url",
        encoded.field("EncodingType")
    );
    let round_tripped: Vec<(String, String)> = encoded
        .entries()
        .into_iter()
        .map(|(key, id)| (decoded(&key), id))
        .collect();
    assert_eq!(
        round_tripped, initiated,
        "case {:?}: decoding the reported keys must yield the keys that were initiated, got {:?}",
        "encoding-type=url",
        encoded.entries()
    );
    // An upload ID is not a key name, so it is reported as minted either way.
    assert_eq!(
        encoded
            .entries()
            .into_iter()
            .map(|(_, id)| id)
            .collect::<Vec<_>>(),
        initiated
            .iter()
            .map(|(_, id)| id.clone())
            .collect::<Vec<_>>(),
        "case {:?}: an upload id must be reported verbatim, got {:?}",
        "encoding-type=url",
        encoded.entries()
    );
    // The key-name elements the request itself carried are encoded too.
    let selected = listing(
        &router,
        name,
        "an encoded prefix and delimiter",
        "&encoding-type=url&prefix=%E8%A8%98%E9%8C%B2/&delimiter=%20",
    )
    .await;
    for (element, wanted) in [("Prefix", "記録/"), ("Delimiter", " ")] {
        assert_eq!(
            decoded(&selected.field(element)),
            wanted,
            "case {:?}: expected {element} {wanted:?} once decoded, got {:?}",
            "an encoded prefix and delimiter",
            selected.field(element)
        );
        assert_ne!(
            selected.field(element),
            wanted,
            "case {:?}: {element} must reach the wire percent-encoded, got {:?}",
            "an encoded prefix and delimiter",
            selected.field(element)
        );
    }
}

/// One row of the refusal table: the case name, the bucket the request names, the query it
/// carries, its headers, and the status, error code and message it must be answered with
/// (an empty code for a row that is SERVED, where there is no error document to read).
type RefusalRow<'a> = (
    &'a str,
    &'a str,
    &'a str,
    Vec<(&'a str, &'a str)>,
    u16,
    &'a str,
    &'a str,
);

#[tokio::test]
async fn an_address_or_a_feature_this_emulator_does_not_hold_is_refused() {
    let router = router();
    let name = "multipart-listing-refusals";
    bucket(&router, name).await;
    initiate(&router, name, "alpha.bin", &[]).await;

    // (case, bucket, query, headers, status, error code, error message). Each message is
    // the wording S3 publishes for that error class, with the variable part — here the
    // argument value the request carried — taken from the request: the two `max-uploads`
    // rows differ only in that value, which is what pins it as a template rather than a
    // fixed string.
    let cases: [RefusalRow; 8] = [
        (
            "a bucket this emulator does not hold",
            "multipart-listing-absent",
            "",
            Vec::new(),
            404,
            "NoSuchBucket",
            "The specified bucket does not exist",
        ),
        (
            "a max-uploads that is not a count",
            name,
            "&max-uploads=-1",
            Vec::new(),
            400,
            "InvalidArgument",
            "Argument max-uploads must be an integer between 0 and 2147483647: -1",
        ),
        (
            "another max-uploads that is not a count",
            name,
            "&max-uploads=-2147483648",
            Vec::new(),
            400,
            "InvalidArgument",
            "Argument max-uploads must be an integer between 0 and 2147483647: -2147483648",
        ),
        // `encoding-type` is a modelled enum of one value, so a value outside it is
        // refused at the BOUNDARY, before this handler runs — and S3 reports that against
        // the encoding METHOD asked for, with its own published sentence and the
        // `<ArgumentName>` / `<ArgumentValue>` pair naming the parameter (the service-wide
        // rule in `src/service.rs`, pinned over every listing by
        // [`super::listing_echoed_members`]). The handler's own guard answers the same code
        // and the same sentence for anything that gets past that enum.
        (
            "an encoding method the model does not declare",
            name,
            "&encoding-type=gzip",
            Vec::new(),
            400,
            "InvalidArgument",
            "Invalid Encoding Method specified in Request",
        ),
        (
            "x-amz-expected-bucket-owner naming another account",
            name,
            "",
            vec![("x-amz-expected-bucket-owner", "111122223333")],
            403,
            "AccessDenied",
            "Access Denied",
        ),
        (
            "a Requester Pays confirmation",
            name,
            "",
            vec![("x-amz-request-payer", "requester")],
            501,
            "NotImplemented",
            "operation ListMultipartUploads with x-amz-request-payer is scaffolded but not \
             implemented in this emulator",
        ),
        (
            "the bucket's own owner, which is served",
            name,
            "",
            vec![(
                "x-amz-expected-bucket-owner",
                engine::emulator_account_id(),
            )],
            200,
            "",
            "",
        ),
        (
            "a max-uploads at the low end of what a count may be, which is served",
            name,
            "&max-uploads=1",
            Vec::new(),
            200,
            "",
            "",
        ),
    ];

    for (case, named, query, headers, status, code, message) in cases {
        let answer = list_uploads(&router, named, query, &headers).await;
        assert_eq!(
            answer.status,
            status,
            "case {case:?}: expected HTTP {status}, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        if !code.is_empty() {
            assert_eq!(
                answer.error_code(),
                code,
                "case {case:?}: expected the error code {code:?}, got {:?} with body {:?}",
                answer.error_code(),
                text(&answer)
            );
            assert_eq!(
                answer.error_message(),
                message,
                "case {case:?}: expected the message {message:?}, got {:?}",
                answer.error_message()
            );
        }
    }

    // The refusals left the one upload in progress exactly where it was.
    let page = listing(&router, name, "after the refusals", "").await;
    assert_eq!(
        page.entries().len(),
        1,
        "case {:?}: a refused listing records and removes nothing, got {:?}",
        "after the refusals",
        page.entries()
    );
}
