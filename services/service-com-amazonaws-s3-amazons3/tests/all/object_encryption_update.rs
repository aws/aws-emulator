// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateObjectEncryption`: it re-records the server-side encryption
//! of an object this emulator already holds, and refuses what it cannot serve with one of
//! the three errors it models.
//!
//! The rules under test, with their sources:
//!
//! * "Updates the server-side encryption type of an existing encrypted object in a general
//!   purpose bucket … you can atomically update the server-side encryption type of an
//!   existing object … without any data movement", and it "preserves all object metadata
//!   properties, including the storage class, creation date, last modified date, ETag, and
//!   checksum properties" (the operation's own documentation): after the call every reader
//!   of the object reports the new encryption and the same bytes, ETag and metadata.
//! * The payload is a union whose only member is `SSEKMS`
//!   (`smithy_query(query="shape",shape="ObjectEncryption")`), carrying a required
//!   `KMSKeyArn` and an optional `BucketKeyEnabled` — so what the object carries afterwards
//!   is SSE-KMS under the key the request named.
//! * The operation models exactly `AccessDenied` (403), `InvalidRequest` (400) and
//!   `NoSuchKey` (404) (`smithy_query(query="operation",
//!   operation="UpdateObjectEncryption")`), so a call naming a key the bucket does not hold
//!   is `NoSuchKey` with S3's fixed `The specified key does not exist.` — whatever else the
//!   request carries — and never an error class the operation does not model.
//! * An object S3 Object Lock protects cannot be re-encrypted: "The encryption type for the
//!   specified object can't be updated because that object is protected by S3 Object Lock"
//!   (that operation's own list of `AccessDenied` messages).
//! * The target key must be the bucket owner's OWN: "by default, all UpdateObjectEncryption
//!   requests that specify a customer-managed KMS key are restricted to KMS keys that are
//!   owned by the bucket owner's Amazon Web Services account" (the operation's own
//!   documentation), so every row that is SERVED names a key KMS really holds for this
//!   account and Region — created here with KMS's own `CreateKey` over the shared engine —
//!   and a target ARN KMS resolves to none of this account's keys is refused `AccessDenied`
//!   ([`a_target_key_outside_this_account_is_refused_and_re_encrypts_nothing`] spans that
//!   rule).

use service_com_amazonaws_s3_amazons3::service::wire::XML_NAMED_UNION_MEMBERS;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, send, send_bytes, Answer};
use super::kms_key_references::{created_key, kms_arn, router_with_kms, REGION};
use super::object_and_tagging_roundtrip::{expected_etag, get_object, put_object, text};
use super::object_versioning::set_versioning;

#[test]
fn the_published_union_member_catalog_matches_the_model() {
    // Every UNION member in the model whose `@xmlName` differs from its member name: the
    // codec binds a union member by its member name, so each of these needs a row in the
    // service's wire catalog or a document spelling the published name binds nothing.
    let model_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-s3-amazons3.json"
    );
    let model: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(model_path).expect("the service's Smithy model must be readable"),
    )
    .expect("the Smithy model must parse as JSON");
    let shapes = model["shapes"]
        .as_object()
        .expect("the model must carry a shapes object");

    let mut derived: Vec<(String, String)> = Vec::new();
    for shape in shapes.values().filter(|shape| shape["type"] == "union") {
        let Some(members) = shape["members"].as_object() else {
            continue;
        };
        for (name, member) in members {
            if let Some(published) = member["traits"]["smithy.api#xmlName"].as_str() {
                if published != name {
                    derived.push((published.to_string(), name.to_string()));
                }
            }
        }
    }
    derived.sort();

    let mut declared: Vec<(String, String)> = XML_NAMED_UNION_MEMBERS
        .iter()
        .map(|member| (member.published.to_string(), member.member.to_string()))
        .collect();
    declared.sort();
    assert_eq!(
        declared, derived,
        "the published union-member catalog must name exactly the union members the model \
         gives an @xmlName; declared {declared:?}, the model declares {derived:?}"
    );
}

