// SPDX-License-Identifier: CC0-1.0

//! What `PutSigningProfile` does with `overrides.signingImageFormat`, end to end
//! over HTTP.
//!
//! `SigningPlatformOverrides$signingImageFormat` documents its OWN option set
//! rather than taking the platform's: "when overriding the default signing
//! platform configuration, a customer can select either of two signing formats,
//! `JSONEmbedded` or `JSONDetached`", the third value the modelled `ImageFormat`
//! enum names being reserved for future use. So the rule under test:
//!
//! * an image-format override naming a format a customer may select is stored and
//!   reported back — on EVERY published platform, whatever that platform publishes
//!   as its own `supportedFormats`;
//! * an image-format override naming any other value the enum admits is refused
//!   400 `ValidationException`, naming the member, the value and the formats it may
//!   carry, and stores no profile — again on every published platform, including
//!   any that publishes that format itself;
//! * so no signing job can ever snapshot an image format a customer may not
//!   select, because no profile holds one.
//!
//! The rows are generated from the published catalog
//! (`crate::state::SIGNING_PLATFORM_CATALOG`) crossed with the WHOLE modelled enum
//! (`crate::ops::types::IMAGE_FORMAT_VALUES`), so both verdicts are asserted for
//! every platform and no row is keyed to one request.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::ops::types::{
    ENCRYPTION_ALGORITHM_VALUES, HASH_ALGORITHM_VALUES, IMAGE_FORMAT_VALUES,
};
use service_com_amazonaws_signer_wallabyservice::state::{
    SigningWorkflow, ERROR_CODE_VALIDATION, SELECTABLE_IMAGE_FORMATS, SIGNING_PLATFORM_CATALOG,
};
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: what routes a request to signer.
const SIGNED_FOR_US_WEST_2: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/signer/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// An ACM certificate ARN for `signingMaterial`, carried by every put below so no
/// create is refused for the certificate instead of for the override under test.
const CERTIFICATE_ARN: &str = "arn:aws:acm:us-west-2:000000000000:certificate/abc";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: status, JSON body, and the `x-amzn-errortype`
/// header restJson1 resolves an error SHAPE from.
struct Answer {
    status: u16,
    body: Value,
    error_type: Option<String>,
}

