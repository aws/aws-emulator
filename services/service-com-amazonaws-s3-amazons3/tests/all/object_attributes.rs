// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetObjectAttributes`: the root-level attributes of one stored
//! object, reported for exactly the fields the request asked for.
//!
//! The rules under test, with their sources:
//!
//! * "Specifies the fields at the root level that you want returned in the response. Fields
//!   that you do not specify are not returned"
//!   (`GetObjectAttributesRequest$ObjectAttributes`): the answer carries a field exactly
//!   when `x-amz-object-attributes` named it, and the values are the stored object's own —
//!   its entity tag, its size in bytes and its storage class.
//! * The `ETag` field carries the entity tag's own VALUE — the MD5 digest of the stored
//!   bytes, lowercase hex — and not the quoted-string rendering of the `ETag` response
//!   header, whose double quotes are that header's syntax ("An entity-tag consists of an
//!   opaque quoted string", RFC 9110 §8.8.3) rather than part of the tag.
//! * The `Checksum` container carries the checksum every stored object has — the algorithm
//!   the write named, or the default S3 adds to a write that names none
//!   (`PutObjectOutput$ChecksumCRC64NVME`) — reported exactly when it was asked for, while
//!   `ObjectParts` is not reported at all for an object a single `PutObject` wrote: it was
//!   assembled from no parts, which is also what S3 reports for such an object.
//! * `LastModified` and `VersionId` are bound HEADERS of the output, so they are reported
//!   for every answer rather than being asked for.
//! * The read is a reporting operation: a second read answers exactly what the first did,
//!   and nothing about the object changes.
//! * An address this emulator does not hold is refused with the code S3 sends for the part
//!   that is missing — the operation's own modeled `NoSuchKey` (404) for a key the bucket
//!   does not hold, `NoSuchBucket` for the bucket, and `NoSuchVersion` for a well-formed
//!   `versionId` the bucket holds no version of, where one that is not shaped like a
//!   version id at all is `InvalidArgument` (400).

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, send_bytes, Answer};
use super::object_and_tagging_roundtrip::{expected_etag, put_object, text};
use super::object_versioning::{head_object, set_versioning};

/// Every value the `ObjectAttributes` enum publishes
/// (`smithy_query(query="shape",shape="ObjectAttributes")`), which is what the header may
/// name.
const OBJECT_ATTRIBUTES_VALUES: [&str; 5] =
    ["ETag", "Checksum", "ObjectParts", "StorageClass", "ObjectSize"];

/// The entity tag this operation reports for `body`: the tag's own value, which is the
/// quoted tag of the `ETag` header ([`expected_etag`]) without that header's quotes.
fn expected_etag_value(body: &[u8]) -> String {
    expected_etag(body).trim_matches('"').to_string()
}

/// `GET /<bucket>/<key>?attributes<query>` — `GetObjectAttributes`, asking for `attributes`.
async fn get_object_attributes(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    query: &str,
    attributes: &[&str],
) -> Answer {
    send_bytes(
        router,
        "GET",
        &format!("/{bucket}/{key}?attributes{query}"),
        &[("x-amz-object-attributes", &attributes.join(", "))],
        b"",
    )
    .await
}

