// SPDX-License-Identifier: CC0-1.0

//! What `GetSigningPlatform` answers for the signing-platform catalog this
//! service publishes, end to end over HTTP.
//!
//! The rules under test, each over a TABLE rather than over one request:
//!
//! * a `platformId` the service publishes is reported with every member the
//!   model declares for it — the id it is published under, the partner, the
//!   target, the category, the algorithm and image-format options, the maximum
//!   payload size and whether its signatures can be revoked — and a platform's
//!   `defaultValue` / `defaultFormat` is always one of the values it allows;
//! * every member of a published row is PUBLISHED DATA, never derived from
//!   another member: a row's `partner` and `target` are whatever the catalog
//!   publishes for it, which need not be segments of its `platformId`, its
//!   `displayName` is served verbatim including any surrounding whitespace, its
//!   `category` is served as published even when the model's enum does not name
//!   that value, and a member the catalog publishes for no value at all is
//!   absent rather than filled in;
//! * a `platformId` the service publishes NO platform for is a 404
//!   `ResourceNotFoundException` naming that condition in the modelled `code`;
//! * a platform is a published fact, not a stored resource: reading one (or
//!   reading one that does not exist) changes nothing a later reader sees;
//! * the catalog is ONE catalog: the `displayName` a platform is published with
//!   is the `platformDisplayName` a profile created on it reports.
//!
//! The rows are the whole published catalog and a set of ids outside it, so they
//! differ along what the rules generalise over: the platform, its algorithms,
//! its image formats, its size bound, whether it supports revocation, whether it
//! publishes a display name, and how an unpublished id differs from a published
//! one (case, prefix, suffix, a platform the catalog never named).

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::ops::types::{
    ENCRYPTION_ALGORITHM_VALUES, IMAGE_FORMAT_VALUES,
};
use service_com_amazonaws_signer_wallabyservice::state::{
    ERROR_CODE_PLATFORM_NOT_FOUND, PLATFORM_CATEGORY_AWS, PLATFORM_CATEGORY_EXTERNAL,
    SIGNING_PLATFORM_CATALOG,
};
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: what routes a request to signer.
const SIGNED_FOR_US_WEST_2: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/signer/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// An ACM certificate ARN for `signingMaterial`, so a profile can be created on
/// every published platform, including the ones that require it.
const CERTIFICATE_ARN: &str = "arn:aws:acm:us-west-2:000000000000:certificate/abc";

/// `platformId`s this service publishes no platform for. The rows differ in HOW
/// they miss: a platform outside the published catalog, a near-miss of a
/// published id's algorithm suffix, a published id in the wrong case, a prefix of
/// a published id, and a published id with something appended.
const UNPUBLISHED_PLATFORM_IDS: &[&str] = &[
    "PrivatePlatform-SHA256-ECDSA",
    "Notation-OCI-SHA256-ECDSA",
    "awslambda-sha384-ecdsa",
    "AWSLambda",
    "AWSLambda-SHA384-ECDSA-v2",
];

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

    /// The modelled `message` member, or `None` when the body carries none.
    fn message(&self) -> Option<&str> {
        self.body["message"].as_str()
    }

    fn str_member(&self, member: &str) -> String {
        self.body[member].as_str().unwrap_or_default().to_string()
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

async fn get_platform(router: &MultiServiceRouter, platform_id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/signing-platforms/{platform_id}"),
        None,
    )
    .await
}

/// The string elements of a JSON array member, or `None` when the member is not
/// an array of strings.
fn string_list(value: &Value) -> Option<Vec<String>> {
    value.as_array().map(|items| {
        items
            .iter()
            .map(|item| item.as_str().unwrap_or_default().to_string())
            .collect()
    })
}

/// The algorithm a platform id spells out, if it does: the `SHA…` segment of an
/// id like `AWSIoTDeviceManagement-SHA256-ECDSA`, and the `ECDSA`/`RSA` one.
///
/// Source: `ENCRYPTION_ALGORITHM_VALUES` for the encryption segment; the hash
/// segment is whatever the id names, since an id can name an algorithm this
/// model's `HashAlgorithm` enum predates.
fn algorithms_named_by_id(platform_id: &str) -> (Option<&str>, Option<&str>) {
    let mut hash = None;
    let mut encryption = None;
    for segment in platform_id.split('-') {
        if segment.starts_with("SHA") && segment.len() > 3 {
            hash = Some(segment);
        }
        if ENCRYPTION_ALGORITHM_VALUES.contains(&segment) {
            encryption = Some(segment);
        }
    }
    (hash, encryption)
}

