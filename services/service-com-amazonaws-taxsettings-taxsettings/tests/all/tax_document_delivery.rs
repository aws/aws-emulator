// SPDX-License-Identifier: CC0-1.0

//! The DOWNLOAD `GetTaxRegistrationDocument` performs, and the document content
//! it downloads, end to end over the wire with Amazon S3 mounted beside this
//! service on ONE engine.
//!
//! The rule these tables assert is one: a document submitted with a tax
//! registration is held with its CONTENT, and a download writes that content as an
//! object into the location the request named — the operation "downloads your tax
//! documents to the Amazon S3 bucket that you specify in your request"
//! (`smithy_query(query="doc",
//! shape="com.amazonaws.taxsettings#GetTaxRegistrationDocument")`), so the place
//! the response names holds the document's own bytes rather than nothing.
//!
//! The objects are read through Amazon S3's OWN client over this emulator's
//! in-process callback, because an object exists only if the service that owns
//! objects serves it.
//!
//! The rows span the classes the rule generalises over rather than one recorded
//! request: both shapes a `TaxRegistrationDocument` carries its content in (an
//! uploaded `file`, an `s3Location` naming an object), contents from a few ASCII
//! bytes through every byte value to the published maximum of
//! `TaxRegistrationDocFile$fileContent` (1..5242880 bytes), every form of
//! `destinationS3Location` (absent, a bucket alone, a bucket with a prefix, the
//! same prefix spelled with its trailing separator, a nested prefix, another
//! bucket, a bucket this emulator holds none of), one document and the five the
//! list admits, and the same request made twice.
//!
//! The TaxSettings harness (`call`, `mount_on`, `make_validations_due`) lives in
//! `tax_registration_lifecycle.rs` and the document setup (`store_documents`,
//! `get_document`) in `tax_registration_document.rs`; nothing here builds a
//! second one.

use std::collections::BTreeSet;
use std::sync::Arc;

use aws_smithy_types::base64;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::tax_registration_document::{get_document, store_documents};
use crate::tax_registration_lifecycle::{call, make_validations_due, mount_on, Answer};

/// The Region the S3 client below addresses, the one this suite's requests are
/// signed for.
const REGION: &str = "us-east-1";

/// The bucket a case that is not about the destination downloads into.
const DESTINATION: &str = "my-tax-documents";

/// The bucket a submitted `s3Location` references its document in.
const SOURCE: &str = "trn-source-documents";

/// This service and the one it holds and delivers documents through — Amazon S3 —
/// over ONE shared engine, which is how the assembled emulator mounts them.
fn emulator_with_objects() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = mount_on(&engine);
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// An Amazon S3 client over this emulator's own in-process callback — the seam a
/// download writes its object through, so a read here sees what the download
/// really stored.
fn s3(router: &MultiServiceRouter) -> aws_sdk_s3::Client {
    let callback = smithy_server_runtime::EmbeddedEmulator::from_router(router.clone());
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
        .region(aws_sdk_s3::config::Region::new(REGION))
        .endpoint_url(format!("https://s3.{REGION}.amazonaws.com"))
        .force_path_style(true)
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    aws_sdk_s3::Client::from_conf(config)
}

/// Create one bucket through Amazon S3's own API, the way a caller makes a place
/// to download into.
async fn create_bucket(router: &MultiServiceRouter, bucket: &str) {
    s3(router)
        .create_bucket()
        .bucket(bucket)
        .send()
        .await
        .unwrap_or_else(|error| panic!("the bucket {bucket:?} is created: {error:?}"));
}

/// The bytes Amazon S3 serves for one key, or `None` when it holds no such
/// object (or no such bucket).
async fn object_bytes(router: &MultiServiceRouter, bucket: &str, key: &str) -> Option<Vec<u8>> {
    let read = s3(router)
        .get_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await
        .ok()?;
    Some(
        read.body
            .collect()
            .await
            .expect("the object's bytes are read")
            .to_vec(),
    )
}