#[tokio::test]
async fn an_answer_carries_exactly_the_fields_the_request_asked_for() {
    let router = router();
    let bucket = "object-attributes";
    let created = create_bucket(&router, bucket, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket must be created first, got {} with body {:?}",
        created.status,
        text(&created)
    );

    // The objects differ along what the answer's values generalise over: the SIZE of the
    // body (empty, one byte, many bytes) and the STORAGE CLASS the write named (none,
    // which is the default, and two the enum publishes).
    let bodies: &[(&str, &[u8], Option<&str>, &str)] = &[
        ("an empty object", b"", None, "STANDARD"),
        ("a one-byte object", b"x", Some("STANDARD_IA"), "STANDARD_IA"),
        (
            "a many-byte object",
            b"attributes-subject-bytes-0123456789",
            Some("GLACIER_IR"),
            "GLACIER_IR",
        ),
    ];

    // Every subset of the three root-level fields this emulator can report, plus the two
    // containers alongside them: what is asked for decides what comes back.
    let asked: &[Vec<&str>] = &[
        vec!["ETag"],
        vec!["ObjectSize"],
        vec!["StorageClass"],
        vec!["ETag", "ObjectSize"],
        vec!["ObjectSize", "StorageClass"],
        vec!["ETag", "StorageClass"],
        vec!["ETag", "ObjectSize", "StorageClass"],
        vec!["StorageClass", "ObjectSize", "ETag"],
        vec!["Checksum", "ObjectParts"],
        OBJECT_ATTRIBUTES_VALUES.to_vec(),
    ];

    for (index, (case, body, storage_class, expected_class)) in bodies.iter().enumerate() {
        let key = format!("subject-{index}.bin");
        let headers: Vec<(&str, &str)> = storage_class
            .map(|class| vec![("x-amz-storage-class", class)])
            .unwrap_or_default();
        let stored = put_object(&router, bucket, &key, body, &headers).await;
        assert_eq!(
            stored.status, 200,
            "case {case:?}: the object must be stored first, got {} with body {:?}",
            stored.status,
            text(&stored)
        );

        for attributes in asked {
            let answer = get_object_attributes(&router, bucket, &key, "", attributes).await;
            let label = format!("{case} asking for {attributes:?}");
            assert_eq!(
                answer.status, 200,
                "case {label:?}: the read must be served, got {} with body {:?}",
                answer.status,
                text(&answer)
            );

            let etag = answer.element("ETag");
            assert_eq!(
                etag.as_deref(),
                attributes
                    .contains(&"ETag")
                    .then(|| expected_etag_value(body))
                    .as_deref(),
                "case {label:?}: the ETag is reported exactly when it was asked for, got \
                 {etag:?} in body {:?}",
                text(&answer)
            );
            let size = answer.element("ObjectSize");
            assert_eq!(
                size.as_deref(),
                attributes
                    .contains(&"ObjectSize")
                    .then(|| body.len().to_string())
                    .as_deref(),
                "case {label:?}: the size is reported exactly when it was asked for, got \
                 {size:?} in body {:?}",
                text(&answer)
            );
            let class = answer.element("StorageClass");
            assert_eq!(
                class.as_deref(),
                attributes
                    .contains(&"StorageClass")
                    .then_some(*expected_class),
                "case {label:?}: the storage class is reported exactly when it was asked \
                 for, got {class:?} in body {:?}",
                text(&answer)
            );
            // `ObjectParts` carries nothing for an object one write stored whole: it was
            // assembled from no parts. `Checksum` carries the one every stored object has —
            // the algorithm the write named, or the default S3 adds to a write that names
            // none (`PutObjectOutput$ChecksumCRC64NVME`) — reported exactly when it was
            // asked for.
            assert_eq!(
                answer.element("ObjectParts"),
                None,
                "case {label:?}: ObjectParts is not reported for an object stored by one \
                 write, got body {:?}",
                text(&answer)
            );
            let checksum = answer.element("Checksum");
            assert_eq!(
                checksum.is_some(),
                attributes.contains(&"Checksum"),
                "case {label:?}: the checksum is reported exactly when it was asked for, \
                 got {checksum:?} in body {:?}",
                text(&answer)
            );
            if let Some(checksum) = checksum {
                let expected = service_com_amazonaws_s3_amazons3::state::full_object_checksum(
                    "CRC64NVME", body,
                )
                .unwrap_or_default();
                assert_eq!(
                    checksum,
                    format!(
                        "<ChecksumCRC64NVME>{expected}</ChecksumCRC64NVME>\
                         <ChecksumType>FULL_OBJECT</ChecksumType>"
                    ),
                    "case {label:?}: the default checksum of the stored bytes must be \
                     reported, got {checksum:?}"
                );
            }
            // Bound headers, reported for every answer.
            assert!(
                answer.header("last-modified").is_some(),
                "case {label:?}: Last-Modified is a bound header of every answer, got none"
            );

            // Reading again reports exactly the same thing: the read changes nothing.
            let again = get_object_attributes(&router, bucket, &key, "", attributes).await;
            assert_eq!(
                text(&again),
                text(&answer),
                "case {label:?}: a second read must answer the same document"
            );
        }
    }
}