/// The XML namespace botocore puts on an S3 request payload.
const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The `ServerSideEncryption` value SSE-KMS carries, which the object reports afterwards
/// (`smithy_query(query="shape",shape="ServerSideEncryption")` publishes it).
const SSE_KMS: &str = "aws:kms";

/// The `AccessDenied` sentence this operation publishes for a target key that is not the
/// bucket owner's own, from its own list of them
/// (`smithy_query(query="doc",shape="UpdateObjectEncryption")`).
const FOREIGN_KEY_MESSAGE: &str =
    "The Amazon Web Services KMS key in the request must be owned by the same account as the \
     bucket. Modify the request to specify a KMS key from the same account, and then try \
     again.";

/// The `ObjectEncryption` payload naming `key_arn`, with the S3 Bucket Key setting when the
/// case carries one.
fn payload(key_arn: &str, bucket_key: Option<bool>) -> String {
    let bucket_key = bucket_key
        .map(|enabled| format!("<BucketKeyEnabled>{enabled}</BucketKeyEnabled>"))
        .unwrap_or_default();
    format!(
        "<ObjectEncryption xmlns=\"{XMLNS}\"><SSE-KMS><KMSKeyArn>{key_arn}</KMSKeyArn>\
         {bucket_key}</SSE-KMS></ObjectEncryption>"
    )
}

/// `PUT /<bucket>/<key>?encryption` — `UpdateObjectEncryption`, with whatever query the
/// case carries beside the subresource.
async fn update_object_encryption(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    query: &str,
    body: &str,
) -> Answer {
    send(
        router,
        "PUT",
        &format!("/{bucket}/{key}?encryption{query}"),
        &[],
        body,
    )
    .await
}

/// `HEAD /<bucket>/<key>` — `HeadObject`.
async fn head_object(router: &MultiServiceRouter, bucket: &str, key: &str) -> Answer {
    send_bytes(router, "HEAD", &format!("/{bucket}/{key}"), &[], b"").await
}

#[tokio::test]
async fn an_update_re_records_the_encryption_and_leaves_the_object_alone() {
    let router = router_with_kms();
    let bucket = "object-encryption-update";
    let created = create_bucket(&router, bucket, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket must be created first, got {} with body {:?}",
        created.status,
        text(&created)
    );

    // The rows differ along what the update generalises over: WHICH of this account's keys
    // it names (a different key per row, so nothing is keyed to one value), whether it asks
    // for an S3 Bucket Key (on, off, unnamed), and the object's own bytes and metadata,
    // which must survive untouched. Each key is one KMS really holds, which is what the
    // operation restricts its target to.
    let (_, first) = created_key(&router, "object-encryption-update-first").await;
    let (_, second) = created_key(&router, "object-encryption-update-second").await;
    let (_, third) = created_key(&router, "object-encryption-update-third").await;
    let rows: &[(&str, &str, Option<bool>)] = &[
        ("a key ARN alone", first.as_str(), None),
        ("a second key, with the Bucket Key on", second.as_str(), Some(true)),
        ("a third key, with the Bucket Key off", third.as_str(), Some(false)),
    ];

    for (index, (case, key_arn, bucket_key)) in rows.iter().enumerate() {
        let key = format!("re-encrypted-{index}.bin");
        let body: Vec<u8> = (0..=u8::try_from(index).unwrap_or(0)).cycle().take(64).collect();
        let stored = put_object(
            &router,
            bucket,
            &key,
            &body,
            &[("content-type", "text/plain")],
        )
        .await;
        assert_eq!(
            stored.status, 200,
            "case {case:?}: the object must be stored first, got {} with body {:?}",
            stored.status,
            text(&stored)
        );

        let updated =
            update_object_encryption(&router, bucket, &key, "", &payload(key_arn, *bucket_key))
                .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: the update must be served, got {} with body {:?}",
            updated.status,
            text(&updated)
        );

        // Every reader reports the encryption the update named, and the object's own bytes
        // and ETag are exactly what they were.
        for (reader, answer) in [
            ("GetObject", get_object(&router, bucket, &key, "", &[]).await),
            ("HeadObject", head_object(&router, bucket, &key).await),
        ] {
            assert_eq!(
                answer.status, 200,
                "case {case:?}/{reader}: the object must still be readable, got {}",
                answer.status
            );
            assert_eq!(
                answer.header("x-amz-server-side-encryption").as_deref(),
                Some(SSE_KMS),
                "case {case:?}/{reader}: the object must report SSE-KMS, got {:?}",
                answer.header("x-amz-server-side-encryption")
            );
            assert_eq!(
                answer
                    .header("x-amz-server-side-encryption-aws-kms-key-id")
                    .as_deref(),
                Some(*key_arn),
                "case {case:?}/{reader}: the object must report the key the update named, \
                 got {:?}",
                answer.header("x-amz-server-side-encryption-aws-kms-key-id")
            );
            assert_eq!(
                answer
                    .header("x-amz-server-side-encryption-bucket-key-enabled")
                    .as_deref(),
                // The header states that S3 USED an S3 Bucket Key on the object, so an update
                // that switched it off, or named it at all, leaves no header behind.
                bucket_key.filter(|enabled| *enabled).map(|_| "true"),
                "case {case:?}/{reader}: the Bucket Key setting must be the one the update \
                 named, got {:?}",
                answer.header("x-amz-server-side-encryption-bucket-key-enabled")
            );
            assert_eq!(
                answer.header("etag").as_deref(),
                Some(expected_etag(&body).as_str()),
                "case {case:?}/{reader}: the update preserves the ETag, got {:?}",
                answer.header("etag")
            );
            assert_eq!(
                answer.header("content-type").as_deref(),
                Some("text/plain"),
                "case {case:?}/{reader}: the update preserves the metadata, got {:?}",
                answer.header("content-type")
            );
        }
        let read = get_object(&router, bucket, &key, "", &[]).await;
        assert_eq!(
            read.body.as_ref(),
            body.as_slice(),
            "case {case:?}: the update moves no data, so the bytes are unchanged"
        );
    }
}