impl Answer {
    /// The wire error shape (`__type`), or the empty string on a success.
    fn shape(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The modelled `code` member, or `None` when the body carries none.
    fn code(&self) -> Option<&str> {
        self.body["code"].as_str()
    }

    /// The modelled `message` member, or the empty string when the body carries
    /// none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }
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
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
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
    Answer {
        status,
        body,
        error_type,
    }
}

async fn put_profile(router: &MultiServiceRouter, name: &str, request: Value) -> Answer {
    call(
        router,
        "PUT",
        &format!("/signing-profiles/{name}"),
        Some(request),
    )
    .await
}

async fn get_profile(router: &MultiServiceRouter, name: &str) -> Answer {
    call(router, "GET", &format!("/signing-profiles/{name}"), None).await
}

async fn start_job(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/signing-jobs", Some(request)).await
}

async fn describe_job(router: &MultiServiceRouter, job_id: &str) -> Answer {
    call(router, "GET", &format!("/signing-jobs/{job_id}"), None).await
}

/// A create naming `platform_id` and `overrides`, with the certificate every
/// platform's profile may carry.
fn create_request(platform_id: &str, overrides: Value) -> Value {
    json!({
        "platformId": platform_id,
        "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
        "overrides": overrides,
    })
}

/// The refusal a value outside the member's own option set must carry: the
/// modelled 400 shape and code, a message naming the member, the value and every
/// format a customer may select — and no stored profile.
async fn assert_refused_for_the_image_format(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    answer: &Answer,
    value: &str,
) {
    assert_eq!(
        answer.status, 400,
        "case {case}: expected HTTP 400, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.shape(),
        "ValidationException",
        "case {case}: expected the modelled 400 shape, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.error_type.as_deref(),
        Some("ValidationException"),
        "case {case}: expected x-amzn-errortype ValidationException, got {:?}",
        answer.error_type
    );
    assert_eq!(
        answer.code(),
        Some(ERROR_CODE_VALIDATION),
        "case {case}: expected the modelled code, got {:?}",
        answer.body
    );
    for expected in ["signingImageFormat", value] {
        assert!(
            answer.message().contains(expected),
            "case {case}: expected the refusal to name {expected:?}, got {:?}",
            answer.body["message"]
        );
    }
    for selectable in SELECTABLE_IMAGE_FORMATS {
        assert!(
            answer.message().contains(selectable),
            "case {case}: expected the refusal to name the selectable format {selectable:?}, got \
             {:?}",
            answer.body["message"]
        );
    }

    let read_back = get_profile(router, name).await;
    assert_eq!(
        read_back.status, 404,
        "case {case}: a refused create must store no profile, got {} {:?}",
        read_back.status, read_back.body
    );
}

/// An `overrides.signingImageFormat` a customer may select is stored and read
/// back; every other value the modelled enum admits is refused 400
/// `ValidationException` and stores nothing — on every published platform, whether
/// or not that platform publishes the format in its own `supportedFormats`.
#[tokio::test]
async fn an_image_format_override_must_be_one_a_customer_may_select() {
    let router = emulator();

    let (mut accepted_rows, mut refused_rows) = (0usize, 0usize);
    // The two halves of the platform-independence claim: a format a customer may
    // select is accepted even by a platform that does not publish it, and one a
    // customer may not select is refused even by a platform that does — the second
    // half only where the published catalog holds such a platform at all, which is
    // why it is checked against the catalog below rather than assumed.
    let (mut accepted_unpublished, mut refused_published) = (0usize, 0usize);

    for (platform_ordinal, entry) in SIGNING_PLATFORM_CATALOG.iter().enumerate() {
        let platform = entry.platform_id;
        let (mut accepted_here, mut refused_here) = (0usize, 0usize);

        for (ordinal, format) in IMAGE_FORMAT_VALUES.iter().enumerate() {
            let published = entry.image_formats.contains(format);
            let case = format!(
                "{platform}: signingImageFormat {format} (platform publishes it: {published})"
            );
            let name = format!("Image_Format_{platform_ordinal}_{ordinal}");
            let overrides = json!({ "signingImageFormat": format });
            let answer = put_profile(&router, &name, create_request(platform, overrides.clone()))
                .await;

            if SELECTABLE_IMAGE_FORMATS.contains(format) {
                accepted_rows += 1;
                accepted_here += 1;
                if !published {
                    accepted_unpublished += 1;
                }
                assert_eq!(
                    answer.status, 200,
                    "case {case}: expected PutSigningProfile 200, got {} {:?}",
                    answer.status, answer.body
                );
                let read_back = get_profile(&router, &name).await;
                assert_eq!(
                    read_back.status, 200,
                    "case {case}: expected the created profile to be readable, got {} {:?}",
                    read_back.status, read_back.body
                );
                assert_eq!(
                    read_back.body["overrides"], overrides,
                    "case {case}: expected the overrides to round-trip as {overrides:?}, got {:?}",
                    read_back.body["overrides"]
                );
            } else {
                refused_rows += 1;
                refused_here += 1;
                if published {
                    refused_published += 1;
                }
                assert_refused_for_the_image_format(&router, &case, &name, &answer, format).await;
            }
        }

        assert!(
            accepted_here > 0 && refused_here > 0,
            "case {platform:?}: expected both accepted and refused image-format rows, got \
             {accepted_here} accepted and {refused_here} refused"
        );
    }

    assert!(
        accepted_rows > 0 && refused_rows > 0,
        "expected the table to carry rows of both verdicts, got {accepted_rows} accepted and \
         {refused_rows} refused"
    );
    assert!(
        accepted_unpublished > 0,
        "expected the table to pin that the option set is the member's own, not the platform's: \
         got {accepted_unpublished} accepted rows for a format the platform does not publish"
    );
    // The other half holds exactly where the catalog publishes a format a customer
    // may not select: every such row must have been refused, and a catalog that
    // publishes none of them cannot show a refusal of one.
    let published_unselectable = SIGNING_PLATFORM_CATALOG
        .iter()
        .flat_map(|entry| entry.image_formats)
        .filter(|format| !SELECTABLE_IMAGE_FORMATS.contains(format))
        .count();
    assert_eq!(
        refused_published, published_unselectable,
        "expected every published format a customer may not select to be refused: the catalog \
         publishes {published_unselectable} of them and {refused_published} were refused"
    );
}

/// The image-format override is judged beside, and independently of, the
/// signing-configuration override: a selectable format rides along with allowed
/// algorithms, a non-selectable one refuses the create even with allowed
/// algorithms, and an algorithm outside the platform's `allowedValues` is still
/// what the refusal names when both members are wrong.
#[tokio::test]
async fn an_image_format_override_is_judged_beside_the_signing_configuration() {
    let router = emulator();

    let mut rows = 0usize;
    for (platform_ordinal, entry) in SIGNING_PLATFORM_CATALOG.iter().enumerate() {
        let platform = entry.platform_id;
        // Only the values the MODEL's enums can carry reach the handler: a
        // platform publishing SHA384 publishes a value `HashAlgorithm` predates,
        // so the boundary validator would refuse it for the modelled constraint
        // first (the same filtering `put_profile_platform_validation` does).
        let allowed_hash = HASH_ALGORITHM_VALUES
            .iter()
            .find(|value| entry.hash_algorithms.contains(value));
        let outside_hash = HASH_ALGORITHM_VALUES
            .iter()
            .find(|value| !entry.hash_algorithms.contains(value));
        let allowed_encryption = ENCRYPTION_ALGORITHM_VALUES
            .iter()
            .find(|value| entry.encryption_algorithms.contains(value));
        let selectable = SELECTABLE_IMAGE_FORMATS
            .first()
            .expect("the member publishes selectable formats");
        let unselectable = IMAGE_FORMAT_VALUES
            .iter()
            .find(|value| !SELECTABLE_IMAGE_FORMATS.contains(value))
            .expect("the modelled enum is wider than the selectable set");

        if let (Some(hash), Some(encryption)) = (allowed_hash, allowed_encryption) {
            // Allowed algorithms + a selectable format: both members stored.
            let case = format!("{platform}: allowed {hash}/{encryption} with {selectable}");
            let name = format!("Image_Format_With_Config_{platform_ordinal}");
            let overrides = json!({
                "signingConfiguration": {
                    "hashAlgorithm": hash,
                    "encryptionAlgorithm": encryption,
                },
                "signingImageFormat": selectable,
            });
            rows += 1;
            let answer =
                put_profile(&router, &name, create_request(platform, overrides.clone())).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected PutSigningProfile 200, got {} {:?}",
                answer.status, answer.body
            );
            let read_back = get_profile(&router, &name).await;
            assert_eq!(
                read_back.body["overrides"], overrides,
                "case {case}: expected the overrides to round-trip as {overrides:?}, got {:?}",
                read_back.body["overrides"]
            );

            // Allowed algorithms + a format a customer may not select: refused for
            // the image format, nothing stored.
            let case = format!("{platform}: allowed {hash}/{encryption} with {unselectable}");
            let name = format!("Image_Format_Bad_With_Config_{platform_ordinal}");
            rows += 1;
            let answer = put_profile(
                &router,
                &name,
                create_request(
                    platform,
                    json!({
                        "signingConfiguration": {
                            "hashAlgorithm": hash,
                            "encryptionAlgorithm": encryption,
                        },
                        "signingImageFormat": unselectable,
                    }),
                ),
            )
            .await;
            assert_refused_for_the_image_format(&router, &case, &name, &answer, unselectable).await;
        }

        if let Some(hash) = outside_hash {
            // An algorithm outside the platform's published options is judged
            // first, whichever image format rides along with it.
            for (ordinal, format) in [*selectable, *unselectable].into_iter().enumerate() {
                let case =
                    format!("{platform}: hashAlgorithm {hash} outside allowedValues with {format}");
                let name = format!("Image_Format_Outside_Config_{platform_ordinal}_{ordinal}");
                rows += 1;
                let answer = put_profile(
                    &router,
                    &name,
                    create_request(
                        platform,
                        json!({
                            "signingConfiguration": { "hashAlgorithm": hash },
                            "signingImageFormat": format,
                        }),
                    ),
                )
                .await;
                assert_eq!(
                    answer.status, 400,
                    "case {case}: expected HTTP 400, got {} {:?}",
                    answer.status, answer.body
                );
                assert!(
                    answer.message().contains("hashAlgorithm")
                        && answer.message().contains(*hash),
                    "case {case}: expected the refusal to name the signing-configuration member \
                     and its value, got {:?}",
                    answer.body["message"]
                );
                let read_back = get_profile(&router, &name).await;
                assert_eq!(
                    read_back.status, 404,
                    "case {case}: a refused create must store no profile, got {} {:?}",
                    read_back.status, read_back.body
                );
            }
        }
    }

