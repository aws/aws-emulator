// SPDX-License-Identifier: CC0-1.0

//! What a listing of a bucket ECHOES back about the request that asked for it.
//!
//! Two rules, over every listing of what a bucket holds (`ListObjects`, `ListObjectsV2`,
//! `ListObjectVersions`, `ListMultipartUploads`):
//!
//! 1. the elements that say WHERE the listing began — the prefix it selected by and the
//!    marker(s) it started after — are part of EVERY answer, carrying the empty value for a
//!    request that named none, because a client paging with them reads them out of the
//!    response rather than out of its own request (`ListObjectsOutput$Prefix` "keys that
//!    begin with the indicated prefix" and `$Marker` "indicates where in the bucket listing
//!    begins", `ListObjectVersionsOutput$KeyMarker`/`$VersionIdMarker` "marks the last key
//!    returned"/"marks the last version of the key returned",
//!    `ListMultipartUploadsOutput$KeyMarker` "the key at or after which the listing began").
//!    The members the model's own documentation makes conditional stay conditional: a
//!    `Delimiter` is "absent from the response" when the request stated none
//!    (`ListMultipartUploadsOutput$Delimiter`), and so is the `Prefix` of the uploads
//!    listing, "when a prefix is provided in the request, this field contains the specified
//!    prefix". A narrowing parameter the request states with NO VALUE states nothing, so
//!    such a listing applies no delimiter and reports none either: the element's own
//!    presence is what tells a client the listing is hierarchical;
//! 2. the page-size element echoes the number the REQUEST asked for, while the published
//!    1,000-entry ceiling bounds only how many entries the page carries — "the response
//!    might contain fewer keys but will never contain more"
//!    (`ListObjectsV2Output$MaxKeys`). A client told the ceiling instead cannot tell a
//!    clamped request from the server-side default and mis-sizes its paging loop. The
//!    uploads listing is the one whose own member reports the bound rather than the request
//!    — "maximum number of multipart uploads that could have been included in the response"
//!    (`ListMultipartUploadsOutput$MaxUploads`) — and this suite pins it that way.
//!
//! Beside them, the third rule this suite pins: a listing refuses a value one of its own
//! query parameters cannot carry with S3's published sentence for that parameter and the
//! `<ArgumentName>` / `<ArgumentValue>` pair naming the parameter as the API publishes it —
//! the refusal of an `encoding-type` that names no encoding method, and of a
//! `continuation-token` no listing issued, whose token the document does not echo because it
//! "is obfuscated and is not a real key". An `encoding-type` the request spells in another
//! CASE names the one method the model declares and is served, echoed as the request spelled
//! it, and one stated with NO value states no encoding at all.
//!
//! And the fourth, over every listing of this service that PAGES (the objects a bucket
//! holds, the buckets an account holds, the annotations one object version holds): the
//! continuation token a truncated page hands back is an OPAQUE value the service mints and
//! the caller hands back unread — "NextContinuationToken is obfuscated and is not a real
//! key" (`ListObjectsV2Output$NextContinuationToken`), "ContinuationToken is obfuscated and
//! is not a real bucket" (`ListBucketsOutput$ContinuationToken`). So it is written in the
//! alphabet AWS writes that listing's own tokens in — URL-safe base64 for the object
//! listings, lowercase hex for the account's bucket listing — it is no rendering of the
//! position it resumes at in any spelling a
//! caller could compute, and only a value this service minted for THIS LISTING names a
//! page at all; while being a usable cursor, it resumes the listing exactly where the page
//! stopped and is the same value for two identical reads of one unchanged listing. What the
//! cursor names is a POSITION: the object listing's token resumes at the key it was issued
//! for in whatever bucket the next request addresses, because the resource that listing is
//! taken over is the `Bucket` the request names.
//!
//! The tables span what each rule generalises over: WHICH listing answers (all four, by
//! their four addresses), whether the request stated each echoed member, the page sizes
//! below, at and past the published ceiling, the values an `encoding-type` may carry
//! that are not the one the model declares, and the SHAPE of the position a continuation
//! token resumes at.
//!
//! Every call goes over the wire through the harnesses the other object suites own, so what
//! is pinned is the wire document.

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, send_bytes, Answer};
use super::continuation_token_alphabet::handed_back_in_query;
use super::list_buckets::list_buckets;
use super::object_and_tagging_roundtrip::{bucket, text};
use super::object_annotations::put_annotation;
use super::object_storage::{list_objects_v2, listing_of, store_keys};

/// The keys every bucket in this suite holds: two under one prefix and one with none, so a
/// prefix-selected listing reports part of the bucket and a plain one reports all of it.
const KEYS: &[&str] = &["notes/april.txt", "notes/may.txt", "plain.txt"];

/// The one `EncodingType` the model declares, which every listing serves.
const ENCODING_TYPE_URL: &str = "url";

/// The code and status a value a request field cannot carry is refused with.
const INVALID_ARGUMENT: &str = "InvalidArgument";

/// The published sentence a listing refuses an `encoding-type` outside the declared set
/// with, and the parameter the document names it against.
const INVALID_ENCODING_METHOD_MESSAGE: &str = "Invalid Encoding Method specified in Request";
const ENCODING_TYPE_PARAMETER: &str = "encoding-type";

/// One listing of what a bucket holds: how it is addressed, and which echoed elements its
/// answer carries whatever the request stated.
struct Surface {
    /// The operation, named in every assertion this row makes.
    call: &'static str,
    /// The query that addresses it — the subresource or discriminator, with no selection.
    address: &'static str,
    /// The elements that are part of every answer, each empty for a request that stated
    /// none, paired with the query parameter that states it (empty where this listing binds
    /// no such parameter, which is every marker but the key one).
    echoed: &'static [(&'static str, &'static str)],
    /// The element this listing reports its page size in, and the parameter that asks for
    /// one.
    page_size: (&'static str, &'static str),
    /// Whether that element reports the number the REQUEST asked for. True for the three
    /// listings of a bucket's contents and versions, whose `MaxKeys` "sets the maximum
    /// number of keys returned in the response"; false for the uploads listing, whose own
    /// member is the bound the page was computed under — "maximum number of multipart
    /// uploads that COULD HAVE BEEN included in the response"
    /// (`ListMultipartUploadsOutput$MaxUploads`).
    page_size_echoes_request: bool,
}

