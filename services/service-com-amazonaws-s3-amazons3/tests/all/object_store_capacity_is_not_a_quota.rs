// SPDX-License-Identifier: CC0-1.0

//! What the object store ADMITS: every write whose size S3 accepts, however much is already
//! stored.
//!
//! Every assertion drives a real HTTP request through the router (the harness is the one
//! [`super::bucket_lifecycle`] and [`super::object_and_tagging_roundtrip`] own), so what is
//! pinned is the wire behaviour a client doing ordinary object traffic sees.
//!
//! The rule under test, asserted over a SET of inputs rather than one request:
//!
//! > S3 bounds neither how many objects a bucket holds, nor how many versions one key keeps,
//! > nor how many bytes either of them or a whole account carries; it bounds only the size of
//! > ONE payload. So an object write within that size is SERVED — HTTP 200 with the entity tag
//! > of the bytes it stored — whatever the bucket, the key or the account already holds, and
//! > it is refused only by an error the operation models.
//!
//! Its sources: "you can store any number of objects in a bucket" and "the total volume of
//! data and number of objects you can store in a bucket are unlimited" (Amazon S3 User Guide,
//! *Buckets overview*); the only published size rules are on one payload — 5 GiB for a
//! `PutObject` body and "Part size: 5 MiB to 5 GiB" (*Amazon S3 multipart upload limits*);
//! `PutObject`'s modelled errors are `EncryptionTypeMismatch`, `InvalidRequest`,
//! `InvalidWriteOffset` and `TooManyParts`, none of them about capacity, and `UploadPart`
//! declares none at all (`smithy_query(query="operation",operation="PutObject,UploadPart")`);
//! and the 1,000 figure the list surface carries is a PAGE SIZE, which is why the operation is
//! paginated at all — "by default, the action returns up to 1,000 key names" with
//! `IsTruncated`, `KeyCount` and `NextContinuationToken` to walk the rest
//! (`ListObjectsV2Output$MaxKeys`, and `ListObjectVersions` the same over versions).
//!
//! The rows differ along what the rule generalises over: how many bytes one bucket holds and
//! in how many objects, how many distinct KEYS one bucket holds, how many VERSIONS one key
//! keeps, and how many buckets the bytes are spread over.

use std::collections::BTreeSet;

use service_com_amazonaws_s3_amazons3::scaffolded::Scaffolded;
use service_com_amazonaws_s3_amazons3::service::object_write::store_full;

use super::bucket_lifecycle::router;
use super::continuation_token_alphabet::handed_back_in_query;
use super::object_and_tagging_roundtrip::{bucket, expected_etag, get_object, put_object, text};
use super::object_partial_reads::seeded_payload;
use super::object_storage::{list_objects_v2, listing_of, Listing};
use super::object_versioning::{list_object_versions, set_versioning, version_listing_of};

/// The `BucketVersioningStatus` that makes every write to a key mint a version of its own
/// ("Enabled—Enables versioning for the objects in the bucket. All objects added to the
/// bucket receive a unique version ID", `PutBucketVersioning`).
const VERSIONING_ENABLED: &str = "Enabled";

/// The default page size of the S3 list surface: "by default, the action returns up to 1,000
/// key names" (`ListObjectsV2Output$MaxKeys`). A page size, NOT a storage limit, which is the
/// whole point of the rows below.
const DEFAULT_PAGE_KEYS: usize = 1_000;

/// One row of the bytes-in-one-bucket table: the case, how many objects the bucket is filled
/// with, and how many bytes each of them carries.
struct BytesRow {
    case: &'static str,
    objects: usize,
    object_bytes: usize,
}

