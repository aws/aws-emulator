// SPDX-License-Identifier: CC0-1.0

//! What a `ListVersionsResult` document carries on EVERY page: the echoed selection
//! parameters, and the owner of every entry.
//!
//! RULE: a version listing answers a complete result document whatever the request
//! selected — the elements that echo the request's prefix and its two position markers
//! (`Prefix`, "selects objects that start with the value supplied by this parameter";
//! `KeyMarker`, "marks the last key returned in a truncated response"; `VersionIdMarker`,
//! "marks the last version of the key returned in a truncated response") are part of every
//! answer, carrying the empty value for a request that named none, and every entry the page
//! reports — an object version (`ObjectVersion$Owner`, "specifies the owner of the object")
//! or a delete marker (`DeleteMarkerEntry$Owner`, "the account that created the delete
//! marker") — carries the owning account's canonical user id as its `Owner`. The echoed
//! values are the ones the request sent, DECODED, and are written percent-encoded only when
//! the request asked for an encoding type: "if you specify the `encoding-type` request
//! parameter, Amazon S3 includes this element in the response, and returns encoded key name
//! values in the following response elements: `KeyMarker, NextKeyMarker, Prefix, Key`, and
//! `Delimiter`" (`ListObjectVersionsOutput$EncodingType`).
//!
//! The rows below differ along WHAT one request selects — nothing at all, a prefix (sent
//! percent-encoded the way an SDK puts it on the wire, and spelled literally), a prefix that
//! matches no key, each position marker alone and the pair together, a delimiter that rolls
//! every key up, a page bound smaller than the listing, and an encoding type — because the
//! rule is about the document the answer has, not about which entries one selection yields.
//!
//! Every call goes over the wire through the harnesses the other version suites own, so what
//! is pinned is the wire behaviour rather than any handler's internals, and every assertion
//! names its case and the value it actually saw.
//!
//! [`super::listing_echoed_members`] holds the same echo rule across all four of this
//! service's listings, one row per surface; this suite is the version listing's own, where
//! the rows can also carry the two position markers together, the entries each selection
//! yields, and the `Owner` and checksum members only a version listing reports.

use service_com_amazonaws_s3_amazons3::state::canonical_user_id;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, Answer};
use super::object_and_tagging_roundtrip::{bucket, put_object, text};
use super::object_versioning::{
    delete_object, list_object_versions, set_versioning, version_listing_of, VersionListing,
    CONTAINER,
};

/// The two keys under the listing's prefix, and the one outside it, as STORED. The prefix
/// carries a space and a `/`: a space is a character `encoding-type=url` renders (as `+`),
/// and a `/` is one it leaves alone, so one prefix exercises both halves of the encoding.
const PREFIXED_FIRST: &str = "reports 2026/q1.csv";
const PREFIXED_SECOND: &str = "reports 2026/q2.csv";
const UNPREFIXED: &str = "top-level.txt";

/// The prefix the two keys above share, as STORED. Each row's query spells it as a request
/// puts it on the wire — percent-encoded — which is what the echoed element must come back
/// DECODED from.
const PREFIX: &str = "reports 2026/";

/// The header a write names the algorithm its checksum is computed under on:
/// `PutObjectRequest$ChecksumAlgorithm` is `httpHeader x-amz-sdk-checksum-algorithm`.
const ALGORITHM_HEADER: &str = "x-amz-sdk-checksum-algorithm";

/// The checksum algorithm the first key's versions are written under, and the type a single
/// write settles for it. `CRC32` is one of the algorithms a whole-object checksum may use,
/// and "for `PutObject` uploads, the checksum type is always `FULL_OBJECT`"
/// (`PutObjectRequest$ChecksumType`).
const ALGORITHM: &str = "CRC32";
const CHECKSUM_TYPE: &str = "FULL_OBJECT";

/// The algorithm every OTHER version here carries: its write named none, and "Amazon S3
/// added the default checksum, `CRC64NVME`, to the uploaded object"
/// (`PutObjectOutput$ChecksumCRC64NVME`).
const DEFAULT_ALGORITHM: &str = "CRC64NVME";