/// The four listings of what a bucket holds, each with the echoed elements its own output
/// members publish.
const SURFACES: &[Surface] = &[
    Surface {
        call: "ListObjects",
        address: "",
        echoed: &[("Prefix", "prefix"), ("Marker", "marker")],
        page_size: ("MaxKeys", "max-keys"),
        page_size_echoes_request: true,
    },
    Surface {
        call: "ListObjectsV2",
        address: "list-type=2",
        echoed: &[("Prefix", "prefix")],
        page_size: ("MaxKeys", "max-keys"),
        page_size_echoes_request: true,
    },
    Surface {
        call: "ListObjectVersions",
        address: "versions",
        echoed: &[
            ("Prefix", "prefix"),
            ("KeyMarker", "key-marker"),
            ("VersionIdMarker", ""),
        ],
        page_size: ("MaxKeys", "max-keys"),
        page_size_echoes_request: true,
    },
    Surface {
        call: "ListMultipartUploads",
        address: "uploads",
        echoed: &[("KeyMarker", "key-marker"), ("UploadIdMarker", "")],
        page_size: ("MaxUploads", "max-uploads"),
        page_size_echoes_request: false,
    },
];

/// `GET /<bucket>?<address><query>` — one listing, however it is addressed.
async fn list(
    router: &MultiServiceRouter,
    bucket_name: &str,
    surface: &Surface,
    query: &str,
) -> Answer {
    let uri = match (surface.address.is_empty(), query.is_empty()) {
        (true, true) => format!("/{bucket_name}"),
        (true, false) => format!("/{bucket_name}?{}", query.trim_start_matches('&')),
        (false, _) => format!("/{bucket_name}?{}{query}", surface.address),
    };
    send(router, "GET", &uri, &[], "").await
}

/// The text of the first `<name>` element of `answer`'s body, or `None` when the document
/// carries no such element at all — which is the difference this suite is about: an element
/// carrying the empty value is `Some("")`.
fn element(answer: &Answer, name: &str) -> Option<String> {
    answer.element(name)
}

#[tokio::test]
async fn a_delimiter_stated_with_no_value_is_no_delimiter_in_the_answer() {
    let router = router();
    let bucket_name = "listing-empty-delimiter";
    bucket(&router, bucket_name).await;
    store_keys(&router, bucket_name, KEYS).await;

    // A narrowing parameter stated with NO value states nothing, so the listing applies no
    // delimiter and reports none: the element is ABSENT rather than empty, because its own
    // presence is what tells a client the listing is hierarchical. The rows differ along how
    // the request writes that empty statement and what else it states beside it; the third
    // field is the same request written as one that never stated the parameter, which is the
    // answer an empty value must be served.
    let rows: &[(&str, &str, &str)] = &[
        ("the parameter alone", "&delimiter=", ""),
        (
            "the parameter before another",
            "&delimiter=&max-keys=2",
            "&max-keys=2",
        ),
        (
            "the parameter after another",
            "&max-keys=2&delimiter=",
            "&max-keys=2",
        ),
    ];

    for surface in SURFACES {
        for (why, query, without) in rows {
            let case = format!("{}/{why}", surface.call);
            let answer = list(&router, bucket_name, surface, query).await;
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: a listing that states no delimiter is served, got {} with \
                 body {:?}",
                answer.status,
                text(&answer)
            );
            assert_eq!(
                element(&answer, "Delimiter"),
                None,
                "case {case:?}: a listing that applied no delimiter reports none, got {:?} in \
                 {:?}",
                element(&answer, "Delimiter"),
                text(&answer)
            );
            // And nothing is rolled up: the keys the bucket holds are reported flat, which
            // is the same answer a request that left the parameter out is served.
            if surface.call == "ListObjectsV2" || surface.address.is_empty() {
                let listing = listing_of(&answer);
                assert!(
                    listing.common_prefixes.is_empty(),
                    "case {case:?}: an empty delimiter rolls nothing up, got {:?}",
                    listing.common_prefixes
                );
                let flat = listing_of(&list(&router, bucket_name, surface, without).await);
                assert_eq!(
                    listing.keys(),
                    flat.keys(),
                    "case {case:?}: the page must report what a request stating no delimiter \
                     at all reports, got {:?} against {:?}",
                    listing.keys(),
                    flat.keys()
                );
            }
        }
    }
}

#[tokio::test]
async fn every_listing_echoes_where_it_began_even_when_the_request_states_nothing() {
    let router = router();
    let bucket_name = "listing-echoed-members";
    bucket(&router, bucket_name).await;
    store_keys(&router, bucket_name, KEYS).await;

    for surface in SURFACES {
        // (case, the extra query, whether each echoed member was stated). A request that
        // states nothing, one that states the prefix, one that states the key marker, and
        // one that states both — the echo must be present in all four, carrying what the
        // request stated or the empty value.
        let stated: Vec<(&str, String)> = vec![
            ("no selection at all", String::new()),
            ("a prefix", "&prefix=notes/".to_string()),
            ("a marker", "&key-marker=notes/april.txt".to_string()),
            (
                "a prefix and a marker",
                "&prefix=notes/&key-marker=notes/april.txt".to_string(),
            ),
        ];
        for (selection, query) in stated {
            // Each row's query is written with the parameter names THIS listing binds, so a
            // marker row addresses `marker` on the revision that calls it that.
            let query = match surface.echoed.iter().any(|(_, parameter)| *parameter == "marker") {
                true => query.replace("key-marker=", "marker="),
                false => query,
            };
            let case = format!("{}/{selection}", surface.call);
            let answer = list(&router, bucket_name, surface, &query).await;
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: a listing of a bucket that exists answers 200, got {} with \
                 body {:?}",
                answer.status,
                text(&answer)
            );
            for (name, parameter) in surface.echoed {
                // What the request stated for this member, or the empty value where it
                // stated none — including a member this listing binds no parameter for.
                let expected = match parameter.is_empty() {
                    true => String::new(),
                    false => query
                        .split('&')
                        .filter_map(|pair| pair.split_once('='))
                        .find(|(key, _)| key == parameter)
                        .map(|(_, value)| value.to_string())
                        .unwrap_or_default(),
                };
                assert_eq!(
                    element(&answer, name).as_deref(),
                    Some(expected.as_str()),
                    "case {case:?}: <{name}> is part of every answer and carries {expected:?} \
                     here, got {:?} in {:?}",
                    element(&answer, name),
                    text(&answer)
                );
            }
            // A `Delimiter` the request did not state is absent from the response, so the
            // rule above is about the members that say where the listing began and not
            // about every optional member.
            assert_eq!(
                element(&answer, "Delimiter"),
                None,
                "case {case:?}: a listing that stated no delimiter reports none, got {:?} in \
                 {:?}",
                element(&answer, "Delimiter"),
                text(&answer)
            );
        }

        // And the uploads listing's own `Prefix` stays conditional, which is what its member
        // publishes: "when a prefix is provided in the request, this field contains the
        // specified prefix".
        if surface.call == "ListMultipartUploads" {
            let plain = list(&router, bucket_name, surface, "").await;
            assert_eq!(
                element(&plain, "Prefix"),
                None,
                "case {}/\"no selection at all\": the uploads listing reports a Prefix only \
                 when the request provided one, got {:?} in {:?}",
                surface.call,
                element(&plain, "Prefix"),
                text(&plain)
            );
            let selected = list(&router, bucket_name, surface, "&prefix=notes/").await;
            assert_eq!(
                element(&selected, "Prefix").as_deref(),
                Some("notes/"),
                "case {}/\"a prefix\": the uploads listing echoes the prefix it was given, \
                 got {:?} in {:?}",
                surface.call,
                element(&selected, "Prefix"),
                text(&selected)
            );
        }
    }
}