/// The keys one bucket holds, in the order S3 lists them.
async fn listed_keys(router: &MultiServiceRouter, bucket: &str) -> Vec<String> {
    let listed = s3(router)
        .list_objects_v2()
        .bucket(bucket)
        .send()
        .await
        .unwrap_or_else(|error| panic!("the bucket {bucket:?} is listed: {error:?}"));
    listed
        .contents()
        .iter()
        .filter_map(|object| object.key().map(str::to_string))
        .collect()
}

/// The buckets the caller's own `ListBuckets` reports.
async fn listed_buckets(router: &MultiServiceRouter) -> Vec<String> {
    let listed = s3(router)
        .list_buckets()
        .send()
        .await
        .expect("the account's buckets are listed");
    listed
        .buckets()
        .iter()
        .filter_map(|bucket| bucket.name().map(str::to_string))
        .collect()
}

/// The bucket and key a reported `destinationFilePath` names, so the assertion is
/// about the object that path resolves to rather than about the string.
fn path_location(path: &str) -> (String, String) {
    let rest = path.strip_prefix("s3://").unwrap_or(path);
    let (bucket, key) = rest.split_once('/').unwrap_or((rest, ""));
    (bucket.to_string(), key.to_string())
}

/// The bucket and key a reported `presignedS3Url` names, for the same reason.
fn url_location(url: &str) -> (String, String) {
    let rest = url.strip_prefix("https://").unwrap_or(url);
    let (host, key) = rest.split_once('/').unwrap_or((rest, ""));
    let bucket = host.strip_suffix(".s3.amazonaws.com").unwrap_or(host);
    (bucket.to_string(), key.to_string())
}

/// One submitted document: the shape it is submitted in and the content a
/// download of it must deliver.
struct ContentRow {
    name: &'static str,
    account_id: &'static str,
    /// Whether the document is submitted as an uploaded file or as a reference to
    /// an object in the caller's bucket.
    referenced: bool,
    document_name: &'static str,
    content: Vec<u8>,
}

/// The contents the rule holds over.
///
/// `TaxRegistrationDocument` carries its content either as an uploaded `file`,
/// whose `fileContent` is a blob of 1..5242880 bytes, or as an `s3Location`
/// naming the object it is stored as (`smithy_query(query="shape",
/// shape="TaxRegistrationDocFile")`, `smithy_query(query="shape",
/// shape="TaxRegistrationDocument")`), so the rows cover both shapes and, for the
/// content itself, a few ASCII bytes, every byte value (NUL and the high bytes
/// included), one byte — the shortest the member admits — and the longest it
/// admits.
fn content_rows() -> Vec<ContentRow> {
    vec![
        ContentRow {
            name: "an uploaded file of a few ASCII bytes",
            account_id: "000000000501",
            referenced: false,
            document_name: "certificate.pdf",
            content: b"hello".to_vec(),
        },
        ContentRow {
            name: "an uploaded file of every byte value",
            account_id: "000000000502",
            referenced: false,
            document_name: "scan.bin",
            content: (0u8..=255).collect(),
        },
        ContentRow {
            name: "an uploaded file of the shortest content the member admits",
            account_id: "000000000503",
            referenced: false,
            document_name: "one-byte.pdf",
            content: vec![b'x'],
        },
        ContentRow {
            name: "an uploaded file of the longest content the member admits",
            account_id: "000000000504",
            referenced: false,
            document_name: "whole-dossier.pdf",
            // The published maximum of `fileContent`: 5242880 bytes.
            content: vec![b'd'; 5_242_880],
        },
        ContentRow {
            name: "a document referenced in the caller's own bucket",
            account_id: "000000000505",
            referenced: true,
            document_name: "2026/de/vat-certificate.pdf",
            content: b"referenced certificate bytes".to_vec(),
        },
        ContentRow {
            name: "a referenced document of every byte value",
            account_id: "000000000506",
            referenced: true,
            document_name: "archive/scan.bin",
            content: (0u8..=255).rev().collect(),
        },
    ]
}

