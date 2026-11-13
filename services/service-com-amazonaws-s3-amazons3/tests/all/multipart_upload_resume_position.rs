// SPDX-License-Identifier: CC0-1.0

//! The RESUME POSITION a `ListMultipartUploads` page reports, on every page it answers.
//!
//! RULE: every `ListMultipartUploads` answer reports the position a following page resumes
//! after — `NextKeyMarker` with the key of the last entry the page reported and
//! `NextUploadIdMarker` with that entry's upload id — whatever `IsTruncated` says, and both
//! empty when the page reported no entry at all. "Set the value of key-marker to the
//! NextKeyMarker value from the previous response. Similarly, set the value of
//! upload-id-marker to the NextUploadIdMarker value from the previous response" (this
//! operation's own documentation) is what the pair is FOR, and the id is what separates two
//! uploads of one key: "any multipart uploads for a key equal to the key-marker might be
//! included in the list only if they have an upload ID lexicographically greater than the
//! specified upload-id-marker" (`ListMultipartUploadsRequest$UploadIdMarker`). The pair sits
//! beside the `KeyMarker`/`UploadIdMarker` that say where the page BEGAN, which are part of
//! every answer too.
//!
//! A rolled-up `CommonPrefixes` entry is a key name and no upload, so it is a position with
//! no upload id: "CommonPrefixes is filtered out from results if it is not
//! lexicographically greater than the key-marker"
//! (`ListMultipartUploadsRequest$Delimiter`) is what resumes after one.
//!
//! The rows span what the rule generalises over rather than one recorded page: how many
//! uploads a bucket holds, how many of them share one key, the page size (one at a time,
//! sizes that do not divide the total, exactly the total, more than the total, and none at
//! all), whether the page is the truncated one or the one that ends the walk, whether it
//! reported anything, whether a `delimiter` rolled its last entry up, and which rendering
//! the request asked for. Nothing here is written down that the service mints: every
//! expected marker is the entry the SAME page reported.

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::router;
use super::multipart_upload_listing::{initiate, listing, UploadListing};
use super::object_and_tagging_roundtrip::bucket;
use super::object_listing_encoding::{decoded, in_path};

/// `(case, keys)`: the upload SETS every walk below is run over, each row differing in how
/// many uploads the bucket holds and how many of them share one object key — the case the
/// upload-id half of the position exists for.
const ROWS: &[(&str, &[&str])] = &[
    ("one upload of one key", &["alpha.bin"]),
    (
        "one upload per key, several keys",
        &["alpha.bin", "beta.bin", "gamma.bin"],
    ),
    (
        "two uploads of the same key",
        &["shared.bin", "shared.bin", "zeta.bin"],
    ),
    (
        "every upload of one key",
        &["shared.bin", "shared.bin", "shared.bin"],
    ),
];

/// The `(NextKeyMarker, NextUploadIdMarker)` a page reported.
fn position(page: &UploadListing) -> (String, String) {
    (
        page.field("NextKeyMarker"),
        page.field("NextUploadIdMarker"),
    )
}

/// The `key-marker`/`upload-id-marker` query a client resumes `position` with.
fn resuming(position: &(String, String)) -> String {
    format!(
        "&key-marker={}&upload-id-marker={}",
        in_path(&position.0),
        in_path(&position.1)
    )
}

/// Initiate one upload per entry of `keys` in `name`, in that order, and hand back
/// `(key, upload id)` per initiation.
async fn initiate_all(
    router: &MultiServiceRouter,
    name: &str,
    keys: &[&str],
) -> Vec<(String, String)> {
    let mut initiated = Vec::new();
    for key in keys {
        let upload_id = initiate(router, name, key, &[]).await;
        initiated.push(((*key).to_string(), upload_id));
    }
    initiated
}