/// The published ceiling on one page of a listing: "by default, the action returns up to
/// 1,000 key names. The response might contain fewer keys but will never contain more"
/// (`ListObjectsV2Request$MaxKeys`).
const PAGE_CEILING: i64 = 1000;

#[tokio::test]
async fn a_listing_echoes_the_page_size_the_request_asked_for() {
    let router = router();
    let bucket_name = "listing-echoed-page-size";
    bucket(&router, bucket_name).await;
    store_keys(&router, bucket_name, KEYS).await;

    // Page sizes spanning the class: one entry, fewer than the bucket holds, exactly what it
    // holds, more than it holds, one below the published ceiling, the ceiling itself, one
    // past it, a value far past it, and the largest the member admits — plus `0`, which asks
    // for no entries at all. A request that states none is answered with the default, which
    // is the ceiling.
    let sizes: &[i64] = &[
        0,
        1,
        2,
        KEYS.len() as i64,
        KEYS.len() as i64 + 1,
        PAGE_CEILING - 1,
        PAGE_CEILING,
        PAGE_CEILING + 1,
        5_000,
        i32::MAX as i64,
    ];

    for surface in SURFACES {
        let (element_name, parameter) = surface.page_size;
        for size in sizes {
            let case = format!("{}/{parameter}={size}", surface.call);
            let answer = list(&router, bucket_name, surface, &format!("&{parameter}={size}")).await;
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: a page size the member admits is served, got {} with body \
                 {:?}",
                answer.status,
                text(&answer)
            );
            let bound = (*size).min(PAGE_CEILING);
            // The echo is the caller's number where the member reports the request, and
            // the ceiling-bounded page size where it reports the bound the page was
            // computed under.
            let expected = match surface.page_size_echoes_request {
                true => *size,
                false => bound,
            };
            assert_eq!(
                element(&answer, element_name).as_deref(),
                Some(expected.to_string().as_str()),
                "case {case:?}: <{element_name}> reports {expected}, got {:?} in {:?}",
                element(&answer, element_name),
                text(&answer)
            );
            // And the page itself never carries more than the ceiling, nor more than the
            // request asked for: the echo above is what the caller asked for, not a promise
            // about the page.
            if surface.call == "ListObjectsV2" || surface.address.is_empty() {
                let reported = listing_of(&answer).keys().len() as i64;
                assert!(
                    reported <= bound,
                    "case {case:?}: a page may carry at most {bound} entries, got {reported}"
                );
            }
        }

        // A request that asks for no page size at all is answered with the default.
        let case = format!("{}/no {parameter}", surface.call);
        let answer = list(&router, bucket_name, surface, "").await;
        assert_eq!(
            element(&answer, element_name).as_deref(),
            Some(PAGE_CEILING.to_string().as_str()),
            "case {case:?}: a request that asked for no page size is answered with the \
             default, got {:?} in {:?}",
            element(&answer, element_name),
            text(&answer)
        );
    }
}

#[tokio::test]
async fn a_listing_refuses_an_encoding_type_by_naming_the_parameter_and_the_value() {
    let router = router();
    let bucket_name = "listing-encoding-method";
    bucket(&router, bucket_name).await;
    store_keys(&router, bucket_name, KEYS).await;

    // Values an `encoding-type` may carry that name no encoding method: a word, and another
    // encoding a client might guess at. The DECLARED value in another case is NOT among them —
    // S3 reads the value as a mode name — and neither is the empty value, which states no
    // encoding at all; both are served below.
    let rejected: &[(&str, &str)] = &[
        ("a word that is no encoding method", "bogus"),
        ("an encoding this service does not write", "base64"),
    ];

    for surface in SURFACES {
        for (why, value) in rejected {
            let case = format!("{}/{why}", surface.call);
            let answer = list(
                &router,
                bucket_name,
                surface,
                &format!("&encoding-type={value}"),
            )
            .await;
            assert_eq!(
                answer.status,
                400,
                "case {case:?}: expected HTTP 400, got {} with body {:?}",
                answer.status,
                text(&answer)
            );
            assert_eq!(
                answer.error_code(),
                INVALID_ARGUMENT,
                "case {case:?}: expected the {INVALID_ARGUMENT:?} code, got {:?}",
                answer.error_code()
            );
            assert_eq!(
                answer.error_message(),
                INVALID_ENCODING_METHOD_MESSAGE,
                "case {case:?}: expected the published sentence \
                 {INVALID_ENCODING_METHOD_MESSAGE:?}, got {:?}",
                answer.error_message()
            );
            assert_eq!(
                element(&answer, "ArgumentName").as_deref(),
                Some(ENCODING_TYPE_PARAMETER),
                "case {case:?}: the envelope must name the parameter it rejected, got {:?} \
                 in {:?}",
                element(&answer, "ArgumentName"),
                text(&answer)
            );
            assert_eq!(
                element(&answer, "ArgumentValue").as_deref(),
                Some(*value),
                "case {case:?}: the envelope must echo the value that parameter carried, got \
                 {:?} in {:?}",
                element(&answer, "ArgumentValue"),
                text(&answer)
            );
        }

        // The value the model declares is served HOWEVER the request spells it — S3 reads it
        // as a mode name — and the answer echoes the spelling the request used, so every
        // refusal above was about the method named rather than about its case or about the
        // parameter being there.
        for spelled in [ENCODING_TYPE_URL, "URL", "Url"] {
            let case = format!("{}/encoding-type={spelled}", surface.call);
            let served = list(
                &router,
                bucket_name,
                surface,
                &format!("&encoding-type={spelled}"),
            )
            .await;
            assert_eq!(
                served.status,
                200,
                "case {case:?}: the declared encoding type is served however it is spelled, \
                 got {} with body {:?}",
                served.status,
                text(&served)
            );
            assert_eq!(
                element(&served, "EncodingType").as_deref(),
                Some(spelled),
                "case {case:?}: a listing that applied an encoding echoes it as the request \
                 spelled it, got {:?} in {:?}",
                element(&served, "EncodingType"),
                text(&served)
            );
        }

        // And the parameter stated with NO value asks for no encoding at all: the listing is
        // served and reports no encoding type, exactly as one that left the parameter out.
        let case = format!("{}/encoding-type=", surface.call);
        let unencoded = list(&router, bucket_name, surface, "&encoding-type=").await;
        assert_eq!(
            unencoded.status,
            200,
            "case {case:?}: a parameter stated with no value states no encoding, got {} with \
             body {:?}",
            unencoded.status,
            text(&unencoded)
        );
        assert_eq!(
            element(&unencoded, "EncodingType"),
            None,
            "case {case:?}: a listing that applied no encoding reports none, got {:?} in {:?}",
            element(&unencoded, "EncodingType"),
            text(&unencoded)
        );
    }
}