/// The `taxRegistrationDocuments` entry one row is submitted as, seeding the
/// source object first for a row that REFERENCES one.
async fn submit(router: &MultiServiceRouter, row: &ContentRow) -> Value {
    if row.referenced {
        s3(router)
            .put_object()
            .bucket(SOURCE)
            .key(row.document_name)
            .body(aws_sdk_s3::primitives::ByteStream::from(row.content.clone()))
            .send()
            .await
            .unwrap_or_else(|error| panic!("case {:?}: the source object is stored: {error:?}", row.name));
        json!({ "s3Location": { "bucket": SOURCE, "key": row.document_name } })
    } else {
        json!({
            "file": {
                "fileName": row.document_name,
                "fileContent": base64::encode(&row.content),
            },
        })
    }
}

/// RULE: the content of a submitted document is held where a read of that
/// document reports it, and a download writes that very content into the location
/// the request named — whichever shape the document was submitted in and whatever
/// its bytes are.
#[tokio::test]
async fn a_submitted_documents_content_is_held_and_downloaded() {
    let (router, _engine) = emulator_with_objects();
    create_bucket(&router, SOURCE).await;
    create_bucket(&router, DESTINATION).await;

    for row in content_rows() {
        let case = row.name;
        let document = submit(&router, &row).await;
        let metadatas = store_documents(&router, row.account_id, &json!([document])).await;
        let metadata = metadatas
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the registration reports its document"));

        let answer = get_document(
            &router,
            &metadata,
            Some(&json!({ "bucket": DESTINATION, "prefix": "tax/2026" })),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {answer:?}"
        );

        // The location the response reports the document itself at holds the
        // document's own content: the reported URL resolves to an object, not to
        // nothing.
        let url = answer
            .get("presignedS3Url")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let (held_bucket, held_key) = url_location(&url);
        let held = object_bytes(&router, &held_bucket, &held_key).await;
        assert_eq!(
            held.as_deref(),
            Some(row.content.as_slice()),
            "case {case:?}: the reported URL {url:?} must name an object carrying the submitted \
             content ({} bytes), got {:?} bytes",
            row.content.len(),
            held.as_ref().map(Vec::len)
        );

        // And the destination the request named now holds that content too, at
        // the key the reported path names.
        let path = answer
            .get("destinationFilePath")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let (bucket, key) = path_location(&path);
        assert_eq!(
            (bucket.as_str(), key.as_str()),
            (DESTINATION, format!("tax/2026/{}", row.document_name).as_str()),
            "case {case:?}: expected the path to name the destination the request gave, got \
             {path:?}"
        );
        let delivered = object_bytes(&router, &bucket, &key).await;
        assert_eq!(
            delivered.as_deref(),
            Some(row.content.as_slice()),
            "case {case:?}: expected the download to deliver the document's {} bytes to \
             {path:?}, got {:?} bytes",
            row.content.len(),
            delivered.as_ref().map(Vec::len)
        );
    }

    // The documents this service holds are its own copies, so the bucket they
    // live in is none of the caller's: `ListBuckets` reports only the buckets the
    // caller made.
    let buckets = listed_buckets(&router).await;
    assert_eq!(
        buckets,
        vec![DESTINATION.to_string(), SOURCE.to_string()],
        "the account's own buckets are the ones it created, got {buckets:?}"
    );
}

/// One destination a request may name, and the key the document is expected to be
/// delivered to under it.
struct DestinationRow {
    name: &'static str,
    destination: Option<Value>,
    expected: Option<(String, String)>,
}