/// Every platform the service publishes is reported with the full modelled
/// contract, and the options it reports are internally consistent.
///
/// One row per published platform — they differ in algorithms, image formats,
/// size bound, revocation support and whether a display name is published.
#[tokio::test]
async fn every_published_platform_is_reported_with_its_full_contract() {
    let router = emulator();

    for entry in SIGNING_PLATFORM_CATALOG {
        let case = entry.platform_id;
        let got = get_platform(&router, case).await;
        assert_eq!(
            got.status, 200,
            "case {case:?}: expected GetSigningPlatform 200, got {} {:?}",
            got.status, got.body
        );

        assert_eq!(
            got.str_member("platformId"),
            case,
            "case {case:?}: expected the platform's own id, got {:?}",
            got.body["platformId"]
        );
        assert_eq!(
            got.body["displayName"].as_str(),
            entry.display_name,
            "case {case:?}: expected displayName {:?}, got {:?}",
            entry.display_name,
            got.body["displayName"]
        );
        assert_eq!(
            got.str_member("partner"),
            entry.partner,
            "case {case:?}: expected partner {:?}, got {:?}",
            entry.partner,
            got.body["partner"]
        );
        assert_eq!(
            got.str_member("target"),
            entry.target,
            "case {case:?}: expected target {:?}, got {:?}",
            entry.target,
            got.body["target"]
        );

        // The category the catalog publishes the platform in. The model's
        // `Category` enum names only the IoT one, which the catalog publishes no
        // platform in at all: it publishes `AWS` and `External` instead, so the
        // category is a per-platform fact served as published rather than clamped
        // to the enum.
        let category = got.str_member("category");
        assert_eq!(
            category, entry.category,
            "case {case:?}: expected category {:?}, got {:?}",
            entry.category, got.body["category"]
        );
        assert!(
            [PLATFORM_CATEGORY_AWS, PLATFORM_CATEGORY_EXTERNAL].contains(&category.as_str()),
            "case {case:?}: expected a category the service publishes, got {category:?}"
        );

        assert_eq!(
            got.body["maxSizeInMB"].as_i64(),
            Some(i64::from(entry.max_size_in_mb)),
            "case {case:?}: expected maxSizeInMB {}, got {:?}",
            entry.max_size_in_mb,
            got.body["maxSizeInMB"]
        );
        assert!(
            got.body["maxSizeInMB"].as_i64().is_some_and(|max| max > 0),
            "case {case:?}: expected a positive payload bound, got {:?}",
            got.body["maxSizeInMB"]
        );
        assert_eq!(
            got.body["revocationSupported"].as_bool(),
            Some(entry.revocation_supported),
            "case {case:?}: expected revocationSupported {}, got {:?}",
            entry.revocation_supported,
            got.body["revocationSupported"]
        );

        assert_option_set(
            case,
            "encryptionAlgorithmOptions",
            &got.body["signingConfiguration"]["encryptionAlgorithmOptions"],
            "allowedValues",
            "defaultValue",
            entry.encryption_algorithms,
        );
        assert_option_set(
            case,
            "hashAlgorithmOptions",
            &got.body["signingConfiguration"]["hashAlgorithmOptions"],
            "allowedValues",
            "defaultValue",
            entry.hash_algorithms,
        );
        // A platform the catalog publishes no image format for reports no
        // `signingImageFormat` member at all, rather than an empty one.
        if entry.image_formats.is_empty() {
            assert!(
                got.body["signingImageFormat"].is_null(),
                "case {case:?}: expected no signingImageFormat for a platform published without \
                 one, got {:?}",
                got.body["signingImageFormat"]
            );
        } else {
            assert_option_set(
                case,
                "signingImageFormat",
                &got.body["signingImageFormat"],
                "supportedFormats",
                "defaultFormat",
                entry.image_formats,
            );
        }

        // Every encryption algorithm and image format is one the model names;
        // the hash algorithm is whichever one the platform id spells out, which
        // can be one this model's enum predates.
        for algorithm in string_list(
            &got.body["signingConfiguration"]["encryptionAlgorithmOptions"]["allowedValues"],
        )
        .unwrap_or_default()
        {
            assert!(
                ENCRYPTION_ALGORITHM_VALUES.contains(&algorithm.as_str()),
                "case {case:?}: expected an encryption algorithm the model names \
                 ({ENCRYPTION_ALGORITHM_VALUES:?}), got {algorithm:?}"
            );
        }
        for format in string_list(&got.body["signingImageFormat"]["supportedFormats"])
            .unwrap_or_default()
        {
            assert!(
                IMAGE_FORMAT_VALUES.contains(&format.as_str()),
                "case {case:?}: expected an image format the model names \
                 ({IMAGE_FORMAT_VALUES:?}), got {format:?}"
            );
        }

        // A platform id that spells out its algorithms is signed under exactly
        // those: the reported defaults cannot contradict the published id.
        let (hash, encryption) = algorithms_named_by_id(case);
        if let Some(hash) = hash {
            assert_eq!(
                got.body["signingConfiguration"]["hashAlgorithmOptions"]["defaultValue"].as_str(),
                Some(hash),
                "case {case:?}: the id names {hash}, so that is the default hash algorithm; got \
                 {:?}",
                got.body["signingConfiguration"]["hashAlgorithmOptions"]
            );
        }
        if let Some(encryption) = encryption {
            assert_eq!(
                got.body["signingConfiguration"]["encryptionAlgorithmOptions"]["defaultValue"]
                    .as_str(),
                Some(encryption),
                "case {case:?}: the id names {encryption}, so that is the default encryption \
                 algorithm; got {:?}",
                got.body["signingConfiguration"]["encryptionAlgorithmOptions"]
            );
        }

        // A read reports a published fact and changes nothing: the next read of
        // the same platform answers identically.
        let again = get_platform(&router, case).await;
        assert_eq!(
            again.body, got.body,
            "case {case:?}: expected a platform to read back identically"
        );
    }
}