#[tokio::test]
async fn a_continuation_token_no_listing_issued_is_refused_by_naming_the_parameter() {
    let router = router();
    let bucket_name = "listing-continuation-token";
    bucket(&router, bucket_name).await;
    store_keys(&router, bucket_name, KEYS).await;

    // Tokens no listing of this service issued: a word, hex that decodes to bytes carrying
    // no cursor of this service's own, and the empty value.
    for (why, token) in [
        ("a word that is no token", "not-a-token"),
        ("hex carrying no cursor", "6162636465"),
        ("no value at all", ""),
    ] {
        let case = format!("ListObjectsV2/{why}");
        let answer = send(
            &router,
            "GET",
            &format!("/{bucket_name}?list-type=2&continuation-token={token}"),
            &[],
            "",
        )
        .await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected HTTP 400, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        assert_eq!(
            answer.error_code(),
            INVALID_ARGUMENT,
            "case {case:?}: expected the {INVALID_ARGUMENT:?} code, got {:?}",
            answer.error_code()
        );
        assert_eq!(
            answer.error_message(),
            "The continuation token provided is incorrect",
            "case {case:?}: expected the published sentence, got {:?}",
            answer.error_message()
        );
        assert_eq!(
            element(&answer, "ArgumentName").as_deref(),
            Some("continuation-token"),
            "case {case:?}: the envelope must name the parameter it rejected, got {:?} in \
             {:?}",
            element(&answer, "ArgumentName"),
            text(&answer)
        );
        // The token is "obfuscated and is not a real key", so the document echoes no value
        // for it: the parameter name is the whole of what the caller has to fix.
        assert_eq!(
            element(&answer, "ArgumentValue"),
            None,
            "case {case:?}: the envelope echoes no token back, got {:?} in {:?}",
            element(&answer, "ArgumentValue"),
            text(&answer)
        );
    }

    // A token the listing itself handed back resumes the walk, so every refusal above was
    // about the token provided rather than about the parameter being there.
    let first = send(
        &router,
        "GET",
        &format!("/{bucket_name}?list-type=2&max-keys=1"),
        &[],
        "",
    )
    .await;
    let page = listing_of(&first);
    let issued = page.field("NextContinuationToken");
    assert!(
        !issued.is_empty(),
        "a truncated page must hand back a continuation token, got body {:?}",
        text(&first)
    );
    let resumed = send(
        &router,
        "GET",
        &format!(
            "/{bucket_name}?list-type=2&continuation-token={}",
            handed_back_in_query(&issued)
        ),
        &[],
        "",
    )
    .await;
    assert_eq!(
        resumed.status,
        200,
        "the token this listing issued must resume it, got {} with body {:?}",
        resumed.status,
        text(&resumed)
    );
    assert_eq!(
        listing_of(&resumed).keys(),
        KEYS[1..].iter().map(|key| (*key).to_string()).collect::<Vec<_>>(),
        "the resumed page must report the keys the first page left, got {:?}",
        listing_of(&resumed).keys()
    );
}

