// SPDX-License-Identifier: CC0-1.0

//! The VALUES the signing-platform catalog publishes, and the position each row
//! is published at, end to end over HTTP.
//!
//! A signing platform is published by AWS Signer: the rows, their order and every
//! member of a row are facts the service states, so the emulator serves each row
//! at the position AWS publishes it in and with the exact members AWS publishes
//! for it, and every read that is computed from the catalog — the single-platform
//! reader, each `@httpQuery` filter, each page of the walk, and the signing
//! configuration a profile created on a row may select — is computed from those
//! same published values.
//!
//! The rules under test, each over the WHOLE published table rather than over one
//! row:
//!
//! * a listing reports the published rows in the published order, each with the
//!   published `displayName`, `partner`, `target`, `category`, algorithm options,
//!   image formats, size bound and revocation capability, and the
//!   single-platform reader answers the same row;
//! * each of `category`, `partner` and `target` selects exactly the published rows
//!   whose value for that member EQUALS the filter, in the published order, and a
//!   value no row publishes selects nothing;
//! * a page of any size, followed with the tokens the listing hands out, reports
//!   the published rows in the published order, so a client that indexes into a
//!   page sees the row AWS has at that index;
//! * a profile may select, through `overrides`, any encryption algorithm its
//!   platform publishes as allowed and no other, and it reports its platform's
//!   published `displayName` verbatim.
//!
//! The table below is the catalog itself, so the rows differ along everything the
//! rules generalise over: the partner, whether the partner/target pair decomposes
//! the platform id, the category, how many algorithms are allowed, which image
//! formats are published (including a row that publishes none), the size bound and
//! the revocation capability. It is written out by hand rather than read off the
//! crate's own table, so that a wrong value stored there fails here.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::ops::types::ENCRYPTION_ALGORITHM_VALUES;
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: what routes a request to signer.
const SIGNED_FOR_US_WEST_2: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/signer/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// An ACM certificate ARN for `signingMaterial`, so a profile can be created on
/// every published platform, including the ones that require it.
const CERTIFICATE_ARN: &str = "arn:aws:acm:us-west-2:000000000000:certificate/abc";

/// One row of the published catalog, with the members
/// `smithy_query(query="shape",shape="SigningPlatform")` declares for a platform.
struct Row {
    platform_id: &'static str,
    display_name: Option<&'static str>,
    partner: &'static str,
    target: &'static str,
    category: &'static str,
    /// `encryptionAlgorithmOptions.allowedValues`, most preferred first: the head
    /// is the published `defaultValue`.
    encryption_algorithms: &'static [&'static str],
    /// `hashAlgorithmOptions`, same convention.
    hash_algorithms: &'static [&'static str],
    /// `signingImageFormat.supportedFormats`, head first — empty for a row
    /// published with no `signingImageFormat` member at all.
    image_formats: &'static [&'static str],
    max_size_in_mb: i64,
    revocation_supported: bool,
}

