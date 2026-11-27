// SPDX-License-Identifier: CC0-1.0

//! The continuation token a paged listing hands back, as it travels BACK on the wire.
//!
//! RULE under test: a `ContinuationToken` / `NextContinuationToken` is an opaque value the
//! service mints for the caller to hand back on the `continuation-token` query parameter —
//! "you can use this ContinuationToken for the pagination of the list results"
//! (`ListDirectoryBucketsRequest$ContinuationToken`), "an opaque value that Amazon S3
//! understands" (`ListBucketMetricsConfigurationsOutput$NextContinuationToken`) — so every
//! character of one is a character a URI query component carries unchanged (RFC 3986's
//! unreserved set): the value a client hands back is then the value it was given whether the
//! client percent-encodes it ([`handed_back_in_query`]) or returns it verbatim, and the one
//! spelling of it that reaches the wire is the one inside the signature the client signed the
//! hand-back with. Both alphabets this service writes a token in stay inside that set —
//! lowercase hex, and base64's URL and filename safe alphabet without padding — so the encoded
//! hand-back and the verbatim one are the SAME bytes for either.
//!
//! The rows span what the rule generalises over: WHICH paged listing minted the token, which
//! covers both alphabets this service writes a token in (`super::continuation_token_alphabet`
//! states which listing writes which) — the OBJECT listing, whose token is base64, and the
//! account's own bucket listing and its directory-bucket listing, whose tokens are lowercase
//! hex — and, per listing, the position the token resumes at.
//!
//! Each call goes over the wire through the harness the bucket and object suites own, so what
//! is pinned is the wire document.

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, send_bytes_signed, Answer};
use super::continuation_token_alphabet::handed_back_in_query;
use super::list_buckets::list_buckets;
use super::object_and_tagging_roundtrip::text;
use super::object_storage::{list_objects_v2, listing_of, store_keys};

/// The credential scope a client of the S3 EXPRESS control endpoint presents, which is what
/// tells that endpoint's `GET /` (the directory-bucket listing) from the general purpose
/// endpoint's.
const S3_EXPRESS_AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/s3express/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The host the S3 Express control endpoint is addressed as — "the HTTP Host header syntax is
/// `s3express-control.region.amazonaws.com`" (`ListDirectoryBuckets`).
const S3_EXPRESS_CONTROL_HOST: &str = "s3express-control.us-east-1.amazonaws.com";

/// The zone the directory buckets below are placed in.
const ZONE_ID: &str = "use1-az4";

/// `GET /<query>` on the S3 Express control endpoint — `ListDirectoryBuckets`.
async fn list_directory_buckets(router: &MultiServiceRouter, query: &str) -> Answer {
    send_bytes_signed(
        router,
        S3_EXPRESS_AUTHORIZATION,
        "GET",
        S3_EXPRESS_CONTROL_HOST,
        &format!("/{query}"),
        &[],
        b"",
    )
    .await
}

/// Whether every character of `token` is one a URI query component carries unchanged: RFC
/// 3986's unreserved set, `A-Za-z0-9` with `-`, `.`, `_` and `~`.
///
/// `pub(crate)`: the suite that holds the DIRECTORY-bucket listing's token to that set
/// ([`super::directory_bucket_listing_token_alphabet`]) reads the rule here rather than
/// keeping a second copy of it.
pub(crate) fn is_unreserved(token: &str) -> bool {
    !token.is_empty()
        && token.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '.' | '_' | '~')
        })
}