#[tokio::test]
async fn the_version_a_read_names_is_the_version_it_reports() {
    let router = router();
    let bucket = "object-attributes-versions";
    create_bucket(&router, bucket, None, None).await;
    set_versioning(&router, bucket, "Enabled").await;
    let key = "versioned.bin";

    // Two versions of one key, of different sizes, so what a read reports can only be
    // right if it read the version it named.
    let first = put_object(&router, bucket, key, b"first", &[]).await;
    let second = put_object(&router, bucket, key, b"second-and-longer", &[]).await;
    let first_id = first
        .header("x-amz-version-id")
        .expect("a versioning-enabled bucket mints a version id");
    let second_id = second
        .header("x-amz-version-id")
        .expect("a versioning-enabled bucket mints a version id");
    assert_ne!(first_id, second_id, "each version gets an id of its own");

    for (case, version_id, body) in [
        ("the older version", Some(first_id.clone()), b"first".to_vec()),
        (
            "the current version",
            Some(second_id.clone()),
            b"second-and-longer".to_vec(),
        ),
        ("no version at all", None, b"second-and-longer".to_vec()),
    ] {
        let query = version_id
            .as_ref()
            .map(|id| format!("&versionId={id}"))
            .unwrap_or_default();
        let answer = get_object_attributes(
            &router,
            bucket,
            key,
            &query,
            &["ETag", "ObjectSize", "StorageClass"],
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the read must be served, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        assert_eq!(
            answer.element("ObjectSize").as_deref(),
            Some(body.len().to_string().as_str()),
            "case {case:?}: the size must be that version's, got body {:?}",
            text(&answer)
        );
        assert_eq!(
            answer.element("ETag").as_deref(),
            Some(expected_etag_value(&body).as_str()),
            "case {case:?}: the ETag must be that version's, got body {:?}",
            text(&answer)
        );
        // The version id is a bound header of the answer, naming the version that was read.
        assert_eq!(
            answer.header("x-amz-version-id"),
            Some(version_id.unwrap_or(second_id.clone())),
            "case {case:?}: the answer must name the version it reported"
        );
    }
}

#[tokio::test]
async fn an_address_this_emulator_does_not_hold_is_refused_with_the_code_for_what_is_missing() {
    let router = router();
    let bucket = "object-attributes-missing";
    create_bucket(&router, bucket, None, None).await;
    set_versioning(&router, bucket, "Enabled").await;
    let key = "present.bin";
    let stored = put_object(&router, bucket, key, b"held", &[]).await;
    assert_eq!(
        stored.status, 200,
        "the object must be stored first, got {}",
        stored.status
    );
    let version_id = stored
        .header("x-amz-version-id")
        .expect("a versioning-enabled bucket mints a version id");
    let absent_version = "b".repeat(version_id.chars().count());
    assert_ne!(
        absent_version, version_id,
        "the absent-version row must name an id the store did not mint"
    );

    // (case, bucket, key, query, status, code) — one row per part of the address that can
    // be missing, and one for a `versionId` that is not shaped like one.
    let rows: &[(&str, &str, &str, String, u16, &str)] = &[
        (
            "a key the bucket does not hold",
            bucket,
            "absent.bin",
            String::new(),
            404,
            "NoSuchKey",
        ),
        (
            "a bucket this emulator does not hold",
            "object-attributes-no-such-bucket",
            key,
            String::new(),
            404,
            "NoSuchBucket",
        ),
        (
            "a well-formed version the bucket holds none of",
            bucket,
            key,
            format!("&versionId={absent_version}"),
            404,
            "NoSuchVersion",
        ),
        (
            "a version id that is not shaped like one",
            bucket,
            key,
            "&versionId=not-a-version".to_string(),
            400,
            "InvalidArgument",
        ),
    ];

    for (case, target, subject, query, status, code) in rows {
        let answer =
            get_object_attributes(&router, target, subject, query, &["ETag", "ObjectSize"]).await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected HTTP {status}, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        assert_eq!(
            answer.error_code(),
            *code,
            "case {case:?}: expected {code:?}, got {:?}",
            answer.error_code()
        );
    }

    // The held object IS served, so every row above failed on the address it named.
    let served = get_object_attributes(&router, bucket, key, "", &["ObjectSize"]).await;
    assert_eq!(
        served.status, 200,
        "the held object must be served, got {} with body {:?}",
        served.status,
        text(&served)
    );
}

#[tokio::test]
async fn the_etag_field_carries_the_tags_value_and_the_etag_header_its_quoted_rendering() {
    let router = router();
    let bucket = "object-attributes-etag";
    create_bucket(&router, bucket, None, None).await;

    // (case, payload, the published MD5 of that payload when there is one).
    //
    // The rows differ along what the rule generalises over: the payload's size (empty, one
    // byte, a line of text, a long line) and type (ASCII text, bytes including NUL and
    // 0xFF). The hex digests are the MD5 test-suite vectors published in RFC 1321,
    // Appendix A.5 — an expectation computed nowhere in this process.
    let binary: Vec<u8> = (0..=255u8).chain(0..=255u8).collect();
    let rows: &[(&str, &[u8], Option<&str>)] = &[
        ("the empty payload", b"", Some("d41d8cd98f00b204e9800998ecf8427e")),
        ("one byte", b"a", Some("0cc175b9c0f1b6a831c399e269772661")),
        ("three bytes", b"abc", Some("900150983cd24fb0d6963f7d28e17f72")),
        (
            "a line of text",
            b"message digest",
            Some("f96b697d7cb7938d525a2f31aaf161d0"),
        ),
        (
            "a long line of digits",
            b"12345678901234567890123456789012345678901234567890123456789012345678901234567890",
            Some("57edf4a22be3c955ac49da2e2107b67a"),
        ),
        ("bytes spanning 0x00..=0xff twice", &binary, None),
    ];

    for (index, (case, body, published)) in rows.iter().enumerate() {
        let key = format!("etag-subject-{index}.bin");
        let written = put_object(&router, bucket, &key, body, &[]).await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: the object must be stored first, got {} with body {:?}",
            written.status,
            text(&written)
        );

        // The tag's value: the published digest where the row publishes one, and otherwise
        // the digest of the row's own bytes.
        let value = published
            .map(str::to_string)
            .unwrap_or_else(|| expected_etag_value(body));
        if let Some(published) = published {
            assert_eq!(
                expected_etag_value(body),
                *published,
                "case {case:?}: the published MD5 must be the digest of the row's bytes, got {:?}",
                expected_etag_value(body)
            );
        }

        // The `ETag` HEADER carries the quoted-string rendering, on the write and on every
        // header-bound read of the same object.
        let quoted = format!("\"{value}\"");
        for (reader, answer) in [
            ("PutObject", &written),
            ("HeadObject", &head_object(&router, bucket, &key, "").await),
        ] {
            assert_eq!(
                answer.header("etag").as_deref(),
                Some(quoted.as_str()),
                "case {case:?}: {reader} must report the tag inside the header's quotes \
                 {quoted:?}, got {:?}",
                answer.header("etag")
            );
        }

        // The FIELD of this operation's response document carries the value alone: no
        // quotes, escaped (`&quot;`) or not, anywhere in the body.
        let answer = get_object_attributes(&router, bucket, &key, "", &["ETag"]).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the read must be served, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        let reported = answer.element("ETag");
        assert_eq!(
            reported.as_deref(),
            Some(value.as_str()),
            "case {case:?}: expected the entity tag's value {value:?}, got {reported:?} in \
             body {:?}",
            text(&answer)
        );
        let document = text(&answer);
        // The element's RAW text, before the harness undoes XML escaping, so a quote that
        // travelled as `&quot;` is caught too.
        let raw = document
            .split_once("<ETag>")
            .and_then(|(_, rest)| rest.split_once("</ETag>"))
            .map(|(inner, _)| inner.to_string());
        assert!(
            raw.as_deref()
                .is_some_and(|raw| !raw.contains('"') && !raw.contains("&quot;")),
            "case {case:?}: the field must carry no quoted-string quotes, got {raw:?} in \
             body {document:?}"
        );
    }
}