/// The destinations the rule holds over.
///
/// `destinationS3Location` is optional and its `prefix` is "the Amazon S3 object
/// prefix that you specify for your tax document file"
/// (`smithy_query(query="shape", shape="DestinationS3Location")`), so the rows are
/// no destination at all — which asked for no download — a bucket alone, a bucket
/// with a prefix, the same prefix spelled with its trailing separator (the same
/// place in the bucket), a nested prefix, another bucket, and a bucket this
/// emulator holds none of, which there is nowhere to download into.
fn destination_rows(document_name: &str, other_bucket: &str) -> Vec<DestinationRow> {
    let destination = DESTINATION.to_string();
    vec![
        DestinationRow {
            name: "no destination",
            destination: None,
            expected: None,
        },
        DestinationRow {
            name: "a bucket alone",
            destination: Some(json!({ "bucket": DESTINATION })),
            expected: Some((destination.clone(), document_name.to_string())),
        },
        DestinationRow {
            name: "a bucket and a prefix",
            destination: Some(json!({ "bucket": DESTINATION, "prefix": "tax/2026" })),
            expected: Some((destination.clone(), format!("tax/2026/{document_name}"))),
        },
        DestinationRow {
            name: "the same prefix spelled with its trailing separator",
            destination: Some(json!({ "bucket": DESTINATION, "prefix": "tax/2026/" })),
            expected: Some((destination.clone(), format!("tax/2026/{document_name}"))),
        },
        DestinationRow {
            name: "a deeply nested prefix",
            destination: Some(json!({ "bucket": DESTINATION, "prefix": "tax/2026/de/vat" })),
            expected: Some((destination, format!("tax/2026/de/vat/{document_name}"))),
        },
        DestinationRow {
            name: "another bucket, the same prefix",
            destination: Some(json!({ "bucket": other_bucket, "prefix": "tax/2026" })),
            expected: Some((other_bucket.to_string(), format!("tax/2026/{document_name}"))),
        },
        DestinationRow {
            name: "a bucket this emulator holds none of",
            destination: Some(json!({ "bucket": "no-such-destination-bucket" })),
            // Nothing is written: there is no bucket to download into. The
            // response still reports where the request asked the document to go.
            expected: None,
        },
    ]
}

/// RULE: the download goes to the location the request named and nowhere else — a
/// request that names no destination writes nothing, two requests naming
/// different locations leave the document in both, and the same request twice
/// leaves exactly what one left.
#[tokio::test]
async fn a_download_writes_the_document_only_where_the_request_asked() {
    let (router, _engine) = emulator_with_objects();
    let other_bucket = "acme-trn-archive";
    create_bucket(&router, DESTINATION).await;
    create_bucket(&router, other_bucket).await;

    let account_id = "000000000511";
    let document_name = "certificate.pdf";
    let content = b"the certificate itself".to_vec();
    let metadatas = store_documents(
        &router,
        account_id,
        &json!([
            {
                "file": {
                    "fileName": document_name,
                    "fileContent": base64::encode(&content),
                },
            },
        ]),
    )
    .await;
    let metadata = metadatas
        .first()
        .cloned()
        .expect("the registration reports its document");

    // Nothing has been downloaded yet, so neither destination holds anything.
    for bucket in [DESTINATION, other_bucket] {
        assert!(
            listed_keys(&router, bucket).await.is_empty(),
            "no download has run, so {bucket:?} must be empty"
        );
    }

    let mut expected_keys: BTreeSet<(String, String)> = BTreeSet::new();
    for row in destination_rows(document_name, other_bucket) {
        let case = row.name;
        // Asked twice: a download is the same write however often it is made.
        for attempt in 1..=2 {
            let answer = get_document(&router, &metadata, row.destination.as_ref()).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?} (attempt {attempt}): expected 200, got {answer:?}"
            );
            assert_eq!(
                answer.get("destinationFilePath").as_str().is_some(),
                row.destination.is_some(),
                "case {case:?} (attempt {attempt}): a path is reported exactly when a \
                 destination was named, got {:?}",
                answer.body
            );
        }
        if let Some((bucket, key)) = row.expected.clone() {
            let delivered = object_bytes(&router, &bucket, &key).await;
            assert_eq!(
                delivered.as_deref(),
                Some(content.as_slice()),
                "case {case:?}: expected the document's bytes at {bucket}/{key}, got {:?}",
                delivered.as_ref().map(Vec::len)
            );
            expected_keys.insert((bucket, key));
        }
        // Exactly the places the rows asked for hold the document — the
        // destination a row named nothing for (no destination at all, or a bucket
        // this emulator holds none of) added nothing anywhere.
        let mut held: BTreeSet<(String, String)> = BTreeSet::new();
        for bucket in [DESTINATION, other_bucket] {
            for key in listed_keys(&router, bucket).await {
                held.insert((bucket.to_string(), key));
            }
        }
        assert_eq!(
            held, expected_keys,
            "case {case:?}: expected the document only where it was asked for, got {held:?}"
        );
    }
}

