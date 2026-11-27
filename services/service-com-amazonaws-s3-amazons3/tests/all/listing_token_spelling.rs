// SPDX-License-Identifier: CC0-1.0

//! The ALPHABET each paging listing of this service writes its continuation token in.
//!
//! The rule under test: a continuation token is an opaque cursor the service mints and the
//! caller hands back unread — "the continuation token is an opaque value that Amazon S3
//! understands" (`ListBucketMetricsConfigurationsOutput$NextContinuationToken`),
//! "ContinuationToken is obfuscated and is not a real bucket"
//! (`ListBucketsOutput$ContinuationToken`), "NextContinuationToken is obfuscated and is not
//! a real key" (`ListObjectsV2Output$NextContinuationToken`) — written in the alphabet AWS
//! writes THAT listing's own tokens in: the account's bucket listing and the listings of a
//! bucket's id-addressed configurations hand back a lowercase-hex value, the listings of what
//! a bucket holds hand back a value in base64's URL AND FILENAME SAFE alphabet (RFC 4648 §5),
//! unpadded — the base64 those listings' recorded tokens are written in, drawn from the
//! characters a query component carries unchanged, because a caller hands the value back on one
//! ([`super::continuation_token_query_safety`]). Whichever
//! alphabet it is, the value is a cursor the service reads back and no caller can mint: the
//! page it names is served, a value one character away from it is refused, and a token one
//! listing issued names no page of another's.
//!
//! The rows span the two alphabets and three of the listings that page — the account's
//! buckets, one bucket's metrics configurations, and one bucket's objects — so the rule is
//! read per listing rather than as one service-wide spelling.
//!
//! Every call goes over the wire through the harnesses the other bucket suites own, so what
//! is pinned is the wire document.

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, Answer};
use super::bucket_metrics_configuration_listing::{
    list_metrics, listed_ids, stand_up, CONFIGURATIONS_PER_PAGE,
};
use super::continuation_token_alphabet::handed_back_in_query;
use super::list_buckets::list_buckets;
use super::object_and_tagging_roundtrip::text;
use super::object_storage::{list_objects_v2, listing_of, store_keys};

/// The alphabet a listing's token is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Alphabet {
    /// Lowercase hexadecimal.
    Hex,
    /// Base64 in the URL and filename safe alphabet, unpadded (RFC 4648 §5), `A-Za-z0-9-_`.
    Base64,
}

impl Alphabet {
    /// Whether `token` is written in this alphabet.
    fn holds(self, token: &str) -> bool {
        !token.is_empty()
            && match self {
                Self::Hex => token
                    .chars()
                    .all(|character| matches!(character, '0'..='9' | 'a'..='f')),
                Self::Base64 => token.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
                }),
            }
    }
}

/// Assert `token` is written in `alphabet` and is no value a caller could compute from the
/// position it resumes at.
fn assert_written_in(case: &str, token: &str, alphabet: Alphabet, position: &str) {
    assert!(
        alphabet.holds(token),
        "case {case:?}: expected a token written in {alphabet:?}, got {token:?}"
    );
    for (rendering, computed) in [
        ("the position itself", position.to_string()),
        ("hex of the position", hex::encode(position)),
        (
            "base64 of the position",
            aws_smithy_types::base64::encode(position),
        ),
    ] {
        assert_ne!(
            token, computed,
            "case {case:?}: the token must not be {rendering}, got {token:?}"
        );
    }
}

/// `token` with its last character replaced by another of the same alphabet — a value one
/// character away from the one a listing issued, which only the authenticator inside it can
/// refuse.
///
/// Never a padding character: neither alphabet this service writes a token in carries any, so
/// the change always lands on a character the value is free in.
fn one_character_away(token: &str, alphabet: Alphabet) -> String {
    let mut characters: Vec<char> = token.chars().collect();
    let at = characters
        .iter()
        .rposition(|character| *character != '=')
        .unwrap_or(0);
    if let Some(character) = characters.get_mut(at) {
        *character = match (alphabet, *character) {
            (Alphabet::Hex, '0') => '1',
            (Alphabet::Hex, _) => '0',
            (Alphabet::Base64, 'A') => 'B',
            (Alphabet::Base64, _) => 'A',
        };
    }
    characters.into_iter().collect()
}