/// The signing-platform catalog AWS Signer publishes, in the order
/// `ListSigningPlatforms` publishes it.
///
/// Source: the platforms AWS Signer's `ListSigningPlatforms` reports, member for
/// member as it words them — the member names and the `ImageFormat` value set are
/// the model's (`smithy_query(query="shape",shape="SigningPlatform")` and
/// `shape="ImageFormat"`), while which rows exist, what each publishes and the
/// order they come in are the service's published data, which the model states
/// nothing about. Two rows are published under a `category` this model's
/// `Category` enum does not name, and two publish a `target` that is a signing
/// template rather than a segment of the platform id.
const PUBLISHED_CATALOG: &[Row] = &[
    Row {
        platform_id: "AWSIoTDeviceManagement-SHA256-ECDSA",
        display_name: Some("AWS IoT Device Management SHA256-ECDSA "),
        partner: "AWSIoTDeviceManagement",
        target: "SHA256-ECDSA",
        category: "AWS",
        encryption_algorithms: &["ECDSA"],
        hash_algorithms: &["SHA256"],
        image_formats: &["JSONDetached"],
        max_size_in_mb: 2048,
        revocation_supported: false,
    },
    Row {
        platform_id: "Notation-OCI-SHA384-ECDSA",
        display_name: Some("Notation for container registries"),
        partner: "Notary",
        target: "OCI",
        category: "External",
        encryption_algorithms: &["ECDSA"],
        hash_algorithms: &["SHA384"],
        image_formats: &[],
        max_size_in_mb: 1,
        revocation_supported: true,
    },
    Row {
        platform_id: "AWSLambda-SHA384-ECDSA",
        display_name: Some("AWS Lambda"),
        partner: "AWSLambda",
        target: "SHA384-ECDSA",
        category: "AWS",
        encryption_algorithms: &["ECDSA"],
        hash_algorithms: &["SHA384"],
        image_formats: &["JSONDetached"],
        max_size_in_mb: 250,
        revocation_supported: true,
    },
    Row {
        platform_id: "AmazonFreeRTOS-TI-CC3220SF",
        display_name: Some("Amazon FreeRTOS SHA1-RSA CC3220SF-Format"),
        partner: "AmazonFreeRTOS",
        target: "SHA1-RSA-TISHA1",
        category: "AWS",
        encryption_algorithms: &["RSA"],
        hash_algorithms: &["SHA1"],
        image_formats: &["JSONEmbedded"],
        max_size_in_mb: 16,
        revocation_supported: false,
    },
    Row {
        platform_id: "AmazonFreeRTOS-Default",
        display_name: Some("Amazon FreeRTOS SHA256-ECDSA"),
        partner: "AmazonFreeRTOS",
        target: "SHA256-ECDSA",
        category: "AWS",
        encryption_algorithms: &["ECDSA", "RSA"],
        hash_algorithms: &["SHA256"],
        image_formats: &["JSONEmbedded"],
        max_size_in_mb: 16,
        revocation_supported: false,
    },
];

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: status and JSON body.
struct Answer {
    status: u16,
    body: Value,
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "signer.us-west-2.amazonaws.com")
        .header("authorization", SIGNED_FOR_US_WEST_2);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// One page of `ListSigningPlatforms` with the raw query string `query`.
async fn list(router: &MultiServiceRouter, case: &str, query: &str) -> Answer {
    let answer = call(router, "GET", &format!("/signing-platforms{query}"), None).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListSigningPlatforms 200 for {query:?}, got {} {:?}",
        answer.status, answer.body
    );
    answer
}

/// The rows a page reported, in the order it reported them.
fn rows_of(case: &str, answer: &Answer) -> Vec<Value> {
    answer.body["platforms"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a platforms list, got {:?}",
                answer.body
            )
        })
        .clone()
}