#[tokio::test]
async fn an_update_naming_an_address_this_emulator_does_not_hold_is_refused_with_its_own_404() {
    let router = router_with_kms();
    let bucket = "object-encryption-update-missing";
    let created = create_bucket(&router, bucket, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket must be created first, got {}",
        created.status
    );
    // The target key every row names is one KMS holds for this account, so each row fails on
    // the ADDRESS and not on the key.
    let (_, key_arn) = created_key(&router, "object-encryption-update-missing").await;
    set_versioning(&router, bucket, "Enabled").await;
    let held = "present.bin";
    let stored = put_object(&router, bucket, held, b"held", &[]).await;
    assert_eq!(
        stored.status, 200,
        "the object must be stored first, got {} with body {:?}",
        stored.status,
        text(&stored)
    );
    let version_id = stored
        .header("x-amz-version-id")
        .expect("a versioning-enabled bucket reports the version id it minted");

    // Every row names an address the bucket does not hold, differing in HOW: a key that was
    // never written, a key that is a prefix and an extension of the one that was, a key
    // differing only in case, and the held key under a version id the bucket has none of.
    // The payload is well formed in all of them, so the answer is about the ADDRESS.
    let absent_version = "a".repeat(version_id.chars().count());
    let rows: &[(&str, &str, String, &str)] = &[
        ("a key never written", "absent.bin", String::new(), "NoSuchKey"),
        ("a prefix of the held key", "present", String::new(), "NoSuchKey"),
        (
            "an extension of the held key",
            "present.bin.2",
            String::new(),
            "NoSuchKey",
        ),
        ("the held key in another case", "Present.bin", String::new(), "NoSuchKey"),
        (
            "the held key under a version it has none of",
            held,
            format!("&versionId={absent_version}"),
            "NoSuchVersion",
        ),
    ];

    for (case, key, query, code) in rows {
        assert_ne!(
            absent_version, version_id,
            "the version row must name an id that is NOT the one the store minted"
        );
        let answer =
            update_object_encryption(&router, bucket, key, query, &payload(&key_arn, None)).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: an address this emulator does not hold is HTTP 404, got {} with \
             body {:?}",
            answer.status,
            text(&answer)
        );
        assert_eq!(
            answer.error_code(),
            *code,
            "case {case:?}: expected {code:?}, got {:?}",
            answer.error_code()
        );
        assert_eq!(
            answer.error_message(),
            if *code == "NoSuchKey" {
                "The specified key does not exist."
            } else {
                "The specified version does not exist."
            },
            "case {case:?}: expected the published message, got {:?}",
            answer.error_message()
        );
    }

    // A bucket this emulator does not hold is answered about the bucket.
    let absent_bucket = update_object_encryption(
        &router,
        "object-encryption-update-no-such-bucket",
        held,
        "",
        &payload(&key_arn, None),
    )
    .await;
    assert_eq!(
        absent_bucket.error_code(),
        "NoSuchBucket",
        "a bucket this emulator does not hold is NoSuchBucket, got {:?} with status {}",
        absent_bucket.error_code(),
        absent_bucket.status
    );

    // The held key IS served, so every row above failed on the address rather than on the
    // request being unserveable.
    let served =
        update_object_encryption(&router, bucket, held, "", &payload(&key_arn, None)).await;
    assert_eq!(
        served.status, 200,
        "the held object must be updated, got {} with body {:?}",
        served.status,
        text(&served)
    );
}