    assert!(
        rows > 0,
        "expected the table to carry rows combining both override members, got {rows}"
    );
}

/// A value the modelled `ImageFormat` enum does not name at all is refused before
/// the handler ever sees it, and stores nothing — so the member admits exactly the
/// selectable formats and nothing else, in any spelling.
#[tokio::test]
async fn a_value_outside_the_modelled_enum_is_refused_too() {
    let router = emulator();
    let platform = SIGNING_PLATFORM_CATALOG
        .first()
        .expect("the catalog publishes platforms")
        .platform_id;

    // Each row differs in HOW it misses the enum: the case of a selectable value,
    // a selectable value with trailing space, a separator inserted, the empty
    // string, and a value from the sibling member's enum.
    for (ordinal, value) in ["jsondetached", "JSONDetached ", "JSON_Embedded", "", "SHA256"]
        .into_iter()
        .enumerate()
    {
        let case = format!("{platform}: signingImageFormat {value:?}");
        let name = format!("Image_Format_Unmodelled_{ordinal}");
        let answer = put_profile(
            &router,
            &name,
            create_request(platform, json!({ "signingImageFormat": value })),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400, got {} {:?}",
            answer.status, answer.body
        );
        let read_back = get_profile(&router, &name).await;
        assert_eq!(
            read_back.status, 404,
            "case {case}: a refused create must store no profile, got {} {:?}",
            read_back.status, read_back.body
        );
    }
}