/// Stand up three buckets and answer the bucket listing's first page, bounded to one bucket.
async fn bucket_listing_page(router: &MultiServiceRouter, names: &[&str]) -> Answer {
    for name in names {
        let created = create_bucket(router, name, None, None).await;
        assert_eq!(
            created.status, 200,
            "the bucket {name:?} must be created, got {} with body {:?}",
            created.status,
            text(&created)
        );
    }
    list_buckets(router, "&max-buckets=1").await
}

/// The `<ContinuationToken>` of a bucket-listing page.
fn bucket_token(case: &str, answer: &Answer) -> String {
    answer
        .element("ContinuationToken")
        .unwrap_or_else(|| panic!("case {case:?}: no ContinuationToken in {:?}", text(answer)))
}

/// RULE: the account's own bucket listing writes its cursor in lowercase hex, and that value
/// resumes the listing where the page stopped.
#[tokio::test]
async fn the_bucket_listing_writes_its_cursor_in_lowercase_hex() {
    let router = router();
    let case = "the account's buckets, one per page";
    let names = ["spelling-a", "spelling-b", "spelling-c"];
    let first = bucket_listing_page(&router, &names).await;
    assert_eq!(
        first.status,
        200,
        "case {case:?}: the first page is served, got {} with body {:?}",
        first.status,
        text(&first)
    );
    assert_eq!(
        first.elements("Name"),
        vec![names[0].to_string()],
        "case {case:?}: the first page reports the first bucket, got {:?}",
        first.elements("Name")
    );

    let token = bucket_token(case, &first);
    // The cursor names a point in the listing's name order — the bucket this page reported
    // last, so the next page begins after it — and is no rendering of the name on either
    // side of that point.
    assert_written_in(case, &token, Alphabet::Hex, names[0]);
    assert_written_in(case, &token, Alphabet::Hex, names[1]);

    let resumed = list_buckets(
        &router,
        &format!(
            "&max-buckets=1&continuation-token={}",
            handed_back_in_query(&token)
        ),
    )
    .await;
    assert_eq!(
        resumed.status,
        200,
        "case {case:?}: the token this listing issued resumes it, got {} with body {:?}",
        resumed.status,
        text(&resumed)
    );
    assert_eq!(
        resumed.elements("Name"),
        vec![names[1].to_string()],
        "case {case:?}: the resumed page reports the bucket the first page left, got {:?}",
        resumed.elements("Name")
    );

    // A value one character away from it is a value this listing never issued.
    let forged = one_character_away(&token, Alphabet::Hex);
    let refused = list_buckets(
        &router,
        &format!(
            "&max-buckets=1&continuation-token={}",
            handed_back_in_query(&forged)
        ),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case {case:?}: a value no listing issued is refused, got {} with body {:?}",
        refused.status,
        text(&refused)
    );
}