#[tokio::test]
async fn an_update_naming_a_kms_key_arn_that_is_not_one_is_an_invalid_request() {
    let router = router();
    let bucket = "object-encryption-update-invalid";
    let created = create_bucket(&router, bucket, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket must be created first, got {}",
        created.status
    );
    let key = "subject.bin";
    let stored = put_object(&router, bucket, key, b"held", &[]).await;
    assert_eq!(
        stored.status, 200,
        "the object must be stored first, got {}",
        stored.status
    );

    // Each row is a value outside `NonEmptyKmsKeyArnString`'s published pattern, paired
    // with the reason the refusal quotes: the ARN grammar is
    // `arn:<partition>:<service>:<region>:<account-id>:<resource>`, and the reason names
    // the first thing about the value that breaks it. The rows differ in WHICH part breaks.
    for (value, reason) in [
        (
            "not-a-valid-kms-key-arn-string",
            "Malformed ARN - doesn't start with 'arn:'",
        ),
        (
            "1234abcd-12ab-34cd-56ef-1234567890ab",
            "Malformed ARN - doesn't start with 'arn:'",
        ),
        (
            "arn:aws:kms:us-east-1:0000",
            "Malformed ARN - doesn't contain enough sections",
        ),
        (
            "arn::kms:us-east-1:000000000000:key/1234abcd",
            "Malformed ARN - no AWS partition",
        ),
        (
            "arn:aws::us-east-1:000000000000:key/1234abcd",
            "Malformed ARN - no AWS service",
        ),
        (
            "arn:aws:kms:us-east-1:000000000000:",
            "Malformed ARN - no AWS resource",
        ),
        (
            "arn:aws:s3:us-east-1:000000000000:key/1234abcd",
            "Invalid ARN - the ARN does not name the resource this member requires",
        ),
    ] {
        let answer = update_object_encryption(&router, bucket, key, "", &payload(value, None)).await;
        assert_eq!(
            answer.error_code(),
            "InvalidRequest",
            "case {value:?}: a KMS key ARN that is not one is InvalidRequest, got {:?} with \
             status {} and body {:?}",
            answer.error_code(),
            answer.status,
            text(&answer)
        );
        assert_eq!(
            answer.status, 400,
            "case {value:?}: expected HTTP 400, got {}",
            answer.status
        );
        assert_eq!(
            answer.error_message(),
            format!(
                "Requests modifying object encryption configuration to SSE-KMS require a \
                 valid target kms key arn: {reason}"
            ),
            "case {value:?}: expected the published message naming the reason {reason:?}, got \
             {:?}",
            answer.error_message()
        );

        // Nothing was written: the object still carries the encryption it was stored with.
        let read = head_object(&router, bucket, key).await;
        assert_eq!(
            read.header("x-amz-server-side-encryption").as_deref(),
            Some("AES256"),
            "case {value:?}: a refused update must leave the object's encryption alone, got \
             {:?}",
            read.header("x-amz-server-side-encryption")
        );
    }
}