/// What a signing job snapshots follows from what a profile can hold: a job takes
/// the selectable image format its profile was created with, and no job can carry
/// a format a customer may not select because no such profile exists to start one
/// from.
#[tokio::test]
async fn a_signing_job_snapshots_only_a_selectable_image_format() {
    let router = emulator();

    let mut jobs = 0usize;
    for (ordinal, entry) in SIGNING_PLATFORM_CATALOG
        .iter()
        .filter(|entry| entry.workflow == SigningWorkflow::CodeSigningJob)
        .enumerate()
    {
        let platform = entry.platform_id;
        for (format_ordinal, format) in SELECTABLE_IMAGE_FORMATS.iter().enumerate() {
            let case = format!("{platform}: job from a profile overriding to {format}");
            let name = format!("Image_Format_Job_{ordinal}_{format_ordinal}");
            let overrides = json!({ "signingImageFormat": format });
            let put = put_profile(&router, &name, create_request(platform, overrides.clone())).await;
            assert_eq!(
                put.status, 200,
                "case {case}: seeding the profile must succeed, got {} {:?}",
                put.status, put.body
            );

            let started = start_job(
                &router,
                json!({
                    "source": { "s3": { "bucketName": "src", "key": "code.bin", "version": "1" } },
                    "destination": { "s3": { "bucketName": "dst" } },
                    "profileName": name,
                    "clientRequestToken": format!("tok-{name}"),
                }),
            )
            .await;
            assert_eq!(
                started.status, 200,
                "case {case}: expected StartSigningJob 200, got {} {:?}",
                started.status, started.body
            );
            let job_id = started.body["jobId"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                !job_id.is_empty(),
                "case {case}: expected a jobId, got {:?}",
                started.body
            );

            let described = describe_job(&router, &job_id).await;
            assert_eq!(
                described.status, 200,
                "case {case}: expected DescribeSigningJob 200, got {} {:?}",
                described.status, described.body
            );
            assert_eq!(
                described.body["overrides"], overrides,
                "case {case}: expected the job to snapshot {overrides:?}, got {:?}",
                described.body["overrides"]
            );
            jobs += 1;
        }

        // The other half: a profile a customer cannot create is a profile no job
        // can be started from, so the format the member does not admit never
        // reaches a job at all.
        for (format_ordinal, format) in IMAGE_FORMAT_VALUES
            .iter()
            .filter(|value| !SELECTABLE_IMAGE_FORMATS.contains(value))
            .enumerate()
        {
            let case = format!("{platform}: job from a profile overriding to {format}");
            let name = format!("Image_Format_No_Job_{ordinal}_{format_ordinal}");
            let put = put_profile(
                &router,
                &name,
                create_request(platform, json!({ "signingImageFormat": format })),
            )
            .await;
            assert_eq!(
                put.status, 400,
                "case {case}: expected the create to be refused 400, got {} {:?}",
                put.status, put.body
            );

            let started = start_job(
                &router,
                json!({
                    "source": { "s3": { "bucketName": "src", "key": "code.bin", "version": "1" } },
                    "destination": { "s3": { "bucketName": "dst" } },
                    "profileName": name,
                    "clientRequestToken": format!("tok-{name}"),
                }),
            )
            .await;
            assert_eq!(
                started.status, 404,
                "case {case}: expected no profile to start a job from, got {} {:?}",
                started.status, started.body
            );
        }
    }

    assert!(
        jobs > 0,
        "expected the suite to describe signing jobs for every selectable format, got {jobs}"
    );
}