#[tokio::test]
async fn every_page_reports_the_entry_it_ended_on_as_its_resume_position() {
    for (row, (case, keys)) in ROWS.iter().enumerate() {
        let router = router();
        let name = format!("mpu-position-{row}");
        bucket(&router, &name).await;
        let initiated = initiate_all(&router, &name, keys).await;
        // The order a listing reports: by key, then by initiation time within one key — a
        // STABLE sort by key of the initiation sequence, which is the order `initiated` is
        // in.
        let mut expected = initiated.clone();
        expected.sort_by(|left, right| left.0.cmp(&right.0));

        // The page sizes: one entry at a time, sizes that do not divide the total, exactly
        // the total, and more than the total — the last two being the pages that report
        // everything and are NOT truncated.
        for size in [1_usize, 2, 3, keys.len(), keys.len() + 1] {
            let case = format!("{case}, pages of {size}");
            let mut seen: Vec<(String, String)> = Vec::new();
            let mut query = format!("&max-uploads={size}");
            let mut last = (String::new(), String::new());
            // A page that made no progress would loop; this bound turns that into a failed
            // assertion instead.
            for _ in 0..=keys.len() {
                let page = listing(&router, &name, &case, &query).await;
                let entries = page.entries();
                seen.extend(entries.clone());

                let reported = position(&page);
                let ended_on = entries.last().cloned().unwrap_or_default();
                assert_eq!(
                    reported,
                    ended_on,
                    "case {case:?}: the position must name the last entry the page \
                     reported, got {reported:?} for entries {entries:?}",
                );
                last = reported.clone();
                if page.field("IsTruncated") != "true" {
                    break;
                }
                assert!(
                    !entries.is_empty(),
                    "case {case:?}: a truncated page must report the entry it resumes after"
                );
                query = format!("&max-uploads={size}{}", resuming(&reported));
            }
            assert_eq!(
                seen, expected,
                "case {case:?}: the walk must report every upload exactly once and in key \
                 order, got {seen:?}"
            );

            // The position the page that ENDED the walk handed back resumes after the last
            // upload there is: a client that sends it back is told there is nothing left,
            // rather than being handed the same page again.
            let after = listing(
                &router,
                &name,
                &case,
                &format!("&max-uploads={size}{}", resuming(&last)),
            )
            .await;
            assert!(
                after.entries().is_empty(),
                "case {case:?}: resuming after the last entry must report no upload, got {:?}",
                after.entries()
            );
            assert_eq!(
                position(&after),
                (String::new(), String::new()),
                "case {case:?}: a page that reported nothing hands back no position, got {:?}",
                position(&after)
            );
            assert_eq!(
                after.field("IsTruncated"),
                "false",
                "case {case:?}: a page with nothing left is not truncated, got {:?}",
                after.field("IsTruncated")
            );
        }
    }
}

#[tokio::test]
async fn a_page_that_reports_nothing_hands_back_an_empty_position() {
    let router = router();
    let name = "mpu-position-empty";
    bucket(&router, name).await;

    // A bucket holding no upload at all: the elements are there, empty, like the
    // `KeyMarker`/`UploadIdMarker` that say where the page began.
    let empty = listing(&router, name, "a bucket with no upload in progress", "").await;
    assert_eq!(
        position(&empty),
        (String::new(), String::new()),
        "case {:?}: expected an empty position, got {:?}",
        "a bucket with no upload in progress",
        position(&empty)
    );

    let initiated = initiate_all(&router, name, &["one.bin", "two.bin"]).await;

    // The pages that report nothing of a bucket that DOES hold uploads: a page bounded to
    // no entries, a key-marker past the last key, and a prefix no key begins with.
    let rows = [
        ("max-uploads=0", "&max-uploads=0".to_string()),
        (
            "a key-marker past the last key",
            format!("&key-marker={}", in_path("zzz-past-the-end.bin")),
        ),
        (
            "a prefix no key begins with",
            format!("&prefix={}", in_path("nothing-here/")),
        ),
    ];
    for (case, query) in rows {
        let page = listing(&router, name, case, &query).await;
        assert!(
            page.entries().is_empty(),
            "case {case:?}: expected no entry, got {:?}",
            page.entries()
        );
        assert_eq!(
            position(&page),
            (String::new(), String::new()),
            "case {case:?}: a page that reported nothing hands back no position, got {:?}",
            position(&page)
        );
    }

    // ...and the uploads are still there, so none of those reads moved anything.
    let again = listing(&router, name, "after the empty pages", "").await;
    assert_eq!(
        again.entries(),
        initiated,
        "case {:?}: expected both uploads still reported, got {:?}",
        "after the empty pages",
        again.entries()
    );
}