/// RULE: a registration carrying several documents holds each one's own content,
/// and a download delivers each to its own key under the destination.
#[tokio::test]
async fn every_document_of_one_registration_is_held_and_downloaded_on_its_own() {
    let (router, _engine) = emulator_with_objects();
    create_bucket(&router, DESTINATION).await;

    // The five documents `VerificationDetails$taxRegistrationDocuments` admits
    // (`smithy_query(query="shape", shape="VerificationDetails")`), each with its
    // own name and its own content.
    let documents: Vec<(String, Vec<u8>)> = (0..5)
        .map(|index| {
            (
                format!("document-{index}.pdf"),
                format!("content of document {index}").into_bytes(),
            )
        })
        .collect();
    let submitted: Vec<Value> = documents
        .iter()
        .map(|(name, content)| {
            json!({ "file": { "fileName": name, "fileContent": base64::encode(content) } })
        })
        .collect();
    let metadatas = store_documents(&router, "000000000521", &json!(submitted)).await;
    assert_eq!(
        metadatas.len(),
        documents.len(),
        "the registration must report one metadata per submitted document, got {metadatas:?}"
    );

    for (index, metadata) in metadatas.iter().enumerate() {
        let (name, content) = &documents[index];
        let case = format!("document {index}");
        let answer = get_document(
            &router,
            metadata,
            Some(&json!({ "bucket": DESTINATION, "prefix": "tax" })),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {answer:?}"
        );
        let delivered = object_bytes(&router, DESTINATION, &format!("tax/{name}")).await;
        assert_eq!(
            delivered.as_deref(),
            Some(content.as_slice()),
            "case {case:?}: expected its own content at tax/{name}, got {delivered:?}"
        );
    }

    let keys = listed_keys(&router, DESTINATION).await;
    assert_eq!(
        keys,
        documents
            .iter()
            .map(|(name, _)| format!("tax/{name}"))
            .collect::<Vec<_>>(),
        "every document must be delivered to its own key, got {keys:?}"
    );
}