/// RULE, on a bucket's id-addressed configurations: the same alphabet, read through the
/// listing that pages at the published page size.
#[tokio::test]
async fn a_configuration_listing_writes_its_cursor_in_lowercase_hex() {
    let router = router();
    let case = "a bucket past one page of metrics configurations";
    let name = "spelling-metrics";
    let configurations: Vec<(String, Option<_>)> = (0..=CONFIGURATIONS_PER_PAGE)
        // Fixed-width ids, so ascending id order is also ascending numeric order.
        .map(|index| (format!("cfg-{index:04}"), None))
        .collect();
    stand_up(&router, case, name, &configurations).await;

    let first = list_metrics(&router, name, None, &[]).await;
    let token = first.element("NextContinuationToken").unwrap_or_else(|| {
        panic!(
            "case {case:?}: a truncated listing hands back a cursor, got {:?}",
            text(&first)
        )
    });
    // The id the cursor resumes at: the first one the page did not report.
    let resumed_at = configurations[CONFIGURATIONS_PER_PAGE].0.clone();
    assert_written_in(case, &token, Alphabet::Hex, &resumed_at);

    let second = list_metrics(&router, name, Some(&token), &[]).await;
    assert_eq!(
        listed_ids(&second),
        vec![resumed_at.clone()],
        "case {case:?}: expected the listing to resume at {resumed_at:?}, got {:?}",
        text(&second)
    );

    let forged = one_character_away(&token, Alphabet::Hex);
    let refused = list_metrics(&router, name, Some(&forged), &[]).await;
    assert_eq!(
        refused.status, 400,
        "case {case:?}: a value no listing issued is refused, got {}",
        refused.status
    );
}

/// RULE, on the other alphabet: a bucket's object listing writes its cursor in URL-safe
/// base64, and that value resumes the listing where the page stopped.
#[tokio::test]
async fn an_object_listing_writes_its_cursor_in_url_safe_base64() {
    let router = router();
    let case = "one bucket's objects, one per page";
    let name = "spelling-objects";
    create_bucket(&router, name, None, None).await;
    let keys = ["alpha.txt", "beta.txt"];
    store_keys(&router, name, &keys).await;

    let first = list_objects_v2(&router, name, "&max-keys=1").await;
    let token = listing_of(&first).field("NextContinuationToken");
    assert_written_in(case, &token, Alphabet::Base64, keys[1]);

    let resumed = list_objects_v2(
        &router,
        name,
        &format!("&continuation-token={}", handed_back_in_query(&token)),
    )
    .await;
    assert_eq!(
        listing_of(&resumed).keys(),
        vec![keys[1].to_string()],
        "case {case:?}: the resumed page reports the key the first page left, got {:?}",
        listing_of(&resumed).keys()
    );

    let forged = one_character_away(&token, Alphabet::Base64);
    let refused = list_objects_v2(
        &router,
        name,
        &format!("&continuation-token={}", handed_back_in_query(&forged)),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case {case:?}: a value no listing issued is refused, got {} with body {:?}",
        refused.status,
        text(&refused)
    );
}

/// RULE, across the listings: a token one listing issued names no page of another, whichever
/// alphabet the two are written in.
#[tokio::test]
async fn a_token_of_one_listing_names_no_page_of_another() {
    let router = router();
    let names = ["crossing-a", "crossing-b", "crossing-c"];
    let buckets_page = bucket_listing_page(&router, &names).await;
    let bucket_cursor = bucket_token("the account's buckets", &buckets_page);

    let holder = "crossing-objects";
    create_bucket(&router, holder, None, None).await;
    store_keys(&router, holder, &["alpha.txt", "beta.txt"]).await;
    let objects_page = list_objects_v2(&router, holder, "&max-keys=1").await;
    let object_cursor = listing_of(&objects_page).field("NextContinuationToken");

    // The bucket listing's cursor handed to the object listing, and the object listing's
    // cursor handed to the bucket listing: each is a value the addressed listing never
    // issued, so neither is read as a position in it.
    let refused = list_objects_v2(
        &router,
        holder,
        &format!("&continuation-token={}", handed_back_in_query(&bucket_cursor)),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case \"a bucket cursor in the object listing\": expected 400, got {} with body {:?}",
        refused.status,
        text(&refused)
    );

    let refused = list_buckets(
        &router,
        &format!(
            "&max-buckets=1&continuation-token={}",
            handed_back_in_query(&object_cursor)
        ),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case \"an object cursor in the bucket listing\": expected 400, got {} with body {:?}",
        refused.status,
        text(&refused)
    );
}