#[test]
fn the_one_remaining_guard_refuses_in_s3s_own_vocabulary() {
    // The other half of the rule: ONE guard is left, and it is the emulator's own memory
    // rather than anything the API announces — so when it is met the answer has to be an S3
    // error document, not a code S3's vocabulary does not contain. No request can meet it
    // here (it takes a gibibyte of retained bytes, or a quarter of a million stored versions,
    // which a test must not allocate), so what is pinned is the ENVELOPE, read at the one
    // place every write of this service builds it.
    //
    // The triple is the published one: "ServiceUnavailable — Service is unable to handle
    // request. HTTP Status Code: 503 Service Unavailable" (Amazon S3 "Error responses", list
    // of error codes).
    let refusal: Scaffolded<()> = store_full();
    let Scaffolded::Unmodeled(unmodeled) = refusal else {
        panic!(
            "case \"the store-full refusal\": it must go out on the channel that carries an S3 \
             <Code>, so a client can branch on it"
        );
    };
    assert_eq!(
        unmodeled.code, "ServiceUnavailable",
        "case \"the store-full refusal\": expected the code S3 publishes for a request it \
         cannot handle, got {:?}",
        unmodeled.code
    );
    assert_eq!(
        unmodeled.status, 503,
        "case \"the store-full refusal\": expected HTTP 503, got {}",
        unmodeled.status
    );
    assert_eq!(
        unmodeled.message, "Service is unable to handle request.",
        "case \"the store-full refusal\": expected that code's published sentence, got {:?}",
        unmodeled.message
    );
}

#[tokio::test]
async fn one_bucket_holds_many_mib_of_objects_and_reads_every_one_back() {
    // Each row fills ONE bucket well past any figure a capacity ceiling could be set at, in a
    // different shape: many medium objects, a few large ones, and more smaller ones. A router
    // per row, so a row is judged on what IT wrote.
    let rows = [
        BytesRow {
            case: "five objects of 13 MiB",
            objects: 5,
            object_bytes: 13 * 1024 * 1024,
        },
        BytesRow {
            case: "two objects of 34 MiB",
            objects: 2,
            object_bytes: 34 * 1024 * 1024,
        },
        BytesRow {
            case: "nine objects of 8 MiB",
            objects: 9,
            object_bytes: 8 * 1024 * 1024,
        },
    ];

    for row in rows {
        let router = router();
        let name = "capacity-one-bucket";
        bucket(&router, name).await;
        let held = row.objects * row.object_bytes;

        for index in 0..row.objects {
            let body = seeded_payload(0x1000_0000 + index as u32, row.object_bytes);
            let key = format!("blob/{index}.bin");
            let stored = put_object(&router, name, &key, &body, &[]).await;
            assert_eq!(
                stored.status, 200,
                "case {:?}: writing {key:?} ({} bytes) into a bucket already holding {} must \
                 be served, got {} with body {:?}",
                row.case,
                row.object_bytes,
                index * row.object_bytes,
                stored.status,
                text(&stored)
            );
            assert_eq!(
                stored.header("etag"),
                Some(expected_etag(&body)),
                "case {:?}: the write of {key:?} must report the entity tag of its own bytes, \
                 got {:?}",
                row.case,
                stored.header("etag")
            );

            let read = get_object(&router, name, &key, "", &[]).await;
            assert_eq!(
                read.status, 200,
                "case {:?}: {key:?} must read back, got {} with body {:?}",
                row.case,
                read.status,
                text(&read)
            );
            assert!(
                read.body.as_ref() == body.as_slice(),
                "case {:?}: {key:?} must read back byte-exact, got {} bytes",
                row.case,
                read.body.len()
            );
        }

        // And the listing reports every one of them, at the size each was written with, so
        // the bucket really holds what the writes claimed.
        let listed = listing_of(&list_objects_v2(&router, name, "").await);
        assert_eq!(
            listed.keys().len(),
            row.objects,
            "case {:?}: a bucket holding {held} bytes must list its {} objects, got {:?}",
            row.case,
            row.objects,
            listed.keys()
        );
        for entry in &listed.contents {
            assert_eq!(
                Listing::entry_field(entry, "Size"),
                row.object_bytes.to_string(),
                "case {:?}: every listed object must report {} bytes, got {:?} for {:?}",
                row.case,
                row.object_bytes,
                Listing::entry_field(entry, "Size"),
                Listing::entry_field(entry, "Key")
            );
        }
    }
}