#[tokio::test]
async fn a_target_key_outside_this_account_is_refused_and_re_encrypts_nothing() {
    let router = router_with_kms();
    let bucket = "object-encryption-update-foreign-key";
    let created = create_bucket(&router, bucket, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket must be created first, got {}",
        created.status
    );
    let key = "subject.bin";
    let stored = put_object(&router, bucket, key, b"held", &[]).await;
    assert_eq!(
        stored.status, 200,
        "the object must be stored first, got {} with body {:?}",
        stored.status,
        text(&stored)
    );

    // Two keys KMS really holds for THIS account and Region, which is what the operation
    // restricts its target to, and one key id no `CreateKey` of this endpoint ever minted.
    let (_, held) = created_key(&router, "object-encryption-update-owned").await;
    let (_, also_held) = created_key(&router, "object-encryption-update-owned-too").await;
    let unminted = "4f5fb9d9-3e64-4ee1-a07a-0cf0bcfee9f9";

    // Each row is a target ARN inside `NonEmptyKmsKeyArnString`'s published pattern
    // (`^arn:aws[a-zA-Z0-9-]*:kms:[a-z0-9-]+:[0-9]{12}:key/[a-zA-Z0-9-]+$`, so the ARN
    // syntax refusal is not what is under test), paired with whether the update is SERVED.
    // The refused rows differ in WHY KMS resolves none of this account's keys for them: no
    // key of this account has that id, the ARN names another account, or it names another
    // Region. They come FIRST, so each of them is judged against the object as it was
    // stored; the two served rows close the table as its positive control.
    let rows: &[(&str, String, bool)] = &[
        (
            "a key ARN of this account and Region that no key has",
            kms_arn(&format!("key/{unminted}")),
            false,
        ),
        (
            "the same key id under another account",
            format!("arn:aws:kms:{REGION}:111122223333:key/{unminted}"),
            false,
        ),
        (
            "a key of this account in another Region",
            held.replace(REGION, "eu-west-1"),
            false,
        ),
        ("a key this account holds", held.clone(), true),
        (
            "another key this account holds, so nothing is keyed to one value",
            also_held.clone(),
            true,
        ),
    ];

    for (case, target, served) in rows {
        let answer = update_object_encryption(&router, bucket, key, "", &payload(target, None)).await;
        if *served {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a target key this account owns must be served, got {} with \
                 body {:?}",
                answer.status,
                text(&answer)
            );
            let read = head_object(&router, bucket, key).await;
            assert_eq!(
                read.header("x-amz-server-side-encryption-aws-kms-key-id")
                    .as_deref(),
                Some(target.as_str()),
                "case {case:?}: the object must report the key the update named, got {:?}",
                read.header("x-amz-server-side-encryption-aws-kms-key-id")
            );
            continue;
        }
        assert_eq!(
            answer.status, 403,
            "case {case:?}: a target key outside this account is HTTP 403, got {} with body \
             {:?}",
            answer.status,
            text(&answer)
        );
        assert_eq!(
            answer.error_code(),
            "AccessDenied",
            "case {case:?}: expected AccessDenied, got {:?}",
            answer.error_code()
        );
        assert_eq!(
            answer.error_message(),
            FOREIGN_KEY_MESSAGE,
            "case {case:?}: expected the published sentence, got {:?}",
            answer.error_message()
        );
        // Nothing was re-encrypted: the object still carries the encryption it was stored
        // with, which every refused row is judged against.
        let read = head_object(&router, bucket, key).await;
        assert_eq!(
            read.header("x-amz-server-side-encryption").as_deref(),
            Some("AES256"),
            "case {case:?}: a refused update must leave the object's encryption alone, got \
             {:?}",
            read.header("x-amz-server-side-encryption")
        );
    }
}