/// One row: what the request selects, and what the result document must then carry.
struct Case {
    /// What this row varies, named in every assertion it makes.
    case: &'static str,
    /// The query this row adds to `GET /<bucket>?versions`, with `{VERSION}` standing for
    /// the newest stored version id of [`PREFIXED_FIRST`] (which no row may hard-code,
    /// because the service mints it).
    query: &'static str,
    /// The `Prefix`, `KeyMarker` and `VersionIdMarker` the answer must echo, as STORED —
    /// written encoded when `encoded` is set.
    echoed: (&'static str, &'static str, &'static str),
    /// The keys the page must report as object VERSIONS, as STORED, in response order — one
    /// element per entry, so a key with two reported versions appears twice.
    versions: &'static [&'static str],
    /// The keys the page must report as DELETE MARKERS, as STORED, in response order.
    delete_markers: &'static [&'static str],
    /// The common prefixes the page must roll up, as STORED, in response order.
    common_prefixes: &'static [&'static str],
    /// Whether the request asked for `encoding-type=url`, which is when every key-name
    /// element of the answer is written percent-encoded.
    encoded: bool,
}

/// The rows: one per selection a client makes.
const CASES: &[Case] = &[
    Case {
        case: "no parameter at all",
        query: "",
        echoed: ("", "", ""),
        versions: &[PREFIXED_FIRST, PREFIXED_FIRST, PREFIXED_SECOND, UNPREFIXED],
        delete_markers: &[PREFIXED_SECOND],
        common_prefixes: &[],
        encoded: false,
    },
    Case {
        case: "a prefix sent percent-encoded, as an SDK puts it on the wire",
        query: "&prefix=reports%202026%2F",
        echoed: (PREFIX, "", ""),
        versions: &[PREFIXED_FIRST, PREFIXED_FIRST, PREFIXED_SECOND],
        delete_markers: &[PREFIXED_SECOND],
        common_prefixes: &[],
        encoded: false,
    },
    Case {
        case: "the same prefix with its separator spelled literally",
        query: "&prefix=reports%202026/",
        echoed: (PREFIX, "", ""),
        versions: &[PREFIXED_FIRST, PREFIXED_FIRST, PREFIXED_SECOND],
        delete_markers: &[PREFIXED_SECOND],
        common_prefixes: &[],
        encoded: false,
    },
    Case {
        case: "a prefix no stored key begins with",
        query: "&prefix=absent%2F",
        echoed: ("absent/", "", ""),
        versions: &[],
        delete_markers: &[],
        common_prefixes: &[],
        encoded: false,
    },
    Case {
        case: "a key-marker alone, which resumes after that whole key",
        query: "&key-marker=reports%202026%2Fq1.csv",
        echoed: ("", PREFIXED_FIRST, ""),
        versions: &[PREFIXED_SECOND, UNPREFIXED],
        delete_markers: &[PREFIXED_SECOND],
        common_prefixes: &[],
        encoded: false,
    },
    Case {
        case: "a key-marker and a version-id-marker, which resume within one key",
        query: "&key-marker=reports%202026%2Fq1.csv&version-id-marker={VERSION}",
        echoed: (
            "",
            PREFIXED_FIRST,
            // The newest stored version of that key, which this row's query names: the one
            // value in the table the service mints rather than the test choosing.
            "{VERSION}",
        ),
        versions: &[PREFIXED_FIRST, PREFIXED_SECOND, UNPREFIXED],
        delete_markers: &[PREFIXED_SECOND],
        common_prefixes: &[],
        encoded: false,
    },
    Case {
        case: "a delimiter that rolls every prefixed key up",
        query: "&delimiter=%2F",
        echoed: ("", "", ""),
        versions: &[UNPREFIXED],
        delete_markers: &[],
        common_prefixes: &[PREFIX],
        encoded: false,
    },
    Case {
        case: "a page bound smaller than the listing",
        query: "&max-keys=2",
        echoed: ("", "", ""),
        versions: &[PREFIXED_FIRST, PREFIXED_FIRST],
        delete_markers: &[],
        common_prefixes: &[],
        encoded: false,
    },
    Case {
        case: "a prefix under encoding-type=url, which every SDK listing carries",
        query: "&prefix=reports%202026%2F&encoding-type=url",
        echoed: (PREFIX, "", ""),
        versions: &[PREFIXED_FIRST, PREFIXED_FIRST, PREFIXED_SECOND],
        delete_markers: &[PREFIXED_SECOND],
        common_prefixes: &[],
        encoded: true,
    },
];