/// RULE: a document access token names the documents of ONE revision of ONE
/// account's registration, so rewriting or deleting that registration drops the
/// copies those tokens named — while the current revision's copy is held.
#[tokio::test]
async fn retiring_a_registrations_tokens_drops_the_copies_they_named() {
    let (router, engine) = emulator_with_objects();
    let account_id = "000000000531";

    let first = store_documents(
        &router,
        account_id,
        &json!([
            { "file": { "fileName": "certificate.pdf", "fileContent": base64::encode("first") } },
        ]),
    )
    .await;
    let first_metadata = first
        .first()
        .cloned()
        .expect("the first revision reports its document");
    let first_answer = get_document(&router, &first_metadata, None).await;
    let (first_bucket, first_key) = url_location(
        first_answer
            .get("presignedS3Url")
            .as_str()
            .unwrap_or_default(),
    );
    assert_eq!(
        object_bytes(&router, &first_bucket, &first_key)
            .await
            .as_deref(),
        Some(b"first".as_slice()),
        "the first revision's document must be held, got {first_answer:?}"
    );

    // A second write needs the first registration's validation to have fallen
    // due — an outstanding TRN may not be replaced — which is state, never a
    // sleep.
    make_validations_due(&engine);
    let second = store_documents(
        &router,
        account_id,
        &json!([
            { "file": { "fileName": "renewed.pdf", "fileContent": base64::encode("second") } },
        ]),
    )
    .await;
    let second_metadata = second
        .first()
        .cloned()
        .expect("the second revision reports its document");
    let second_answer = get_document(&router, &second_metadata, None).await;
    let (second_bucket, second_key) = url_location(
        second_answer
            .get("presignedS3Url")
            .as_str()
            .unwrap_or_default(),
    );
    assert_eq!(
        object_bytes(&router, &second_bucket, &second_key)
            .await
            .as_deref(),
        Some(b"second".as_slice()),
        "the current revision's document must be held, got {second_answer:?}"
    );
    assert_eq!(
        object_bytes(&router, &first_bucket, &first_key).await,
        None,
        "the retired revision's copy must be gone, it was still at {first_bucket}/{first_key}"
    );

    let deleted = call(
        &router,
        "DeleteTaxRegistration",
        &json!({ "accountId": account_id }),
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "the setup delete must succeed, got {deleted:?}"
    );
    assert_eq!(
        object_bytes(&router, &second_bucket, &second_key).await,
        None,
        "a deleted registration's copies must be gone, one was still at \
         {second_bucket}/{second_key}"
    );
}

/// RULE: `BatchPutTaxRegistration` writes ONE entry to every account it names, so
/// each of those accounts holds its own copy of that entry's documents and a
/// download for any of them delivers the submitted content.
#[tokio::test]
async fn a_batch_write_holds_the_entrys_documents_for_every_account_it_wrote() {
    let (router, _engine) = emulator_with_objects();
    create_bucket(&router, DESTINATION).await;
    let accounts = ["000000000541", "000000000542", "000000000543"];
    let content = b"batched certificate".to_vec();

    let batch = call(
        &router,
        "BatchPutTaxRegistration",
        &json!({
            "accountIds": accounts,
            "taxRegistrationEntry": {
                "registrationId": "DE136695976",
                "registrationType": "VAT",
                "legalName": "Acme GmbH",
                "legalAddress": { "postalCode": "10115", "countryCode": "DE" },
                "verificationDetails": {
                    "taxRegistrationDocuments": [
                        {
                            "file": {
                                "fileName": "certificate.pdf",
                                "fileContent": base64::encode(&content),
                            },
                        },
                    ],
                },
            },
        }),
    )
    .await;
    assert_eq!(batch.status, 200, "the batch write must succeed: {batch:?}");

    for account_id in accounts {
        let read = call(
            &router,
            "GetTaxRegistration",
            &json!({ "accountId": account_id }),
        )
        .await;
        let metadata = read
            .body
            .pointer("/taxRegistration/taxDocumentMetadatas/0")
            .cloned()
            .unwrap_or_else(|| panic!("case {account_id:?}: the registration reports its document"));
        let answer = get_document(
            &router,
            &metadata,
            Some(&json!({ "bucket": DESTINATION, "prefix": account_id })),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {account_id:?}: expected 200, got {answer:?}"
        );
        let delivered =
            object_bytes(&router, DESTINATION, &format!("{account_id}/certificate.pdf")).await;
        assert_eq!(
            delivered.as_deref(),
            Some(content.as_slice()),
            "case {account_id:?}: expected the batch entry's document to be delivered, got {:?}",
            delivered.as_ref().map(Vec::len)
        );
    }
}