/// Whether `token` is written in base64's URL and filename safe alphabet, unpadded (RFC 4648
/// §5), which is the alphabet an object listing's continuation token is spelled in.
fn is_base64(token: &str) -> bool {
    !token.is_empty()
        && token
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

/// Whether `token` is written in lowercase hexadecimal, which is the alphabet the account's
/// own bucket listing spells its continuation token in.
fn is_lowercase_hex(token: &str) -> bool {
    !token.is_empty() && token.chars().all(|character| matches!(character, '0'..='9' | 'a'..='f'))
}

/// Every spelling of `position` a caller could compute for itself, none of which a
/// continuation token may be: the position, its hex, its base64, and the hex and base64 of a
/// cursor carrying a marker beside it — the renderings a client would reach for if it tried
/// to mint a cursor of its own rather than round-trip what it was handed.
fn computable_spellings(position: &str) -> Vec<(String, String)> {
    let mut spellings = vec![
        ("the position itself".to_string(), position.to_string()),
        ("hex of the position".to_string(), hex::encode(position)),
        (
            "base64 of the position".to_string(),
            aws_smithy_types::base64::encode(position),
        ),
    ];
    // Markers a client could guess a cursor is laid out with, beside the position.
    for marker in ["s3v1 ", "s3lb1 ", "s3oa1 "] {
        let cursor = format!("{marker}{position}");
        spellings.push((format!("hex of a {marker:?} cursor"), hex::encode(&cursor)));
        spellings.push((
            format!("base64 of a {marker:?} cursor"),
            aws_smithy_types::base64::encode(cursor.as_str()),
        ));
    }
    spellings
}

/// `token` with its FIRST character changed — a value one character away from the token a
/// listing really issued, which only the authenticator inside it can refuse.
///
/// The first character rather than the last: the authenticator leads the value, so a change
/// there is read by the authenticator wherever a listing's spelling puts its padding.
fn flip_first(token: &str) -> String {
    let mut characters: Vec<char> = token.chars().collect();
    if let Some(first) = characters.first_mut() {
        *first = match *first {
            'A' => 'B',
            _ => 'A',
        };
    }
    characters.into_iter().collect()
}

/// Assert that `token` is an opaque cursor over `position`: URL-safe base64 text, and no
/// rendering of
/// that position a caller could compute for itself.
fn assert_opaque(case: &str, token: &str, position: &str) {
    assert!(
        is_base64(token),
        "case {case:?}: a continuation token is URL-safe base64 text, got {token:?}"
    );
    assert_no_computable_spelling(case, token, position);
    // And the position's own text is nowhere in it — read only for a position wide enough
    // that a chance run of token characters could not be one.
    if position.len() >= 3 {
        assert!(
            !token.contains(position) && !token.contains(&hex::encode(position)),
            "case {case:?}: the token must carry no rendering of {position:?}, got {token:?}"
        );
    }
}

/// Assert that `token` is an opaque cursor the BUCKET listing minted: lowercase hex text, and
/// no rendering of the position a caller could compute for itself.
///
/// A hex-spelled token is base16 of the authenticated cursor, so the cursor's own hex is
/// inside it — the value is one no caller can MINT (the authenticator is what decides that,
/// pinned by the forged-token rows), not one whose bytes are hidden: "obfuscated and is not a
/// real bucket" is the claim the member makes, and the value is no bucket name.
fn assert_opaque_hex(case: &str, token: &str, position: &str) {
    assert!(
        is_lowercase_hex(token),
        "case {case:?}: the bucket listing's continuation token is lowercase hex, got {token:?}"
    );
    assert_no_computable_spelling(case, token, position);
}

/// Assert `token` is none of the values a caller could compute for `position` itself.
fn assert_no_computable_spelling(case: &str, token: &str, position: &str) {
    for (spelling, computed) in computable_spellings(position) {
        assert_ne!(
            token, computed,
            "case {case:?}: the token must not be {spelling}, got {token:?}"
        );
    }
}

/// Assert that `answer` is the refusal a listing answers a token it did not issue with.
///
/// `names_the_parameter` is true for the object listing, whose refusal carries the
/// `<ArgumentName>` pair this suite pins above; the two listings whose refusal is the plain
/// error document answer the same code and sentence without it.
fn assert_token_refused(case: &str, answer: &Answer, names_the_parameter: bool) {
    assert_eq!(
        answer.status,
        400,
        "case {case:?}: expected HTTP 400, got {} with body {:?}",
        answer.status,
        text(answer)
    );
    assert_eq!(
        answer.error_code(),
        INVALID_ARGUMENT,
        "case {case:?}: expected the {INVALID_ARGUMENT:?} code, got {:?}",
        answer.error_code()
    );
    assert_eq!(
        answer.error_message(),
        INVALID_CONTINUATION_TOKEN_MESSAGE,
        "case {case:?}: expected the published sentence \
         {INVALID_CONTINUATION_TOKEN_MESSAGE:?}, got {:?}",
        answer.error_message()
    );
    if names_the_parameter {
        assert_eq!(
            element(answer, "ArgumentName").as_deref(),
            Some("continuation-token"),
            "case {case:?}: the envelope must name the parameter it rejected, got {:?} in {:?}",
            element(answer, "ArgumentName"),
            text(answer)
        );
        // The token "is obfuscated and is not a real key", so none of it is echoed back.
        assert_eq!(
            element(answer, "ArgumentValue"),
            None,
            "case {case:?}: the envelope echoes no token back, got {:?} in {:?}",
            element(answer, "ArgumentValue"),
            text(answer)
        );
    }
}

/// The sentence a listing refuses a continuation token it did not issue with.
const INVALID_CONTINUATION_TOKEN_MESSAGE: &str = "The continuation token provided is incorrect";

/// `GET /<bucket>/<key>?annotation&x-id=ListObjectAnnotations…` — `ListObjectAnnotations`,
/// with whatever extra query parameters the case carries. The `x-id` literal is what the
/// model binds to separate this listing from `GetObjectAnnotation`.
async fn list_annotations(
    router: &MultiServiceRouter,
    bucket_name: &str,
    key: &str,
    query: &str,
) -> Answer {
    let uri = format!("/{bucket_name}/{key}?annotation&x-id=ListObjectAnnotations{query}");
    send_bytes(router, "GET", &uri, &[], b"").await
}

/// Every bucket name in the paged-bucket-listing table begins with this, so the walk below
/// reports the buckets this test made and nothing else the account holds.
const BUCKET_PREFIX: &str = "echoed-token-buckets-";

#[tokio::test]
async fn the_object_listing_hands_back_an_opaque_token_whatever_key_it_resumes_at() {
    // (case, the keys the bucket holds) — the rows differ along the SHAPE of the key the
    // token resumes at, which is what a key cursor generalises over: a flat name, a name
    // under a path, punctuation a client might expect an encoding to touch, names ordered
    // by their digits, and a name far longer than the rest.
    let rows: &[(&str, &[&str])] = &[
        ("a flat name", &["alpha.txt", "beta.txt", "gamma.txt"]),
        (
            "a nested path",
            &[
                "media/2024/april.jpg",
                "media/2024/may.jpg",
                "media/2025/june.jpg",
            ],
        ),
        (
            "punctuation in the name",
            &["report-2024.01.csv", "report-2024.02.csv"],
        ),
        (
            "digits that order the walk",
            &["page0001", "page0002", "page0010"],
        ),
        (
            "a name far longer than the rest",
            &[
                "a.txt",
                "deeply/nested/prefix/that/goes/on/for/a/while/and/then/some/more/object.bin",
            ],
        ),
    ];

    for (index, (case, keys)) in rows.iter().enumerate() {
        let router = router();
        let bucket_name = format!("echoed-token-objects-{index}");
        bucket(&router, &bucket_name).await;
        store_keys(&router, &bucket_name, keys).await;

        // The listing itself is the order of truth: the key the first page reports and the
        // key the token must resume at come out of the full listing, not out of this table.
        let whole = list_objects_v2(&router, &bucket_name, "").await;
        let listed = listing_of(&whole).keys();
        assert_eq!(
            listed.len(),
            keys.len(),
            "case {case:?}: the bucket must list every stored key, got {listed:?} in {:?}",
            text(&whole)
        );

        let first = list_objects_v2(&router, &bucket_name, "&max-keys=1").await;
        assert_eq!(
            first.status,
            200,
            "case {case:?}: a bounded listing is served, got {} with body {:?}",
            first.status,
            text(&first)
        );
        assert_eq!(
            listing_of(&first).keys(),
            listed[..1].to_vec(),
            "case {case:?}: the first page reports the first key, got {:?}",
            listing_of(&first).keys()
        );
        let token = element(&first, "NextContinuationToken").unwrap_or_else(|| {
            panic!(
                "case {case:?}: a truncated page must hand back a token, got {:?}",
                text(&first)
            )
        });
        let resumed_at = listed[1].clone();
        assert_opaque(case, &token, &resumed_at);

        // It resumes exactly where the page stopped.
        let second = list_objects_v2(
            &router,
            &bucket_name,
            &format!(
                "&max-keys=1&continuation-token={}",
                handed_back_in_query(&token)
            ),
        )
        .await;
        assert_eq!(
            second.status,
            200,
            "case {case:?}: the token this listing issued must resume it, got {} with body {:?}",
            second.status,
            text(&second)
        );
        assert_eq!(
            listing_of(&second).keys(),
            vec![resumed_at.clone()],
            "case {case:?}: expected the listing to resume at {resumed_at:?}, got {:?}",
            listing_of(&second).keys()
        );
        // "If ContinuationToken was sent with the request, it is included in the response":
        // the value as sent, which is what the caller round-trips.
        assert_eq!(
            element(&second, "ContinuationToken").as_deref(),
            Some(token.as_str()),
            "case {case:?}: the page echoes the token it was given, got {:?}",
            element(&second, "ContinuationToken")
        );

        // Two identical reads of one unchanged bucket hand back the SAME token.
        let again = list_objects_v2(&router, &bucket_name, "&max-keys=1").await;
        assert_eq!(
            element(&again, "NextContinuationToken").as_deref(),
            Some(token.as_str()),
            "case {case:?}: an unchanged listing hands back the same token, got {:?}",
            element(&again, "NextContinuationToken")
        );

        // Values a caller could assemble rather than round-trip: every spelling of the
        // position above, a token one character away from the real one, and text that is no
        // encoding at all.
        let mut forged = computable_spellings(&resumed_at);
        forged.push((
            "a token with its last character changed".to_string(),
            flip_first(&token),
        ));
        forged.push((
            "not an encoding at all".to_string(),
            "not.a.token".to_string(),
        ));
        for (why, value) in forged {
            let refused = list_objects_v2(
                &router,
                &bucket_name,
                &format!(
                    "&max-keys=1&continuation-token={}",
                    handed_back_in_query(&value)
                ),
            )
            .await;
            assert_token_refused(&format!("{case}/{why}"), &refused, true);
        }

        // A token minted over ANOTHER bucket's listing of the same family names the same
        // POSITION there: the cursor is the key the next page starts at, not a handle on the
        // bucket it was issued over, so it resumes the listing of whatever bucket the request
        // addresses.
        let elsewhere = format!("echoed-token-objects-{index}-elsewhere");
        bucket(&router, &elsewhere).await;
        store_keys(&router, &elsewhere, keys).await;
        let foreign = list_objects_v2(
            &router,
            &elsewhere,
            &format!(
                "&max-keys=1&continuation-token={}",
                handed_back_in_query(&token)
            ),
        )
        .await;
        assert_eq!(
            foreign.status,
            200,
            "case {case:?}: a token issued over another bucket resumes this listing, got {} \
             with body {:?}",
            foreign.status,
            text(&foreign)
        );
        assert_eq!(
            listing_of(&foreign).keys(),
            vec![resumed_at.clone()],
            "case {case:?}: a token issued over another bucket resumes at {resumed_at:?}, got \
             {:?}",
            listing_of(&foreign).keys()
        );

        // And every refusal left the listing exactly as it was.
        let unchanged = list_objects_v2(&router, &bucket_name, "").await;
        assert_eq!(
            listing_of(&unchanged).keys(),
            listed,
            "case {case:?}: the listing must be unchanged by every refusal, got {:?}",
            listing_of(&unchanged).keys()
        );
    }
}

/// Every bucket `<Name>` a `ListBuckets` body reports, in response order.
fn listed_bucket_names(answer: &Answer) -> Vec<String> {
    text(answer)
        .split("<Name>")
        .skip(1)
        .filter_map(|chunk| chunk.split("</Name>").next())
        .map(str::to_string)
        .collect()
}

#[tokio::test]
async fn the_bucket_listing_hands_back_an_opaque_token_that_resumes_the_account() {
    let router = router();
    // Bucket names spanning what one can be: a short one, a dotted one, a dashed one and
    // one far longer than the others — the names this listing's cursor is a position in.
    let names: Vec<String> = ["a", "b.dotted", "c-dashed", "d-far-longer-than-the-others"]
        .iter()
        .map(|suffix| format!("{BUCKET_PREFIX}{suffix}"))
        .collect();
    for name in &names {
        bucket(&router, name).await;
    }

    let whole = list_buckets(&router, &format!("&prefix={BUCKET_PREFIX}")).await;
    let listed = listed_bucket_names(&whole);
    assert_eq!(
        listed.len(),
        names.len(),
        "the account must list every created bucket, got {listed:?}"
    );

    // One bucket per page: the cursor names the bucket the page REPORTED, and the next page
    // begins after it.
    let mut token = String::new();
    for (page, expected) in listed.iter().enumerate() {
        let query = match token.is_empty() {
            true => format!("&max-buckets=1&prefix={BUCKET_PREFIX}"),
            false => format!(
                "&max-buckets=1&prefix={BUCKET_PREFIX}&continuation-token={}",
                handed_back_in_query(&token)
            ),
        };
        let answer = list_buckets(&router, &query).await;
        assert_eq!(
            answer.status,
            200,
            "case \"page {page}\": expected 200, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        assert_eq!(
            listed_bucket_names(&answer),
            vec![expected.clone()],
            "case \"page {page}\": expected the listing to resume at {expected:?}, got {:?}",
            listed_bucket_names(&answer)
        );
        match element(&answer, "ContinuationToken") {
            Some(next) if !next.is_empty() => {
                assert_opaque_hex(&format!("page {page}"), &next, expected);
                token = next;
            }
            // Only the last page owes no cursor, which is how the walk ends.
            _ => {
                assert_eq!(
                    page + 1,
                    listed.len(),
                    "case \"page {page}\": only the last page owes no continuation token, got \
                     {:?}",
                    text(&answer)
                );
                token.clear();
            }
        }
    }

    // Values a caller could assemble rather than round-trip, including the token the OBJECT
    // listing minted: a token is a cursor only into the listing that issued it.
    let objects = format!("{BUCKET_PREFIX}objects");
    bucket(&router, &objects).await;
    store_keys(&router, &objects, &["one.txt", "two.txt"]).await;
    let object_page = list_objects_v2(&router, &objects, "&max-keys=1").await;
    let object_token = element(&object_page, "NextContinuationToken")
        .expect("a truncated object listing hands back a token");
    let mut forged = computable_spellings(&listed[0]);
    forged.push((
        "a token the object listing minted".to_string(),
        object_token.clone(),
    ));
    forged.push((
        "not an encoding at all".to_string(),
        "not.a.token".to_string(),
    ));
    for (why, value) in forged {
        let refused = list_buckets(
            &router,
            &format!(
                "&max-buckets=1&prefix={BUCKET_PREFIX}&continuation-token={}",
                handed_back_in_query(&value)
            ),
        )
        .await;
        assert_token_refused(&format!("ListBuckets/{why}"), &refused, false);
    }

    // And the bucket listing's own token names no page of the object listing either.
    let first = list_buckets(&router, &format!("&max-buckets=1&prefix={BUCKET_PREFIX}")).await;
    let bucket_token = element(&first, "ContinuationToken")
        .expect("a truncated bucket listing hands back a token");
    let crossed = list_objects_v2(
        &router,
        &objects,
        &format!(
            "&max-keys=1&continuation-token={}",
            handed_back_in_query(&bucket_token)
        ),
    )
    .await;
    assert_token_refused(
        "ListObjectsV2/a token the bucket listing minted",
        &crossed,
        true,
    );
}

#[tokio::test]
async fn the_annotation_listing_token_is_a_cursor_only_into_the_object_it_was_issued_over() {
    let router = router();
    let bucket_name = "echoed-token-annotations";
    bucket(&router, bucket_name).await;
    // Two objects carrying the same annotation names, so a token issued over one is a value
    // the other's listing could otherwise have read as a cursor of its own.
    let keys = &["first.txt", "second.txt"];
    store_keys(&router, bucket_name, keys).await;
    // Annotation names spanning what one can be: a plain word, one carrying the punctuation
    // the naming rules allow, and one far longer than the others.
    let annotations = &[
        "alpha",
        "beta.report-2024",
        "gamma-far-longer-than-the-others",
    ];
    for key in keys {
        for name in annotations {
            let written = put_annotation(&router, bucket_name, key, name, b"payload", &[]).await;
            assert_eq!(
                written.status,
                200,
                "case {key:?}/{name:?}: the annotation must be written, got {} with body {:?}",
                written.status,
                text(&written)
            );
        }
    }

    let first = list_annotations(&router, bucket_name, keys[0], "&max-annotation-results=1").await;
    assert_eq!(
        first.status,
        200,
        "case \"the first page\": expected 200, got {} with body {:?}",
        first.status,
        text(&first)
    );
    let token = element(&first, "NextContinuationToken").unwrap_or_else(|| {
        panic!(
            "case \"the first page\": a truncated listing hands back a token, got {:?}",
            text(&first)
        )
    });
    // The annotations are reported in ascending name order, so the page resumes at the
    // second name.
    let resumed_at = {
        let mut sorted: Vec<&str> = annotations.to_vec();
        sorted.sort_unstable();
        sorted[1].to_string()
    };
    assert_opaque("ListObjectAnnotations", &token, &resumed_at);

    let second = list_annotations(
        &router,
        bucket_name,
        keys[0],
        &format!(
            "&max-annotation-results=1&continuation-token={}",
            handed_back_in_query(&token)
        ),
    )
    .await;
    assert_eq!(
        element(&second, "AnnotationName").as_deref(),
        Some(resumed_at.as_str()),
        "case \"the second page\": expected the listing to resume at {resumed_at:?}, got {:?}",
        text(&second)
    );

    // The same token over ANOTHER object names no page: the cursor is scoped to the object
    // version the listing was taken over.
    let foreign = list_annotations(
        &router,
        bucket_name,
        keys[1],
        &format!(
            "&max-annotation-results=1&continuation-token={}",
            handed_back_in_query(&token)
        ),
    )
    .await;
    assert_token_refused(
        "ListObjectAnnotations/a token issued over another object",
        &foreign,
        false,
    );

    // And so does every value a caller could assemble for itself.
    let mut forged = computable_spellings(&resumed_at);
    forged.push((
        "a token with its last character changed".to_string(),
        flip_first(&token),
    ));
    forged.push((
        "not an encoding at all".to_string(),
        "not.a.token".to_string(),
    ));
    for (why, value) in forged {
        let refused = list_annotations(
            &router,
            bucket_name,
            keys[0],
            &format!(
                "&max-annotation-results=1&continuation-token={}",
                handed_back_in_query(&value)
            ),
        )
        .await;
        assert_token_refused(&format!("ListObjectAnnotations/{why}"), &refused, false);
    }
}

/// The keys the bucket that ISSUES the token below holds: two under one prefix and one with
/// none, so the position a token names is not always a short name.
const TOKEN_POSITION_KEYS: &[&str] = &["notes/april.txt", "notes/may.txt", "plain.txt"];

/// One bucket an issued object-listing token is handed to, and the page that token names
/// there.
struct Resumed {
    case: &'static str,
    /// The keys the addressed bucket holds.
    keys: &'static [&'static str],
    /// The keys the resumed page must report, in listing order.
    reports: &'static [&'static str],
}