/// Whether `token` is written in one of the two alphabets this service writes a token in:
/// lowercase hex, or base64's URL and filename safe alphabet without padding.
fn is_a_published_alphabet(token: &str) -> bool {
    !token.is_empty()
        && token
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

/// Assert the rule about the token ITSELF: it is written in one of the two alphabets this
/// service publishes, both of which are inside the characters a query component carries
/// unchanged — so the value a client hands back is the value it was given, percent-encoded or
/// verbatim.
fn assert_a_published_alphabet(case: &str, token: &str) {
    assert!(
        is_a_published_alphabet(token),
        "case {case:?}: a continuation token is written in one of the two published alphabets, \
         got {token:?}"
    );
    assert_travels_unchanged(case, token);
}

/// Assert the rule that holds for a token of EITHER alphabet: every character of it is one a
/// query component carries unchanged, so percent-encoding it as a client encodes a query value
/// leaves the value it hands back identical to the one it was given.
fn assert_travels_unchanged(case: &str, token: &str) {
    assert!(
        is_unreserved(token),
        "case {case:?}: a continuation token is written in the characters a query component \
         carries unchanged, got {token:?}"
    );
    let encoded = handed_back_in_query(token);
    assert_eq!(
        encoded, token,
        "case {case:?}: the encoded hand-back and the verbatim hand-back must be the same \
         bytes, got {encoded:?} for {token:?}"
    );
}

/// RULE on the DIRECTORY-bucket listing: its token is written in a published alphabet and
/// resumes the account's listing where the page stopped, handed back the way a client puts a
/// query value on the wire. Which alphabet that is, and that either hand-back of it resumes
/// the same page, is `super::directory_bucket_listing_token_alphabet`'s rule.
#[tokio::test]
async fn the_directory_bucket_listing_token_travels_unchanged_and_resumes_the_listing() {
    let router = router();
    // Two buckets, so one page of one bucket leaves a second page to resume; the names are
    // directory-bucket names (`<base>--<az-id>--x-s3`) and are created as such.
    let names = [
        format!("tokensafe-a--{ZONE_ID}--x-s3"),
        format!("tokensafe-b--{ZONE_ID}--x-s3"),
    ];
    for name in &names {
        super::directory_bucket_sessions_and_appends::directory_bucket(&router, name, ZONE_ID)
            .await;
    }

    // One bucket per page, so each page hands back the cursor for the next.
    let mut resumed_names: Vec<String> = Vec::new();
    let mut query = "?max-directory-buckets=1".to_string();
    for page_number in 1..=names.len() {
        let case = format!("page {page_number} of the directory-bucket listing");
        let page = list_directory_buckets(&router, &query).await;
        assert_eq!(
            page.status,
            200,
            "case {case:?}: expected HTTP 200, got {} with body {:?}",
            page.status,
            text(&page)
        );
        resumed_names.extend(page.elements("Name"));
        let token = page
            .element("ContinuationToken")
            .filter(|token| !token.is_empty());
        match token {
            Some(token) => {
                assert_a_published_alphabet(&case, &token);
                // Handed back the way a client puts a query value on the wire: the characters
                // a query component reads otherwise percent-encoded, everything else as it was
                // given.
                query = format!(
                    "?max-directory-buckets=1&continuation-token={}",
                    handed_back_in_query(&token)
                );
            }
            None => {
                assert_eq!(
                    page_number,
                    names.len(),
                    "case {case:?}: only the last page owes no cursor, got body {:?}",
                    text(&page)
                );
            }
        }
    }
    assert_eq!(
        resumed_names, names,
        "paging one bucket at a time must report every directory bucket exactly once, got \
         {resumed_names:?}"
    );
}

/// RULE on the OBJECT listing, whose token is written in the same alphabet: the value handed
/// back the way a client puts a query value on the wire resumes the listing.
#[tokio::test]
async fn the_object_listing_token_travels_unchanged_and_resumes_the_listing() {
    let router = router();
    let bucket_name = "token-query-safety-objects";
    create_bucket(&router, bucket_name, None, None).await;
    // Keys of three lengths, so the token is minted at each of base64's alignment classes and
    // the rule is not read back at one alignment alone.
    let keys = ["a", "beta.txt", "media/2024/april.jpg"];
    store_keys(&router, bucket_name, &keys).await;

    let mut listed: Vec<String> = Vec::new();
    let mut query = "&max-keys=1".to_string();
    for page_number in 1..=keys.len() {
        let case = format!("page {page_number} of the object listing");
        let page = list_objects_v2(&router, bucket_name, &query).await;
        assert_eq!(
            page.status,
            200,
            "case {case:?}: expected HTTP 200, got {} with body {:?}",
            page.status,
            text(&page)
        );
        let listing = listing_of(&page);
        listed.extend(listing.keys());
        let token = listing.field("NextContinuationToken");
        if token.is_empty() {
            assert_eq!(
                page_number,
                keys.len(),
                "case {case:?}: only the last page owes no cursor, got body {:?}",
                text(&page)
            );
            continue;
        }
        assert_a_published_alphabet(&case, &token);
        query = format!(
            "&max-keys=1&continuation-token={}",
            handed_back_in_query(&token)
        );
    }
    let mut expected: Vec<String> = keys.iter().map(|key| (*key).to_string()).collect();
    expected.sort();
    assert_eq!(
        listed, expected,
        "paging one key at a time must report every key exactly once, in order, got {listed:?}"
    );
}

/// RULE on the listing whose token is written in the OTHER alphabet: the account's own bucket
/// listing hands back a hex token, which is inside RFC 3986's unreserved set — so the encoded
/// hand-back and the verbatim one are the same bytes, and both resume the listing.
#[tokio::test]
async fn the_bucket_listing_token_travels_unchanged_and_resumes_the_account() {
    let router = router();
    let names = ["token-safety-a", "token-safety-b"];
    for name in names {
        create_bucket(&router, name, None, None).await;
    }

    let case = "the account's buckets, one per page";
    let first = list_buckets(&router, "&max-buckets=1").await;
    assert_eq!(
        first.status,
        200,
        "case {case:?}: expected HTTP 200, got {} with body {:?}",
        first.status,
        text(&first)
    );
    let token = first
        .element("ContinuationToken")
        .filter(|token| !token.is_empty())
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: a truncated page hands back a cursor, got body {:?}",
                text(&first)
            )
        });
    assert_travels_unchanged(case, &token);

    // Handed back VERBATIM: a hex token needs no encoding at all.
    let resumed = list_buckets(&router, &format!("&max-buckets=1&continuation-token={token}")).await;
    assert_eq!(
        resumed.status,
        200,
        "case {case:?}: the value the listing handed back resumes it, got {} with body {:?}",
        resumed.status,
        text(&resumed)
    );
    assert_eq!(
        resumed.elements("Name"),
        vec![names[1].to_string()],
        "case {case:?}: the resumed page reports the bucket the first page left, got {:?}",
        resumed.elements("Name")
    );
}