/// `value` as the answer writes it: verbatim, or the form `encoding-type=url` asks for — "a
/// space becomes `+`" and the `/` separating the elements of a key prefix is left alone
/// (`ListObjectVersionsOutput$EncodingType`). Every character of the keys here is either
/// unreserved, a space or that separator, so this is the whole encoding for them.
fn as_reported(value: &str, encoded: bool) -> String {
    match encoded {
        true => value.replace(' ', "+"),
        false => value.to_string(),
    }
}

/// The version id a write reported, failing the test when it reported none.
fn minted(answer: &Answer, case: &str) -> String {
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: the write must be accepted, got {} with body {:?}",
        answer.status,
        text(answer)
    );
    answer
        .header("x-amz-version-id")
        .unwrap_or_else(|| panic!("case {case:?}: a versioned write must report its version id"))
}

/// The bucket every row lists: versioning enabled, two versions of one checksummed key under
/// a prefix, a plain key under the same prefix whose current version is a DELETE MARKER, and
/// a plain key outside the prefix. Hands back the newest stored version id of
/// [`PREFIXED_FIRST`].
async fn given_versions(router: &MultiServiceRouter, name: &str) -> String {
    bucket(router, name).await;
    set_versioning(router, name, "Enabled").await;

    // The key is written through the wire path, so its space travels percent-encoded in the
    // request URI and is stored decoded.
    let first = "reports%202026/q1.csv";
    minted(
        &put_object(
            router,
            name,
            first,
            b"q1, first revision",
            &[(ALGORITHM_HEADER, ALGORITHM)],
        )
        .await,
        "the older version of the checksummed key",
    );
    let newest = minted(
        &put_object(
            router,
            name,
            first,
            b"q1, second revision",
            &[(ALGORITHM_HEADER, ALGORITHM)],
        )
        .await,
        "the newest version of the checksummed key",
    );

    minted(
        &put_object(router, name, "reports%202026/q2.csv", b"q2", &[]).await,
        "the key whose current version is a delete marker",
    );
    let deleted = delete_object(router, name, "reports%202026/q2.csv", "").await;
    assert_eq!(
        deleted.status,
        204,
        "a delete that names no version lays a delete marker, got {} with body {:?}",
        deleted.status,
        text(&deleted)
    );

    minted(
        &put_object(router, name, UNPREFIXED, b"top level", &[]).await,
        "the key outside the prefix",
    );
    newest
}