#[tokio::test]
async fn a_bucket_larger_than_one_page_is_served_and_pages_through_the_listing() {
    // More distinct keys than the default page holds, written one request at a time: every
    // write is served, and the listing then does what it is paginated FOR — a full first page
    // marked truncated, with a token that returns the rest.
    let router = router();
    let name = "capacity-larger-than-a-page";
    bucket(&router, name).await;

    let keys: Vec<String> = (0..=DEFAULT_PAGE_KEYS)
        .map(|index| format!("obj/{index:04}"))
        .collect();
    for (index, key) in keys.iter().enumerate() {
        let stored = put_object(&router, name, key, b"x", &[]).await;
        assert_eq!(
            stored.status, 200,
            "case \"key {index} of {}\": the write of {key:?} must be served, got {} with \
             body {:?}",
            keys.len(),
            stored.status,
            text(&stored)
        );
    }

    let first = listing_of(&list_objects_v2(&router, name, "").await);
    assert_eq!(
        first.field("MaxKeys"),
        DEFAULT_PAGE_KEYS.to_string(),
        "case \"the first page\": the default page size must be reported, got {:?}",
        first.field("MaxKeys")
    );
    assert_eq!(
        first.field("KeyCount"),
        DEFAULT_PAGE_KEYS.to_string(),
        "case \"the first page\": a full page must be reported, got {:?}",
        first.field("KeyCount")
    );
    assert_eq!(
        first.field("IsTruncated"),
        "true",
        "case \"the first page\": a bucket larger than one page must be truncated, got {:?}",
        first.field("IsTruncated")
    );
    let token = first.field("NextContinuationToken");
    assert!(
        !token.is_empty(),
        "case \"the first page\": a truncated page must carry a continuation token, got {:?}",
        first.field("NextContinuationToken")
    );
    assert_eq!(
        first.keys(),
        &keys[..DEFAULT_PAGE_KEYS],
        "case \"the first page\": it must carry the first {DEFAULT_PAGE_KEYS} keys in order"
    );

    let second = listing_of(
        &list_objects_v2(
            &router,
            name,
            &format!("&continuation-token={}", handed_back_in_query(&token)),
        )
        .await,
    );
    assert_eq!(
        second.keys(),
        &keys[DEFAULT_PAGE_KEYS..],
        "case \"the second page\": the token must return the keys the first page left, got \
         {:?}",
        second.keys()
    );
    assert_eq!(
        second.field("IsTruncated"),
        "false",
        "case \"the second page\": the last page must not be truncated, got {:?}",
        second.field("IsTruncated")
    );
}