/// The platform ids a page reported, in order.
fn ids_of(case: &str, answer: &Answer) -> Vec<String> {
    rows_of(case, answer)
        .iter()
        .map(|row| row["platformId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The `SigningPlatform` body the catalog publishes for `row` — the expectation
/// both readers are held to, member for member.
fn published(row: &Row) -> Value {
    let mut expected = json!({
        "platformId": row.platform_id,
        "partner": row.partner,
        "target": row.target,
        "category": row.category,
        "signingConfiguration": {
            "encryptionAlgorithmOptions": {
                "allowedValues": row.encryption_algorithms,
                "defaultValue": row.encryption_algorithms[0],
            },
            "hashAlgorithmOptions": {
                "allowedValues": row.hash_algorithms,
                "defaultValue": row.hash_algorithms[0],
            },
        },
        "maxSizeInMB": row.max_size_in_mb,
        "revocationSupported": row.revocation_supported,
    });
    if let Some(display_name) = row.display_name {
        expected["displayName"] = json!(display_name);
    }
    if let Some(default_format) = row.image_formats.first() {
        expected["signingImageFormat"] = json!({
            "supportedFormats": row.image_formats,
            "defaultFormat": default_format,
        });
    }
    expected
}

/// Both readers report the published catalog: the whole page in the published
/// order, each row with exactly the published members.
#[tokio::test]
async fn the_catalog_is_published_in_order_with_its_published_values() {
    let router = emulator();
    let case = "the whole catalog";
    let page = list(&router, case, "?maxResults=25").await;
    let listed = rows_of(case, &page);

    assert_eq!(
        listed.len(),
        PUBLISHED_CATALOG.len(),
        "case {case:?}: expected the {} published rows, got {listed:?}",
        PUBLISHED_CATALOG.len()
    );
    assert!(
        page.body["nextToken"].is_null(),
        "case {case:?}: a page carrying the whole catalog must hand out no nextToken, got {:?}",
        page.body["nextToken"]
    );

    for (ordinal, row) in PUBLISHED_CATALOG.iter().enumerate() {
        let case = format!("{} at position {ordinal}", row.platform_id);
        let expected = published(row);
        assert_eq!(
            listed[ordinal], expected,
            "case {case:?}: expected the published row {expected:?}, got {:?}",
            listed[ordinal]
        );

        let got = call(
            &router,
            "GET",
            &format!("/signing-platforms/{}", row.platform_id),
            None,
        )
        .await;
        assert_eq!(
            got.status, 200,
            "case {case:?}: expected GetSigningPlatform 200, got {} {:?}",
            got.status, got.body
        );
        assert_eq!(
            got.body, expected,
            "case {case:?}: expected the get to answer the published row {expected:?}, got {:?}",
            got.body
        );
    }
}

/// One filter row: the member filtered on and the value, plus whether any
/// published row is expected to carry it (so a filter that selects nothing
/// everywhere cannot pass).
struct FilterCase {
    member: &'static str,
    value: &'static str,
    selects_rows: bool,
}

/// The filter rows: every value the published table carries for each filterable
/// member, plus values no row publishes — the only value this model's `Category`
/// enum names (`AWSIoT`), and the id suffixes a reader that split the platform id
/// would publish as a `target` instead of the signing template.
fn filter_cases() -> Vec<FilterCase> {
    let mut cases = Vec::new();
    for row in PUBLISHED_CATALOG {
        for (member, value) in [
            ("category", row.category),
            ("partner", row.partner),
            ("target", row.target),
        ] {
            if !cases
                .iter()
                .any(|case: &FilterCase| case.member == member && case.value == value)
            {
                cases.push(FilterCase {
                    member,
                    value,
                    selects_rows: true,
                });
            }
        }
    }
    for (member, value) in [
        ("category", "AWSIoT"),
        ("target", "Default"),
        ("target", "TI-CC3220SF"),
        ("partner", "Notation"),
    ] {
        cases.push(FilterCase {
            member,
            value,
            selects_rows: false,
        });
    }
    cases
}

/// Each filter selects exactly the published rows whose value for that member
/// equals it, in the published order.
#[tokio::test]
async fn a_filter_selects_the_published_rows_that_carry_its_value() {
    let router = emulator();

    for row in filter_cases() {
        let case = format!("{}={}", row.member, row.value);
        let expected: Vec<String> = PUBLISHED_CATALOG
            .iter()
            .filter(|published| match row.member {
                "category" => published.category == row.value,
                "partner" => published.partner == row.value,
                _ => published.target == row.value,
            })
            .map(|published| published.platform_id.to_string())
            .collect();
        assert_eq!(
            !expected.is_empty(),
            row.selects_rows,
            "case {case:?}: the table expects this filter to select {} rows, got {expected:?}",
            if row.selects_rows { "some" } else { "no" }
        );

        let page = list(
            &router,
            &case,
            &format!("?maxResults=25&{}={}", row.member, row.value),
        )
        .await;
        let ids = ids_of(&case, &page);
        assert_eq!(
            ids, expected,
            "case {case:?}: expected {expected:?} in the published order, got {ids:?}"
        );
    }
}

/// Walking the catalog with the tokens a page hands out reports the published rows
/// in the published order, whatever the page size — so a client that indexes into a
/// page sees the row the catalog publishes at that index.
#[tokio::test]
async fn paging_the_catalog_keeps_the_published_order() {
    let router = emulator();

    let expected: Vec<String> = PUBLISHED_CATALOG
        .iter()
        .map(|row| row.platform_id.to_string())
        .collect();

    // Every page size from the modelled floor (`@range(min: 1, max: 25)`) to one
    // past the catalog, plus the modelled ceiling: a page smaller than, equal to
    // and larger than the catalog it cuts.
    let mut page_sizes: Vec<usize> = (1..=PUBLISHED_CATALOG.len() + 1).collect();
    page_sizes.push(25);

    for page_size in page_sizes {
        let case = format!("the catalog at {page_size} per page");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // One page per row at worst, plus one: a walk that does not finish inside
        // that many pages is reporting something twice.
        for _ in 0..=PUBLISHED_CATALOG.len() {
            let query = match &token {
                Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
                None => format!("?maxResults={page_size}"),
            };
            let page = list(&router, &case, &query).await;
            let ids = ids_of(&case, &page);
            assert!(
                ids.len() <= page_size,
                "case {case:?}: expected at most {page_size} rows per page, got {ids:?}"
            );
            walked.extend(ids);
            token = page.body["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case {case:?}: expected the walk to end, got {walked:?} and another token"
        );
        assert_eq!(
            walked, expected,
            "case {case:?}: expected the published order {expected:?}, got {walked:?}"
        );
    }
}

/// A profile selects, through `overrides`, among the encryption algorithms its
/// platform publishes as allowed: each published value is accepted and reads back,
/// and an expressible value the row does not publish is refused 400 — and the
/// profile reports its platform's published display name verbatim.
#[tokio::test]
async fn a_profile_selects_among_the_algorithms_its_row_publishes() {
    let router = emulator();

    for (ordinal, row) in PUBLISHED_CATALOG.iter().enumerate() {
        // `EncryptionAlgorithm` is what a request can carry at all
        // (`crate::ops::types::ENCRYPTION_ALGORITHM_VALUES`), so the rows below
        // partition the expressible values into the ones this platform publishes
        // and the ones it does not.
        for algorithm in ENCRYPTION_ALGORITHM_VALUES {
            let allowed = row.encryption_algorithms.contains(algorithm);
            let case = format!("{} overriding to {algorithm}", row.platform_id);
            let name = format!("Catalog_Row_{ordinal}_{algorithm}");
            let put = call(
                &router,
                "PUT",
                &format!("/signing-profiles/{name}"),
                Some(json!({
                    "platformId": row.platform_id,
                    "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
                    "overrides": {
                        "signingConfiguration": { "encryptionAlgorithm": algorithm },
                    },
                })),
            )
            .await;

            if !allowed {
                assert_eq!(
                    put.status, 400,
                    "case {case:?}: the row publishes {:?}, so this override must be refused, \
                     got {} {:?}",
                    row.encryption_algorithms, put.status, put.body
                );
                let read_back =
                    call(&router, "GET", &format!("/signing-profiles/{name}"), None).await;
                assert_eq!(
                    read_back.status, 404,
                    "case {case:?}: a refused create must store no profile, got {} {:?}",
                    read_back.status, read_back.body
                );
                continue;
            }

            assert_eq!(
                put.status, 200,
                "case {case:?}: the row publishes {:?}, so this override must be accepted, \
                 got {} {:?}",
                row.encryption_algorithms, put.status, put.body
            );
            let read_back = call(&router, "GET", &format!("/signing-profiles/{name}"), None).await;
            assert_eq!(
                read_back.status, 200,
                "case {case:?}: expected the created profile to be readable, got {} {:?}",
                read_back.status, read_back.body
            );
            assert_eq!(
                read_back.body["overrides"]["signingConfiguration"]["encryptionAlgorithm"].as_str(),
                Some(*algorithm),
                "case {case:?}: expected the selected algorithm to read back, got {:?}",
                read_back.body["overrides"]
            );
            assert_eq!(
                read_back.body["platformDisplayName"].as_str(),
                row.display_name,
                "case {case:?}: expected the published displayName {:?}, got {:?}",
                row.display_name,
                read_back.body["platformDisplayName"]
            );
        }
    }
}