#[tokio::test]
async fn a_rolled_up_prefix_is_a_position_with_no_upload_id() {
    let router = router();
    let name = "mpu-position-rolled-up";
    bucket(&router, name).await;
    // Two keys under one rolled-up prefix and one key outside it, so the listing's first
    // entry is the prefix and its last is an upload.
    let initiated = initiate_all(
        &router,
        name,
        &["photos/first.bin", "photos/second.bin", "zeta.bin"],
    )
    .await;
    let outside = initiated
        .last()
        .cloned()
        .unwrap_or_else(|| panic!("the row must initiate an upload outside the prefix"));

    // One entry per page: the first page is the rolled-up prefix, which is a key name and
    // no upload, so it is a position with no upload id.
    let case = "a page that ends on a rolled-up prefix";
    let first = listing(&router, name, case, "&delimiter=%2F&max-uploads=1").await;
    assert!(
        first.entries().is_empty(),
        "case {case:?}: the uploads under the prefix roll into it, got {:?}",
        first.entries()
    );
    assert_eq!(
        position(&first),
        ("photos/".to_string(), String::new()),
        "case {case:?}: expected the prefix as the position and no upload id, got {:?}",
        position(&first)
    );
    assert_eq!(
        first.field("IsTruncated"),
        "true",
        "case {case:?}: an entry was left for the next page, got {:?}",
        first.field("IsTruncated")
    );

    // Resuming after it reports the upload outside the prefix and nothing of the prefix
    // again, and that page's position is that upload.
    let second = listing(
        &router,
        name,
        case,
        &format!(
            "&delimiter=%2F&max-uploads=1{}",
            resuming(&position(&first))
        ),
    )
    .await;
    assert_eq!(
        second.entries(),
        vec![outside.clone()],
        "case {case:?}: expected the upload outside the prefix, got {:?}",
        second.entries()
    );
    assert_eq!(
        position(&second),
        outside,
        "case {case:?}: expected that upload as the position, got {:?}",
        position(&second)
    );
}

#[tokio::test]
async fn the_position_of_an_encoded_page_is_written_the_way_its_entries_are() {
    let router = router();
    let name = "mpu-position-encoding";
    bucket(&router, name).await;
    // A key the encoder has something to escape in — a space, the escape character itself
    // and multi-byte UTF-8 — initiated last, so it is the entry the page ends on.
    let keys = ["a-plain.bin", "記録 100%.bin"];
    let initiated = initiate_all(&router, name, &keys).await;
    let (last_key, last_id) = initiated
        .last()
        .cloned()
        .unwrap_or_else(|| panic!("the row must initiate an upload"));

    // "returns encoded key name values in the following response elements: Delimiter,
    // KeyMarker, Prefix, NextKeyMarker, Key" (`ListMultipartUploadsOutput$EncodingType`) —
    // `NextUploadIdMarker` is not among them, an upload id being no key name.
    let case = "encoding-type=url";
    let page = listing(&router, name, case, "&encoding-type=url").await;
    let (written_key, written_id) = position(&page);
    assert_eq!(
        decoded(&written_key),
        last_key,
        "case {case:?}: decoding the position's key must yield the key the page ended on, \
         got {written_key:?}"
    );
    assert_ne!(
        written_key, last_key,
        "case {case:?}: the encoder has something to escape in {last_key:?}, so the position \
         must be written encoded, got {written_key:?}"
    );
    assert_eq!(
        written_id, last_id,
        "case {case:?}: an upload id is reported as minted, got {written_id:?}"
    );

    // The same page without the encoding reports the position verbatim.
    let verbatim = listing(&router, name, "no encoding type", "").await;
    assert_eq!(
        position(&verbatim),
        (last_key, last_id),
        "case {:?}: expected the position verbatim, got {:?}",
        "no encoding type",
        position(&verbatim)
    );
}