#[tokio::test]
async fn one_versioned_key_takes_more_versions_than_one_listing_page_holds() {
    // A versioned key's version count is unbounded, and a version is NOT another object: the
    // bucket holds one key however many times it is written. Every write is served with a
    // version id of its own, and the version listing pages them.
    let router = router();
    let name = "capacity-hot-versioned-key";
    bucket(&router, name).await;
    set_versioning(&router, name, VERSIONING_ENABLED).await;

    let key = "hot/key.txt";
    let writes = DEFAULT_PAGE_KEYS + 1;
    let mut minted: BTreeSet<String> = BTreeSet::new();
    for index in 0..writes {
        let stored = put_object(&router, name, key, format!("revision {index}").as_bytes(), &[])
            .await;
        assert_eq!(
            stored.status, 200,
            "case \"revision {index} of {writes}\": the write must be served, got {} with \
             body {:?}",
            stored.status,
            text(&stored)
        );
        let version_id = stored
            .header("x-amz-version-id")
            .unwrap_or_else(|| panic!("case \"revision {index}\": a versioned write must report \
                                       a version id, got none"));
        assert!(
            minted.insert(version_id.clone()),
            "case \"revision {index}\": every version id must be new, got {version_id:?} twice"
        );
    }
    assert_eq!(
        minted.len(),
        writes,
        "case \"the whole run\": {writes} writes must mint {writes} distinct version ids, got \
         {}",
        minted.len()
    );

    // The current version reads back as the newest write, so the versions behind it displaced
    // nothing.
    let read = get_object(&router, name, key, "", &[]).await;
    assert_eq!(
        read.body.as_ref(),
        format!("revision {}", writes - 1).as_bytes(),
        "case \"the current version\": it must be the newest write, got {:?}",
        text(&read)
    );

    // And the listing pages them: a full page, truncated, with markers that return the rest.
    let first = version_listing_of(&list_object_versions(&router, name, "").await);
    assert_eq!(
        first.listed().len(),
        DEFAULT_PAGE_KEYS,
        "case \"the first version page\": it must carry a full page, got {}",
        first.listed().len()
    );
    assert_eq!(
        first.field("IsTruncated"),
        "true",
        "case \"the first version page\": more versions than one page must be truncated, got \
         {:?}",
        first.field("IsTruncated")
    );
    let key_marker = first.field("NextKeyMarker");
    let version_marker = first.field("NextVersionIdMarker");
    assert_eq!(
        key_marker, key,
        "case \"the first version page\": the next key marker must name the key it stopped \
         inside, got {key_marker:?}"
    );
    assert!(
        !version_marker.is_empty(),
        "case \"the first version page\": a truncated page must carry a version id marker, \
         got {version_marker:?}"
    );

    let second = version_listing_of(
        &list_object_versions(
            &router,
            name,
            &format!(
                "&key-marker={}&version-id-marker={}",
                handed_back_in_query(&key_marker),
                handed_back_in_query(&version_marker)
            ),
        )
        .await,
    );
    assert_eq!(
        second.listed().len(),
        writes - DEFAULT_PAGE_KEYS,
        "case \"the second version page\": it must carry the versions the first page left, \
         got {}",
        second.listed().len()
    );
    let paged: BTreeSet<String> = first
        .entries()
        .into_iter()
        .chain(second.entries())
        .map(|(_, version_id)| version_id)
        .collect();
    assert_eq!(
        paged, minted,
        "case \"both version pages\": together they must name every version that was written"
    );
}

#[tokio::test]
async fn objects_spread_over_several_buckets_are_all_written_and_read_back() {
    // The same rule across buckets: what one bucket holds says nothing about what another may
    // take, and there is no account-wide total either — so bytes well past any single bucket's
    // worth, spread over several buckets, are all stored and all read back byte-exact.
    let router = router();
    let buckets = ["capacity-estate-a", "capacity-estate-b", "capacity-estate-c"];
    let object_bytes = 15 * 1024 * 1024;
    let keys = ["part-a.bin", "part-b.bin", "part-c.bin"];
    let mut written: Vec<(String, &str, Vec<u8>)> = Vec::new();

    for (bucket_index, name) in buckets.iter().enumerate() {
        bucket(&router, name).await;
        for (key_index, key) in keys.iter().enumerate() {
            let body = seeded_payload(
                0x2000_0000 + (bucket_index * keys.len() + key_index) as u32,
                object_bytes,
            );
            let stored = put_object(&router, name, key, &body, &[]).await;
            assert_eq!(
                stored.status, 200,
                "case \"{name}/{key}\": the write must be served whatever the other buckets \
                 hold, got {} with body {:?}",
                stored.status,
                text(&stored)
            );
            assert_eq!(
                stored.header("etag"),
                Some(expected_etag(&body)),
                "case \"{name}/{key}\": the write must report the entity tag of its own \
                 bytes, got {:?}",
                stored.header("etag")
            );
            written.push(((*name).to_string(), key, body));
        }
    }

    for (name, key, body) in &written {
        let read = get_object(&router, name, key, "", &[]).await;
        assert_eq!(
            read.status, 200,
            "case \"{name}/{key}\": it must read back, got {} with body {:?}",
            read.status,
            text(&read)
        );
        assert_eq!(
            read.header("content-length"),
            Some(object_bytes.to_string()),
            "case \"{name}/{key}\": it must report {object_bytes} bytes, got {:?}",
            read.header("content-length")
        );
        assert!(
            read.body.as_ref() == body.as_slice(),
            "case \"{name}/{key}\": it must read back byte-exact, got {} bytes",
            read.body.len()
        );
    }
}