/// A list member and the default drawn from it: the default is one of the
/// allowed values, the list is non-empty, and both are what the catalog
/// publishes.
fn assert_option_set(
    case: &str,
    options: &str,
    body: &Value,
    list_member: &str,
    default_member: &str,
    expected: &[&str],
) {
    let values = string_list(&body[list_member]).unwrap_or_else(|| {
        panic!("case {case:?}: expected {options}.{list_member} to be a list, got {body:?}")
    });
    assert!(
        !values.is_empty(),
        "case {case:?}: expected {options}.{list_member} to name at least one value, got {body:?}"
    );
    assert_eq!(
        values,
        expected.iter().map(|value| (*value).to_string()).collect::<Vec<_>>(),
        "case {case:?}: expected {options}.{list_member} {expected:?}, got {body:?}"
    );
    let default = body[default_member].as_str().unwrap_or_default().to_string();
    assert!(
        values.contains(&default),
        "case {case:?}: expected {options}.{default_member} to be one of {values:?}, got \
         {default:?}"
    );
}

/// A `platformId` the service publishes no platform for is refused 404, naming
/// the condition in the modelled `code` — and the refusal creates nothing.
#[tokio::test]
async fn an_unpublished_platform_id_is_refused_as_not_found() {
    let router = emulator();

    for case in UNPUBLISHED_PLATFORM_IDS {
        let got = get_platform(&router, case).await;
        assert_eq!(
            got.status, 404,
            "case {case:?}: expected HTTP 404, got {} {:?}",
            got.status, got.body
        );
        assert_eq!(
            got.shape(),
            "ResourceNotFoundException",
            "case {case:?}: expected the modelled 404 shape, got {:?}",
            got.body
        );
        assert_eq!(
            got.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected x-amzn-errortype ResourceNotFoundException, got {:?}",
            got.error_type
        );
        assert_eq!(
            got.code(),
            Some(ERROR_CODE_PLATFORM_NOT_FOUND),
            "case {case:?}: expected the modelled code to name the condition, got {:?}",
            got.body
        );
        assert!(
            got.message().is_some_and(|message| !message.is_empty()),
            "case {case:?}: expected a message beside the code, got {:?}",
            got.body
        );

        // A read never publishes a platform: the second call is refused too.
        let again = get_platform(&router, case).await;
        assert_eq!(
            again.status, 404,
            "case {case:?}: expected the platform to still be unpublished, got {} {:?}",
            again.status, again.body
        );
    }
}

/// One catalog: the `displayName` a platform is published with is exactly the
/// `platformDisplayName` a profile created on that platform reports — including
/// for a platform published with no display name at all.
///
/// One row per published platform, each put with the certificate so the row holds
/// whether or not the platform requires it.
#[tokio::test]
async fn a_profiles_platform_display_name_is_the_one_the_platform_publishes() {
    let router = emulator();

    for (ordinal, entry) in SIGNING_PLATFORM_CATALOG.iter().enumerate() {
        let platform = entry.platform_id;
        let name = format!("Platform_Profile_{ordinal}");
        let put = call(
            &router,
            "PUT",
            &format!("/signing-profiles/{name}"),
            Some(json!({
                "platformId": platform,
                "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
            })),
        )
        .await;
        assert_eq!(
            put.status, 200,
            "case {platform:?}: expected PutSigningProfile 200, got {} {:?}",
            put.status, put.body
        );

        let profile = call(&router, "GET", &format!("/signing-profiles/{name}"), None).await;
        assert_eq!(
            profile.status, 200,
            "case {platform:?}: expected GetSigningProfile 200, got {} {:?}",
            profile.status, profile.body
        );
        let published = get_platform(&router, platform).await;
        assert_eq!(
            published.body["displayName"].as_str(),
            entry.display_name,
            "case {platform:?}: expected the platform to publish displayName {:?}, got {:?}",
            entry.display_name,
            published.body["displayName"]
        );
        match entry.display_name {
            // A platform published WITH a display name: both readers report that
            // exact string, so neither can have dropped the member.
            Some(display) => {
                assert_eq!(
                    profile.body["platformDisplayName"].as_str(),
                    Some(display),
                    "case {platform:?}: expected the profile's platformDisplayName to be the \
                     published displayName {display:?}, got {:?}",
                    profile.body["platformDisplayName"]
                );
            }
            // A platform published with NONE: the profile reports none either.
            None => {
                assert!(
                    profile.body["platformDisplayName"].is_null(),
                    "case {platform:?}: expected no platformDisplayName for a platform published \
                     without one, got {:?}",
                    profile.body["platformDisplayName"]
                );
            }
        }
        assert_eq!(
            profile.str_member("platformId"),
            published.str_member("platformId"),
            "case {platform:?}: expected the profile and the platform to agree on the id, got \
             {:?} and {:?}",
            profile.body["platformId"],
            published.body["platformId"]
        );
    }
}