#[tokio::test]
async fn a_version_listing_echoes_its_selection_and_owns_every_entry() {
    let router = router();
    let name = "version-listing-echoed-members";
    let newest = given_versions(&router, name).await;

    for case in CASES {
        let query = case.query.replace("{VERSION}", &newest);
        let answer = list_object_versions(&router, name, &query).await;
        assert_eq!(
            answer.status,
            200,
            "case {:?}: a version listing of a bucket that exists is 200, got {} with body {:?}",
            case.case,
            answer.status,
            text(&answer)
        );
        let listing = version_listing_of(&answer);

        // The three echoed elements are part of the document whether or not the request
        // named them, and carry the request's own values — decoded, and re-encoded only
        // where the request asked for an encoding type.
        let (prefix, key_marker, version_id_marker) = case.echoed;
        for (element, wanted) in [
            ("Prefix", prefix),
            ("KeyMarker", key_marker),
            ("VersionIdMarker", version_id_marker),
        ] {
            assert!(
                listing.carries(element),
                "case {:?}: the result document must carry <{element}>, got body {:?}",
                case.case,
                text(&answer)
            );
            let wanted = as_reported(&wanted.replace("{VERSION}", &newest), case.encoded);
            assert_eq!(
                listing.field(element),
                wanted,
                "case {:?}: expected {element} {wanted:?}, got {:?}",
                case.case,
                listing.field(element)
            );
        }

        // The entries the selection yields, each reported under the key it is stored at, in
        // the container its kind belongs to: an object version in `Version`, a delete marker
        // in `DeleteMarker`.
        for (container, wanted) in [
            ("Version", case.versions),
            ("DeleteMarker", case.delete_markers),
        ] {
            let reported: Vec<String> = listing
                .listed()
                .iter()
                .filter(|entry| VersionListing::entry(entry, CONTAINER) == container)
                .map(|entry| VersionListing::entry(entry, "Key"))
                .collect();
            let expected: Vec<String> = wanted
                .iter()
                .map(|key| as_reported(key, case.encoded))
                .collect();
            assert_eq!(
                reported, expected,
                "case {:?}: expected the {container} entries {expected:?}, got {reported:?}",
                case.case
            );
        }
        let rolled_up: Vec<String> = case
            .common_prefixes
            .iter()
            .map(|prefix| as_reported(prefix, case.encoded))
            .collect();
        assert_eq!(
            listing.rolled_up(),
            rolled_up,
            "case {:?}: expected the common prefixes {rolled_up:?}, got {:?}",
            case.case,
            listing.rolled_up()
        );

        // Every entry — version or delete marker — names the one account this emulator
        // serves as its owner, under the canonical user id every other owner this service
        // reports carries.
        for entry in listing.listed() {
            let container = VersionListing::entry(entry, CONTAINER);
            let key = VersionListing::entry(entry, "Key");
            let owner = VersionListing::entry(entry, "Owner.ID");
            assert_eq!(
                owner,
                canonical_user_id(),
                "case {:?}, {container} of {key:?}: Owner/ID is the account's canonical user \
                 id, got {owner:?}",
                case.case
            );
            assert_eq!(
                VersionListing::entry(entry, "Owner.DisplayName"),
                "",
                "case {:?}, {container} of {key:?}: no display name is recorded, got {:?}",
                case.case,
                VersionListing::entry(entry, "Owner.DisplayName")
            );

            // The checksum members report what the write that stored THIS version recorded:
            // the algorithm it named under the type a single write settles, and the default
            // S3 adds to a write that named none
            // (`PutObjectOutput$ChecksumCRC64NVME`). A delete marker holds no
            // bytes, so it carries neither — nor an entity tag or a size.
            let (algorithm, checksum_type) = match (
                container.as_str(),
                key == as_reported(PREFIXED_FIRST, case.encoded),
            ) {
                ("Version", true) => (ALGORITHM, CHECKSUM_TYPE),
                ("Version", false) => (DEFAULT_ALGORITHM, CHECKSUM_TYPE),
                _ => ("", ""),
            };
            for (element, wanted) in [
                ("ChecksumAlgorithm", algorithm),
                ("ChecksumType", checksum_type),
            ] {
                assert_eq!(
                    VersionListing::entry(entry, element),
                    wanted,
                    "case {:?}, {container} of {key:?}: expected {element} {wanted:?}, got {:?}",
                    case.case,
                    VersionListing::entry(entry, element)
                );
            }
            if container == "DeleteMarker" {
                for element in ["ETag", "Size"] {
                    assert_eq!(
                        VersionListing::entry(entry, element),
                        "",
                        "case {:?}, the delete marker of {key:?}: it holds no bytes, so it \
                         carries no {element}, got {:?}",
                        case.case,
                        VersionListing::entry(entry, element)
                    );
                }
            } else {
                assert!(
                    !VersionListing::entry(entry, "ETag").is_empty(),
                    "case {:?}, the version of {key:?}: a stored version reports its entity \
                     tag, got {:?}",
                    case.case,
                    VersionListing::entry(entry, "ETag")
                );
            }
        }
    }
}

#[tokio::test]
async fn the_echoed_markers_are_there_for_an_empty_bucket_too() {
    // A bucket holding nothing reports no entry at all — and still answers the same complete
    // document, which is what lets a paginating client read its position out of the first
    // page it ever gets.
    let router = router();
    let name = "version-listing-empty-echo";
    bucket(&router, name).await;
    set_versioning(&router, name, "Enabled").await;

    let answer = list_object_versions(&router, name, "").await;
    assert_eq!(
        answer.status,
        200,
        "case \"an empty versioned bucket\": expected HTTP 200, got {} with body {:?}",
        answer.status,
        text(&answer)
    );
    let listing = version_listing_of(&answer);
    assert!(
        listing.listed().is_empty(),
        "case \"an empty versioned bucket\": nothing is stored, so nothing is reported, got \
         {:?}",
        listing.entries()
    );
    for element in ["Prefix", "KeyMarker", "VersionIdMarker"] {
        assert!(
            listing.carries(element),
            "case \"an empty versioned bucket\": the result document must carry <{element}>, \
             got body {:?}",
            text(&answer)
        );
        assert_eq!(
            listing.field(element),
            "",
            "case \"an empty versioned bucket\": {element} must be empty, got {:?}",
            listing.field(element)
        );
    }
    assert_eq!(
        listing.field("IsTruncated"),
        "false",
        "case \"an empty versioned bucket\": an empty page is not a truncated one, got {:?}",
        listing.field("IsTruncated")
    );
}