/// RULE: a location a REQUEST names is a bucket of the caller's own. This
/// service's per-account document buckets are its own, and their names and keys
/// are derived from the account and the revision rather than secret, so a request
/// cannot name one as the source of a document it submits (reading another
/// account's held document) nor as the destination of a download (replacing it).
#[tokio::test]
async fn a_request_cannot_name_this_services_own_document_buckets() {
    let (router, _engine) = emulator_with_objects();
    create_bucket(&router, DESTINATION).await;

    // One account's document, held where this service keeps it.
    let other = "000000000551";
    let other_metadatas = store_documents(
        &router,
        other,
        &json!([
            {
                "file": {
                    "fileName": "certificate.pdf",
                    "fileContent": base64::encode("the other account's document"),
                },
            },
        ]),
    )
    .await;
    let other_metadata = other_metadatas
        .first()
        .cloned()
        .expect("the other account's registration reports its document");
    let held = get_document(&router, &other_metadata, None).await;
    let (held_bucket, held_key) =
        url_location(held.get("presignedS3Url").as_str().unwrap_or_default());
    assert_eq!(
        object_bytes(&router, &held_bucket, &held_key)
            .await
            .as_deref(),
        Some(b"the other account's document".as_slice()),
        "the other account's document must be held, got {held:?}"
    );

    // As a SOURCE: a submission naming that location holds no content, so a
    // download of it delivers nothing into the caller's own bucket.
    let source_reader = "000000000552";
    let reader_metadatas = store_documents(
        &router,
        source_reader,
        &json!([{ "s3Location": { "bucket": held_bucket, "key": held_key } }]),
    )
    .await;
    let reader_metadata = reader_metadatas
        .first()
        .cloned()
        .expect("the submitter's registration reports its document");
    let read = get_document(
        &router,
        &reader_metadata,
        Some(&json!({ "bucket": DESTINATION })),
    )
    .await;
    assert_eq!(read.status, 200, "expected 200, got {read:?}");
    assert!(
        listed_keys(&router, DESTINATION).await.is_empty(),
        "a submission naming this service's own document bucket must hold no content, so its \
         download delivers nothing"
    );

    // As a DESTINATION: the key the other account's document occupies is
    // `<revision>/<index>`, which a prefix and a document name can spell, so this
    // submitter asks for its own document to be downloaded exactly there.
    let overwriter = "000000000553";
    let (prefix, name) = held_key
        .split_once('/')
        .expect("a held document's key is <revision>/<index>");
    let overwriter_metadatas = store_documents(
        &router,
        overwriter,
        &json!([
            { "file": { "fileName": name, "fileContent": base64::encode("someone else's bytes") } },
        ]),
    )
    .await;
    let overwriter_metadata = overwriter_metadatas
        .first()
        .cloned()
        .expect("the submitter's registration reports its document");
    let answer = get_document(
        &router,
        &overwriter_metadata,
        Some(&json!({ "bucket": held_bucket, "prefix": prefix })),
    )
    .await;
    assert_eq!(answer.status, 200, "expected 200, got {answer:?}");
    assert_eq!(
        object_bytes(&router, &held_bucket, &held_key)
            .await
            .as_deref(),
        Some(b"the other account's document".as_slice()),
        "a download must not write into this service's own document bucket, so {held_bucket}/\
         {held_key} must still carry the other account's content"
    );
}

/// A refused download delivers nothing: the metadata names no held document, so
/// there is nothing to write, and the destination the request named stays as it
/// was.
#[tokio::test]
async fn a_refused_download_delivers_nothing() {
    let (router, _engine) = emulator_with_objects();
    create_bucket(&router, DESTINATION).await;
    let answer: Answer = get_document(
        &router,
        &json!({
            "taxDocumentAccessToken": base64::encode("999999999999/1/0/certificate.pdf"),
            "taxDocumentName": "certificate.pdf",
        }),
        Some(&json!({ "bucket": DESTINATION })),
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "a token naming no held document must be refused, got {answer:?}"
    );
    assert!(
        listed_keys(&router, DESTINATION).await.is_empty(),
        "a refused download must deliver nothing"
    );
}