/// RULE: an object listing's continuation token names the POSITION the next page starts at and
/// not the bucket it was issued over — the resource the listing is taken over is the `Bucket`
/// the request addresses — so a token this service issued resumes the object listing of
/// whatever bucket the next request names, at that position.
///
/// The rows differ in where the issued position falls among the addressed bucket's keys
/// (inside them, before all of them, after all of them, exactly at one) and in how many keys
/// that bucket holds.
#[tokio::test]
async fn an_object_listing_token_names_a_position_and_resumes_whichever_bucket_is_addressed() {
    let router = router();
    let issuer = "token-position-issuer";
    bucket(&router, issuer).await;
    store_keys(&router, issuer, TOKEN_POSITION_KEYS).await;

    let first = list_objects_v2(&router, issuer, "&max-keys=1").await;
    let token = element(&first, "NextContinuationToken").unwrap_or_else(|| {
        panic!(
            "case \"the issuing bucket\": a truncated page hands back a token, got {:?}",
            text(&first)
        )
    });
    let listed = listing_of(&list_objects_v2(&router, issuer, "").await).keys();
    let position = listed[1].clone();
    assert_opaque("the issuing bucket", &token, &position);

    let rows: &[Resumed] = &[
        Resumed {
            case: "a bucket holding the same keys resumes at the same key",
            keys: &["notes/april.txt", "notes/may.txt", "plain.txt"],
            reports: &["notes/may.txt", "plain.txt"],
        },
        Resumed {
            case: "a bucket whose keys straddle the position resumes after it",
            keys: &["aaa.txt", "notes/zulu.txt", "zzz.txt"],
            reports: &["notes/zulu.txt", "zzz.txt"],
        },
        Resumed {
            case: "a bucket every key of which sorts before the position reports no entry",
            keys: &["aaa.txt", "notes/april.txt"],
            reports: &[],
        },
        Resumed {
            case: "a bucket every key of which sorts after the position reports all of them",
            keys: &["zulu.txt"],
            reports: &["zulu.txt"],
        },
        Resumed {
            case: "a bucket holding one key at the position reports exactly it",
            keys: &["notes/may.txt"],
            reports: &["notes/may.txt"],
        },
        // The two rows one byte either side of the position, so what the cursor names is a
        // position in the KEY ORDER rather than an offset into the bucket: a key that sorts
        // immediately past it is reported and one that sorts immediately before it is not.
        Resumed {
            case: "a bucket holding one key one byte past the position",
            keys: &["notes/may.txt.bak"],
            reports: &["notes/may.txt.bak"],
        },
        Resumed {
            case: "a bucket holding one key one byte before the position",
            keys: &["notes/may.tx"],
            reports: &[],
        },
        Resumed {
            case: "a bucket holding nothing at all reports no entry",
            keys: &[],
            reports: &[],
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let name = format!("token-position-addressed-{index}");
        bucket(&router, &name).await;
        store_keys(&router, &name, row.keys).await;
        let resumed = list_objects_v2(
            &router,
            &name,
            &format!("&continuation-token={}", handed_back_in_query(&token)),
        )
        .await;
        assert_eq!(
            resumed.status,
            200,
            "case {:?}: a token this service issued resumes the addressed bucket, got {} with \
             body {:?}",
            row.case,
            resumed.status,
            text(&resumed)
        );
        let listing = listing_of(&resumed);
        assert_eq!(
            listing.keys(),
            row.reports
                .iter()
                .map(|key| (*key).to_string())
                .collect::<Vec<String>>(),
            "case {:?}: expected the resumed page to carry {:?}, got {:?}",
            row.case,
            row.reports,
            listing.keys()
        );
        // The resumed page is the ADDRESSED bucket's own, and it echoes the token it was
        // given.
        assert_eq!(
            (
                element(&resumed, "Name"),
                element(&resumed, "ContinuationToken")
            ),
            (Some(name.clone()), Some(token.clone())),
            "case {:?}: expected the page of {name:?} resumed from the token it was given, got \
             {:?}",
            row.case,
            text(&resumed)
        );
        // "KeyCount is the number of keys returned with this request", and the page carries
        // every key of this bucket at or after the position, so nothing is left for a
        // further one.
        assert_eq!(
            (
                listing.field("KeyCount"),
                listing.field("IsTruncated"),
                listing.field("NextContinuationToken")
            ),
            (
                row.reports.len().to_string(),
                "false".to_string(),
                String::new()
            ),
            "case {:?}: expected an untruncated page of {} key(s) handing back no token, got \
             {:?}",
            row.case,
            row.reports.len(),
            text(&resumed)
        );
    }

    // The CONTROL: only a value this service minted for an object listing names a position,
    // so what a caller could assemble for itself is still refused naming the parameter it
    // arrived on — on the issuing bucket and on another one alike.
    let mut forged = computable_spellings(&position);
    forged.push((
        "a token with its last character changed".to_string(),
        flip_first(&token),
    ));
    forged.push((
        "not an encoding at all".to_string(),
        "not.a.token".to_string(),
    ));
    for (why, value) in forged {
        for addressed in [issuer, "token-position-addressed-0"] {
            let refused = list_objects_v2(
                &router,
                addressed,
                &format!("&continuation-token={}", handed_back_in_query(&value)),
            )
            .await;
            assert_token_refused(&format!("{addressed}/{why}"), &refused, true);
        }
    }
}

/// RULE, carried across PAGES and in BOTH directions: a token one bucket's listing issued
/// keeps walking the addressed bucket page by page — each page of that bucket hands back its
/// own token, which continues the SAME bucket — and a token the second bucket issued walks
/// the first just the same, so neither bucket owns the tokens its listings minted.
#[tokio::test]
async fn an_issued_token_walks_the_addressed_bucket_page_by_page_either_way() {
    let router = router();
    let issuer = "token-walk-issuer";
    bucket(&router, issuer).await;
    store_keys(&router, issuer, TOKEN_POSITION_KEYS).await;
    // The addressed bucket holds keys on BOTH sides of the issued position and more of them
    // than one page carries, so the walk has something to report and something to leave.
    let addressed = "token-walk-addressed";
    let addressed_keys: &[&str] = &["aaa.txt", "notes/may.txt", "notes/zulu.txt", "zzz.txt"];
    bucket(&router, addressed).await;
    store_keys(&router, addressed, addressed_keys).await;

    let first = list_objects_v2(&router, issuer, "&max-keys=1").await;
    let token = element(&first, "NextContinuationToken").unwrap_or_else(|| {
        panic!(
            "case \"the issuing bucket\": a truncated page hands back a token, got {:?}",
            text(&first)
        )
    });

    // One key per page, starting from the token the OTHER bucket's listing issued: the walk
    // reports the addressed bucket's keys at or after that position, in order.
    let mut walked: Vec<String> = Vec::new();
    let mut carried = token.clone();
    for step in 0..addressed_keys.len() {
        let case = format!("step {step} of the walk");
        let answer = list_objects_v2(
            &router,
            addressed,
            &format!(
                "&max-keys=1&continuation-token={}",
                handed_back_in_query(&carried)
            ),
        )
        .await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: each page of the walk is served, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        let page = listing_of(&answer);
        walked.extend(page.keys());
        let next = page.field("NextContinuationToken");
        assert_eq!(
            page.field("IsTruncated"),
            match next.is_empty() {
                true => "false",
                false => "true",
            },
            "case {case:?}: a page is truncated exactly when it hands back a token, got {:?}",
            text(&answer)
        );
        if next.is_empty() {
            break;
        }
        carried = next;
    }
    assert_eq!(
        walked,
        vec![
            "notes/may.txt".to_string(),
            "notes/zulu.txt".to_string(),
            "zzz.txt".to_string(),
        ],
        "the walk from the other bucket's token must report the addressed bucket's keys at or \
         after the issued position, got {walked:?}"
    );

    // And back: a token the ADDRESSED bucket's own listing issued names a position in the
    // ISSUING bucket, which is where its listing resumes.
    let from_addressed = element(
        &list_objects_v2(&router, addressed, "&max-keys=1").await,
        "NextContinuationToken",
    )
    .unwrap_or_else(|| panic!("the addressed bucket's bounded page hands back a token"));
    let back = list_objects_v2(
        &router,
        issuer,
        &format!(
            "&continuation-token={}",
            handed_back_in_query(&from_addressed)
        ),
    )
    .await;
    assert_eq!(
        back.status,
        200,
        "the other bucket's token must resume this listing, got {} with body {:?}",
        back.status,
        text(&back)
    );
    // That token names "notes/may.txt" — the addressed bucket's second key — so the issuing
    // bucket resumes at its own keys from there on.
    assert_eq!(
        listing_of(&back).keys(),
        vec!["notes/may.txt".to_string(), "plain.txt".to_string()],
        "expected the issuing bucket's keys at or after the position, got {:?}",
        listing_of(&back).keys()
    );
}
