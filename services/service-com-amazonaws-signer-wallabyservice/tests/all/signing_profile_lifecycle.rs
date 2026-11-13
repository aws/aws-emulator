// SPDX-License-Identifier: CC0-1.0

//! The AWS Signer signing-profile and signing-job slice, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted record and serialization — the same
//! path a client takes. The rules under test are the ones Signer states for these
//! two resources:
//!
//! * a profile's members, version, status and ARN are whatever the
//!   `PutSigningProfile` that created it stored — the name it was created under
//!   stays taken, so no later create replaces it — and every later read computes
//!   its answer from that record;
//! * a job's platform, profile version, signing material, source and destination
//!   are the ones SNAPSHOTTED when it started, whatever the profile does later.
//!
//! The cases are tables: a row per input class the rules generalise over (member
//! count, name length, validity-period unit, published platform, page size,
//! filter).

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::ops::types::{
    SIGNING_PROFILE_STATUS_VALUES, SIGNING_STATUS_VALUES, VALIDITY_TYPE_VALUES,
};
use service_com_amazonaws_signer_wallabyservice::state::JOB_COMPLETION_SPAN;
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service. It is what routes a request to
/// signer, and its region is what a profile ARN is built for.
const SIGNED_FOR_US_WEST_2: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/signer/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The region [`SIGNED_FOR_US_WEST_2`] signs for.
const REGION: &str = "us-west-2";

/// The IAM principal the emulator resolves every signed request as: the one IAM
/// user whose credentials it publishes, which is the identity
/// `sts:GetCallerIdentity` answers with and what a job records as having
/// requested it.
fn caller_principal() -> String {
    format!("arn:aws:iam::{ACCOUNT_ID}:user/emulator")
}

/// The account id every generated emulator presents (`EMULATOR_ACCOUNT_ID`),
/// spelled out here rather than imported so a wrong account in the handlers fails.
const ACCOUNT_ID: &str = "000000000000";

/// The largest page `ListSigningProfilesRequest$maxResults` admits
/// (`@range(min: 1, max: 25)`), spelled out here for the same reason.
const MAX_RESULTS_CEILING: i32 = 25;

/// `ProfileVersion`'s `@length(10, 10)` and `^[a-zA-Z0-9]{10}$`, checked here
/// rather than through any crate helper so a wrong mint cannot make this pass.
fn matches_profile_version_pattern(value: &str) -> bool {
    value.len() == 10 && value.chars().all(|c| c.is_ascii_alphanumeric())
}

/// An ACM certificate ARN for `signingMaterial`: the certificate a profile's
/// signatures are issued under. A put below carries it whenever its platform is
/// one whose profile holds that certificate — see the published
/// `requires_signing_material` of `crate::state::SIGNING_PLATFORM_CATALOG`.
const CERTIFICATE_ARN: &str = "arn:aws:acm:us-west-2:000000000000:certificate/abc";

/// `signingMaterial` naming [`CERTIFICATE_ARN`].
fn signing_material() -> Value {
    json!({ "certificateArn": CERTIFICATE_ARN })
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// The same emulator with Amazon S3 mounted beside signer on the one shared
/// engine — what a case that starts a signing job needs, because the job reads
/// the object it signs out of S3 and writes the signed artifact back there.
fn emulator_with_object_storage() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    super::code_signing_objects::mount_object_storage(&mut router, &engine);
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
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

async fn list_profiles(router: &MultiServiceRouter, query: &str) -> Answer {
    call(router, "GET", &format!("/signing-profiles{query}"), None).await
}

async fn cancel_profile(router: &MultiServiceRouter, name: &str) -> Answer {
    call(router, "DELETE", &format!("/signing-profiles/{name}"), None).await
}

async fn start_job(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/signing-jobs", Some(request)).await
}

async fn describe_job(router: &MultiServiceRouter, job_id: &str) -> Answer {
    call(router, "GET", &format!("/signing-jobs/{job_id}"), None).await
}

/// The profile names `ListSigningProfiles` reported, in the order it reported them.
fn listed_names(answer: &Answer) -> Vec<String> {
    answer.body["profiles"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "ListSigningProfiles must return a profiles list: {:?}",
                answer.body
            )
        })
        .iter()
        .map(|profile| {
            profile["profileName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Every profile name a paged walk of `ListSigningProfiles` reports, following
/// `nextToken` until it stops coming.
async fn walk_profiles(router: &MultiServiceRouter, filters: &str, page_size: i32) -> Vec<String> {
    let mut names = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..64 {
        let query = match token.as_deref() {
            Some(token) => format!("?maxResults={page_size}{filters}&nextToken={token}"),
            None => format!("?maxResults={page_size}{filters}"),
        };
        let page = list_profiles(router, &query).await;
        assert_eq!(
            page.status, 200,
            "page {query}: expected 200, got {} {:?}",
            page.status, page.body
        );
        let reported = listed_names(&page);
        assert!(
            reported.len() <= page_size as usize,
            "page {query}: expected at most {page_size} profiles, got {reported:?}"
        );
        names.extend(reported);
        match page.body["nextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return names,
        }
    }
    panic!("ListSigningProfiles paged more than 64 times for {filters:?}");
}

/// A profile shape to put and read back. The rows differ along what the rule
/// generalises over: how many optional members the request carries, the length and
/// form of the name, the validity-period unit, and which published platform the
/// profile is bound to (including the one published in a category of its own).
struct ProfileCase {
    name: &'static str,
    /// The request body, minus nothing: exactly what the client sends.
    request: Value,
    /// The display name the platform catalog publishes for this row's platform,
    /// or `None` for a platform outside it.
    platform_display_name: Option<&'static str>,
}

fn profile_cases() -> Vec<ProfileCase> {
    vec![
        // The smallest request a code-signing profile admits: `platformId` alone,
        // and a name at `ProfileName`'s `@length` minimum of 2.
        ProfileCase {
            name: "Ab",
            request: json!({ "platformId": "AWSLambda-SHA384-ECDSA" }),
            platform_display_name: Some("AWS Lambda"),
        },
        // A CONTAINER-IMAGE profile, which carries the certificate its signatures
        // are issued under.
        ProfileCase {
            name: "Container_Profile",
            request: json!({
                "platformId": "Notation-OCI-SHA384-ECDSA",
                "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
            }),
            platform_display_name: Some("Notation for container registries"),
        },
        // Signing material, a DAYS validity period, parameters and tags.
        ProfileCase {
            name: "MyProfile_1",
            request: json!({
                "platformId": "AWSIoTDeviceManagement-SHA256-ECDSA",
                "signingMaterial": {
                    "certificateArn":
                        "arn:aws:acm:us-west-2:000000000000:certificate/abc",
                },
                "signatureValidityPeriod": { "value": 30, "type": "DAYS" },
                "signingParameters": { "env": "prod", "team": "iot" },
                "tags": { "owner": "platform", "tier": "gold" },
            }),
            platform_display_name: Some("AWS IoT Device Management SHA256-ECDSA "),
        },
        // A name at `ProfileName`'s `@length` maximum of 64, platform overrides, and
        // a MONTHS validity period.
        ProfileCase {
            name: "A_profile_name_that_is_exactly_sixty_four_characters_long_______",
            request: json!({
                "platformId": "AmazonFreeRTOS-TI-CC3220SF",
                "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
                "signatureValidityPeriod": { "value": 135, "type": "MONTHS" },
                "overrides": {
                    "signingConfiguration": {
                        "encryptionAlgorithm": "RSA",
                        "hashAlgorithm": "SHA1",
                    },
                    "signingImageFormat": "JSONDetached",
                },
            }),
            platform_display_name: Some("Amazon FreeRTOS SHA1-RSA CC3220SF-Format"),
        },
        // A YEARS period, one tag, and overrides naming exactly the algorithms
        // this platform publishes as allowed.
        ProfileCase {
            name: "FreeRTOS_Profile_9",
            request: json!({
                "platformId": "AmazonFreeRTOS-Default",
                "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
                "signatureValidityPeriod": { "value": 2, "type": "YEARS" },
                "overrides": {
                    "signingConfiguration": {
                        "encryptionAlgorithm": "ECDSA",
                        "hashAlgorithm": "SHA256",
                    },
                },
                "tags": { "only": "one" },
            }),
            platform_display_name: Some("Amazon FreeRTOS SHA256-ECDSA"),
        },
    ]
}

/// Every member a `PutSigningProfile` carries comes back from
/// `GetSigningProfile` as submitted, with the identity the put minted.
#[tokio::test]
async fn put_then_get_round_trips_every_profile_shape() {
    let router = emulator();

    for case in profile_cases() {
        let put = put_profile(&router, case.name, case.request.clone()).await;
        assert_eq!(
            put.status, 200,
            "case {}: expected PutSigningProfile 200, got {} {:?}",
            case.name, put.status, put.body
        );

        let version = put.str_member("profileVersion");
        assert!(
            matches_profile_version_pattern(&version),
            "case {}: expected a 10-character alphanumeric profileVersion, got {version:?}",
            case.name
        );
        let expected_arn =
            format!("arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-profiles/{}", case.name);
        assert_eq!(
            put.str_member("arn"),
            expected_arn,
            "case {}: expected arn {expected_arn}, got {:?}",
            case.name,
            put.body["arn"]
        );
        assert_eq!(
            put.str_member("profileVersionArn"),
            format!("{expected_arn}/{version}"),
            "case {}: expected profileVersionArn <arn>/<version>, got {:?}",
            case.name,
            put.body["profileVersionArn"]
        );

        let got = get_profile(&router, case.name).await;
        assert_eq!(
            got.status, 200,
            "case {}: expected GetSigningProfile 200, got {} {:?}",
            case.name, got.status, got.body
        );
        assert_eq!(
            got.str_member("profileName"),
            case.name,
            "case {}: expected the stored profileName, got {:?}",
            case.name,
            got.body["profileName"]
        );
        assert_eq!(
            got.str_member("profileVersion"),
            version,
            "case {}: expected the version Put returned, got {:?}",
            case.name,
            got.body["profileVersion"]
        );
        assert_eq!(
            got.str_member("arn"),
            expected_arn,
            "case {}: expected the stored arn, got {:?}",
            case.name,
            got.body["arn"]
        );
        assert_eq!(
            got.str_member("profileVersionArn"),
            format!("{expected_arn}/{version}"),
            "case {}: expected the stored profileVersionArn, got {:?}",
            case.name,
            got.body["profileVersionArn"]
        );
        assert_eq!(
            got.str_member("status"),
            "Active",
            "case {}: expected a put profile to be Active, got {:?}",
            case.name,
            got.body["status"]
        );
        assert!(
            SIGNING_PROFILE_STATUS_VALUES.contains(&got.str_member("status").as_str()),
            "case {}: status {:?} is not one of the modelled {SIGNING_PROFILE_STATUS_VALUES:?}",
            case.name,
            got.body["status"]
        );

        // Every member the request carried comes back exactly as submitted, and
        // one it did not carry is absent rather than invented.
        for member in [
            "platformId",
            "signingMaterial",
            "signingParameters",
            "overrides",
        ] {
            assert_eq!(
                got.body[member], case.request[member],
                "case {}: expected {member} to round-trip as {:?}, got {:?}",
                case.name, case.request[member], got.body[member]
            );
        }
        // `signatureValidityPeriod` round-trips when the request carried one, and
        // otherwise reports the default the model documents for
        // `PutSigningProfileRequest$signatureValidityPeriod` ("If unspecified,
        // the default is 135 months"), spelled out here rather than imported so a
        // wrong default in the handler fails. `tags` is always reported, empty for
        // a profile the request gave none.
        let expected_period = if case.request["signatureValidityPeriod"].is_null() {
            json!({ "value": 135, "type": "MONTHS" })
        } else {
            case.request["signatureValidityPeriod"].clone()
        };
        assert_eq!(
            got.body["signatureValidityPeriod"], expected_period,
            "case {}: expected signatureValidityPeriod {expected_period:?}, got {:?}",
            case.name, got.body["signatureValidityPeriod"]
        );
        let expected_tags = if case.request["tags"].is_null() {
            json!({})
        } else {
            case.request["tags"].clone()
        };
        assert_eq!(
            got.body["tags"], expected_tags,
            "case {}: expected tags {expected_tags:?}, got {:?}",
            case.name, got.body["tags"]
        );
        match case.platform_display_name {
            Some(display) => assert_eq!(
                got.str_member("platformDisplayName"),
                display,
                "case {}: expected platformDisplayName {display:?}, got {:?}",
                case.name,
                got.body["platformDisplayName"]
            ),
            None => assert!(
                got.body["platformDisplayName"].is_null(),
                "case {}: expected no platformDisplayName for a platform outside the catalog, got {:?}",
                case.name,
                got.body["platformDisplayName"]
            ),
        }
    }
}

/// A validity period round-trips for EVERY unit `ValidityType` admits, so the
/// member is stored rather than interpreted.
#[tokio::test]
async fn every_validity_type_round_trips() {
    let router = emulator();

    for (ordinal, validity_type) in VALIDITY_TYPE_VALUES.iter().enumerate() {
        let name = format!("Validity_{validity_type}");
        let period = json!({ "value": ordinal as i64 + 1, "type": validity_type });
        let put = put_profile(
            &router,
            &name,
            json!({
                "platformId": "AWSLambda-SHA384-ECDSA",
                "signingMaterial": signing_material(),
                "signatureValidityPeriod": period,
            }),
        )
        .await;
        assert_eq!(
            put.status, 200,
            "case {validity_type}: expected 200, got {} {:?}",
            put.status, put.body
        );

        let got = get_profile(&router, &name).await;
        assert_eq!(
            got.body["signatureValidityPeriod"], period,
            "case {validity_type}: expected {period:?}, got {:?}",
            got.body["signatureValidityPeriod"]
        );
    }
}

/// Every create mints a version no other profile holds, and a create under a name
/// already taken mints nothing at all. The invariant is uniqueness, so it is
/// checked over a run of creates rather than over one pair.
#[tokio::test]
async fn each_create_mints_a_version_no_other_profile_holds() {
    let router = emulator();
    // Fewer creates than one page of `ListSigningProfiles` holds
    // (`PROFILE_PAGE_LIMIT`), so the listing below is the whole store.
    const CREATES: usize = 12;

    let mut versions = Vec::with_capacity(CREATES);
    let mut names = Vec::with_capacity(CREATES);
    for ordinal in 0..CREATES {
        let name = format!("Versioned_Profile_{ordinal}");
        let request = json!({
            "platformId": "AWSLambda-SHA384-ECDSA",
            "signingMaterial": signing_material(),
            "signingParameters": { "generation": ordinal.to_string() },
        });
        let put = put_profile(&router, &name, request.clone()).await;
        assert_eq!(
            put.status, 200,
            "create {ordinal}: expected 200, got {} {:?}",
            put.status, put.body
        );
        let version = put.str_member("profileVersion");
        assert!(
            matches_profile_version_pattern(&version),
            "create {ordinal}: expected a 10-character alphanumeric profileVersion, got \
             {version:?}"
        );
        assert!(
            !versions.contains(&version),
            "create {ordinal}: expected a version no other profile holds, got {version:?} which \
             was already minted: {versions:?}"
        );
        assert_eq!(
            put.str_member("arn"),
            format!("arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-profiles/{name}"),
            "create {ordinal}: expected the profile ARN to name the profile, got {:?}",
            put.body["arn"]
        );

        // The same name again: the name is taken, so nothing is minted for it.
        let again = put_profile(&router, &name, request).await;
        assert_eq!(
            again.status, 400,
            "create {ordinal}: expected the repeated create to be refused, got {} {:?}",
            again.status, again.body
        );
        assert!(
            again.body["profileVersion"].is_null(),
            "create {ordinal}: expected the refusal to mint no version, got {:?}",
            again.body["profileVersion"]
        );

        versions.push(version);
        names.push(name);
    }

    // Each profile reports the version its own create minted, and the members
    // that create carried.
    for (ordinal, name) in names.iter().enumerate() {
        let got = get_profile(&router, name).await;
        assert_eq!(
            &got.str_member("profileVersion"),
            &versions[ordinal],
            "profile {name}: expected the version its create minted, got {:?}",
            got.body["profileVersion"]
        );
        assert_eq!(
            got.body["signingParameters"],
            json!({ "generation": ordinal.to_string() }),
            "profile {name}: expected its own create's signingParameters, got {:?}",
            got.body["signingParameters"]
        );
    }
    // And exactly one record exists per name, however many creates were sent.
    let listed = listed_names(&list_profiles(&router, "").await);
    let mut expected = names.clone();
    expected.sort();
    assert_eq!(
        listed, expected,
        "expected one profile record per name after {CREATES} creates, got {listed:?}"
    );
}

/// `ListSigningProfiles` pages over the store in a stable order and honours every
/// filter it models. The page sizes are a table: both published bounds of
/// `maxResults`, and a size in between.
#[tokio::test]
async fn list_pages_through_profiles_and_honours_the_filters() {
    let router = emulator();

    // Two platforms, so the `platformId` filter has something to select.
    let seeded: Vec<(String, &str)> = (0..7)
        .map(|ordinal| {
            let platform = if ordinal % 2 == 0 {
                "AWSLambda-SHA384-ECDSA"
            } else {
                "AWSIoTDeviceManagement-SHA256-ECDSA"
            };
            (format!("Listed_{ordinal}"), platform)
        })
        .collect();
    for (name, platform) in &seeded {
        let put = put_profile(
            &router,
            name,
            json!({ "platformId": platform, "signingMaterial": signing_material() }),
        )
        .await;
        assert_eq!(
            put.status, 200,
            "seeding {name}: expected 200, got {} {:?}",
            put.status, put.body
        );
    }

    let mut all_names: Vec<String> = seeded.iter().map(|(name, _)| name.clone()).collect();
    all_names.sort();

    for page_size in [1, 3, MAX_RESULTS_CEILING] {
        let walked = walk_profiles(&router, "", page_size).await;
        assert_eq!(
            walked, all_names,
            "maxResults {page_size}: expected the whole store once, in name order, got {walked:?}"
        );
    }

    // A filtered walk reports only the matching profiles.
    let lambda_names: Vec<String> = seeded
        .iter()
        .filter(|(_, platform)| *platform == "AWSLambda-SHA384-ECDSA")
        .map(|(name, _)| name.clone())
        .collect();
    let walked = walk_profiles(&router, "&platformId=AWSLambda-SHA384-ECDSA", 2).await;
    assert_eq!(
        walked, lambda_names,
        "expected only the AWSLambda profiles, got {walked:?}"
    );

    // A truncated page carries a token that resumes where it ended.
    let first = list_profiles(&router, "?maxResults=2").await;
    let token = first.body["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a truncated page must carry a nextToken: {:?}", first.body))
        .to_string();
    let resumed = list_profiles(&router, &format!("?maxResults=2&nextToken={token}")).await;
    assert_eq!(
        listed_names(&resumed),
        all_names[2..4].to_vec(),
        "expected the page after {token:?} to be {:?}, got {:?}",
        &all_names[2..4],
        listed_names(&resumed)
    );

    // The last page carries no token.
    let whole = list_profiles(&router, "?maxResults=25").await;
    assert!(
        whole.body["nextToken"].is_null(),
        "expected no nextToken on a page that reported everything, got {:?}",
        whole.body["nextToken"]
    );

    // A canceled profile is hidden by default, shown by `includeCanceled`, and
    // selectable by `statuses`.
    let canceled = &all_names[0];
    assert_eq!(
        cancel_profile(&router, canceled).await.status,
        204,
        "expected CancelSigningProfile 204 for {canceled}"
    );
    let remaining: Vec<String> = all_names
        .iter()
        .filter(|name| *name != canceled)
        .cloned()
        .collect();
    let cases: [(&str, Vec<String>); 4] = [
        ("", remaining.clone()),
        ("&includeCanceled=false", remaining.clone()),
        ("&includeCanceled=true", all_names.clone()),
        ("&statuses=Canceled", vec![canceled.clone()]),
    ];
    for (filters, expected) in cases {
        let walked = walk_profiles(&router, filters, 2).await;
        assert_eq!(
            walked, expected,
            "filters {filters:?}: expected {expected:?}, got {walked:?}"
        );
    }
}

/// A signing job snapshots the profile it ran against, is idempotent on its
/// `clientRequestToken`, and reports what it stored — not what a later request
/// carries.
#[tokio::test]
async fn start_signing_job_is_idempotent_and_describe_reports_the_stored_job() {
    let router = emulator();

    let put = put_profile(
        &router,
        "Job_Profile",
        json!({
            "platformId": "AWSIoTDeviceManagement-SHA256-ECDSA",
            "signingMaterial": {
                "certificateArn": "arn:aws:acm:us-west-2:000000000000:certificate/abc",
            },
            "signingParameters": { "env": "prod" },
        }),
    )
    .await;
    assert_eq!(put.status, 200, "seeding the profile: {:?}", put.body);
    let first_version = put.str_member("profileVersion");

    let source = json!({ "s3": { "bucketName": "src-bucket", "key": "code.bin", "version": "1" } });
    let destination = json!({ "s3": { "bucketName": "dst-bucket", "prefix": "signed/" } });
    let started = start_job(
        &router,
        json!({
            "source": source,
            "destination": destination,
            "profileName": "Job_Profile",
            "clientRequestToken": "tok-1",
        }),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "expected StartSigningJob 200, got {} {:?}",
        started.status, started.body
    );
    let job_id = started.str_member("jobId");
    assert!(!job_id.is_empty(), "expected a jobId, got {:?}", started.body);
    assert_eq!(
        started.str_member("jobOwner"),
        ACCOUNT_ID,
        "expected jobOwner {ACCOUNT_ID}, got {:?}",
        started.body["jobOwner"]
    );

    // The same token names the same job, even when the rest of the request
    // differs — and it does NOT start a second one.
    let replayed = start_job(
        &router,
        json!({
            "source": { "s3": { "bucketName": "other-bucket", "key": "other.bin", "version": "9" } },
            "destination": { "s3": { "bucketName": "other-dst" } },
            "profileName": "Job_Profile",
            "clientRequestToken": "tok-1",
        }),
    )
    .await;
    assert_eq!(
        replayed.str_member("jobId"),
        job_id,
        "expected the same clientRequestToken to answer with jobId {job_id}, got {:?}",
        replayed.body
    );

    // A different token starts a different job.
    let second = start_job(
        &router,
        json!({
            "source": source,
            "destination": destination,
            "profileName": "Job_Profile",
            "clientRequestToken": "tok-2",
        }),
    )
    .await;
    assert_ne!(
        second.str_member("jobId"),
        job_id,
        "expected a new clientRequestToken to start a new job, got {:?}",
        second.body
    );

    // A later create under the same profile name is refused — the name is taken —
    // and the job keeps reporting the version it started under.
    let refused = put_profile(
        &router,
        "Job_Profile",
        json!({
            "platformId": "AWSLambda-SHA384-ECDSA",
            "signingMaterial": signing_material(),
        }),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "expected the repeated create to be refused, got {} {:?}",
        refused.status, refused.body
    );

    let described = describe_job(&router, &job_id).await;
    assert_eq!(
        described.status, 200,
        "expected DescribeSigningJob 200, got {} {:?}",
        described.status, described.body
    );
    let expectations: [(&str, Value); 7] = [
        ("jobId", json!(job_id)),
        ("profileName", json!("Job_Profile")),
        ("profileVersion", json!(first_version)),
        ("platformId", json!("AWSIoTDeviceManagement-SHA256-ECDSA")),
        (
            "platformDisplayName",
            json!("AWS IoT Device Management SHA256-ECDSA "),
        ),
        (
            "signingMaterial",
            json!({ "certificateArn": "arn:aws:acm:us-west-2:000000000000:certificate/abc" }),
        ),
        ("signingParameters", json!({ "env": "prod" })),
    ];
    for (member, expected) in expectations {
        assert_eq!(
            described.body[member], expected,
            "expected {member} to be the snapshot taken at start ({expected:?}), got {:?}",
            described.body[member]
        );
    }
    assert_eq!(
        described.body["source"], source,
        "expected the source StartSigningJob stored, got {:?}",
        described.body["source"]
    );
    assert_eq!(
        described.str_member("requestedBy"),
        caller_principal(),
        "expected requestedBy to name the resolved caller principal, got {:?}",
        described.body["requestedBy"]
    );
    assert_eq!(
        described.str_member("jobOwner"),
        ACCOUNT_ID,
        "expected jobOwner {ACCOUNT_ID}, got {:?}",
        described.body["jobOwner"]
    );
    assert!(
        described.body["createdAt"].is_number(),
        "expected a createdAt timestamp, got {:?}",
        described.body["createdAt"]
    );
    assert!(
        SIGNING_STATUS_VALUES.contains(&described.str_member("status").as_str()),
        "status {:?} is not one of the modelled {SIGNING_STATUS_VALUES:?}",
        described.body["status"]
    );
}

/// A job is `InProgress` when it is started and `Succeeded` once the clock has
/// made its completion due; the completion is reported identically to every later
/// reader.
#[tokio::test]
async fn a_signing_job_completes_once_the_clock_makes_it_due() {
    let router = emulator_with_object_storage();
    assert_eq!(
        put_profile(
            &router,
            "Completing_Profile",
            json!({
                "platformId": "AWSLambda-SHA384-ECDSA",
                "signingMaterial": signing_material(),
            })
        )
        .await
        .status,
        200
    );

    // The object the job signs, and the bucket it writes the artifact to: a job
    // of this workflow reads one and writes the other.
    let version = super::code_signing_objects::seed_signing_objects(
        &router,
        REGION,
        "src-bucket",
        "code.zip",
        "dst-bucket",
    )
    .await;

    let started = start_job(
        &router,
        json!({
            "source": { "s3": {
                "bucketName": "src-bucket",
                "key": "code.zip",
                "version": version,
            } },
            "destination": { "s3": { "bucketName": "dst-bucket", "prefix": "signed/" } },
            "profileName": "Completing_Profile",
            "clientRequestToken": "tok-completing",
        }),
    )
    .await;
    assert_eq!(started.status, 200, "starting the job: {:?}", started.body);
    let job_id = started.str_member("jobId");

    // Straight after the start the job is still running: the span is never zero.
    let running = describe_job(&router, &job_id).await;
    assert_eq!(
        running.str_member("status"),
        "InProgress",
        "expected a just-started job to be InProgress, got {:?}",
        running.body
    );
    assert!(
        running.body["completedAt"].is_null(),
        "expected no completedAt while InProgress, got {:?}",
        running.body["completedAt"]
    );
    assert!(
        running.body["signedObject"].is_null(),
        "expected no signedObject while InProgress, got {:?}",
        running.body["signedObject"]
    );

    tokio::time::sleep(JOB_COMPLETION_SPAN + Duration::from_millis(250)).await;

    let done = describe_job(&router, &job_id).await;
    assert_eq!(
        done.str_member("status"),
        "Succeeded",
        "expected the job to have Succeeded once due, got {:?}",
        done.body
    );
    assert!(
        done.body["completedAt"].is_number(),
        "expected a completedAt on a Succeeded job, got {:?}",
        done.body["completedAt"]
    );
    assert_eq!(
        done.body["signedObject"],
        json!({ "s3": { "bucketName": "dst-bucket", "key": format!("signed/{job_id}") } }),
        "expected the signed object under the destination prefix, got {:?}",
        done.body["signedObject"]
    );

    // Reading does not move anything: a second reader sees the same completion.
    let again = describe_job(&router, &job_id).await;
    assert_eq!(
        again.body["completedAt"], done.body["completedAt"],
        "expected completedAt to be stable across reads, got {:?} then {:?}",
        done.body["completedAt"], again.body["completedAt"]
    );
    assert_eq!(
        again.body["signedObject"], done.body["signedObject"],
        "expected the signed object to be stable across reads"
    );
}

/// Cancelling a profile is a SOFT cancel: the record stays and stays viewable, it
/// drops out of the default listing, and it signs nothing more.
#[tokio::test]
async fn cancel_is_a_soft_cancel_that_stops_new_jobs() {
    let router = emulator();
    let put = put_profile(
        &router,
        "Canceled_Profile",
        json!({
            "platformId": "AWSLambda-SHA384-ECDSA",
            "signingMaterial": signing_material(),
            "tags": { "keep": "me" },
        }),
    )
    .await;
    assert_eq!(put.status, 200, "seeding the profile: {:?}", put.body);
    let version = put.str_member("profileVersion");

    let canceled = cancel_profile(&router, "Canceled_Profile").await;
    assert_eq!(
        canceled.status, 204,
        "expected CancelSigningProfile 204, got {} {:?}",
        canceled.status, canceled.body
    );

    let got = get_profile(&router, "Canceled_Profile").await;
    assert_eq!(
        got.status, 200,
        "expected a canceled profile to stay viewable, got {} {:?}",
        got.status, got.body
    );
    assert_eq!(
        got.str_member("status"),
        "Canceled",
        "expected status Canceled, got {:?}",
        got.body["status"]
    );
    assert_eq!(
        got.str_member("profileVersion"),
        version,
        "expected the version to survive the cancel, got {:?}",
        got.body["profileVersion"]
    );
    assert_eq!(
        got.body["tags"],
        json!({ "keep": "me" }),
        "expected the tags to survive the cancel, got {:?}",
        got.body["tags"]
    );

    assert!(
        listed_names(&list_profiles(&router, "").await).is_empty(),
        "expected the default listing to omit a canceled profile"
    );

    let refused = start_job(
        &router,
        json!({
            "source": { "s3": { "bucketName": "src-bucket", "key": "code.bin", "version": "1" } },
            "destination": { "s3": { "bucketName": "dst-bucket" } },
            "profileName": "Canceled_Profile",
            "clientRequestToken": "tok-canceled",
        }),
    )
    .await;
    assert_eq!(
        refused.status, 404,
        "expected a canceled profile to refuse a new job with 404, got {} {:?}",
        refused.status, refused.body
    );
    assert!(
        refused.error_code().contains("ResourceNotFoundException"),
        "expected ResourceNotFoundException, got {:?}",
        refused.body
    );
}

/// What the slice refuses, and with which status and code: an unknown resource is
/// a modelled 404, and input outside a modelled constraint or a rule the model
/// cannot express is a 400 `ValidationException`.
#[tokio::test]
async fn unknown_resources_and_bad_input_are_refused() {
    let router = emulator();
    assert_eq!(
        put_profile(
            &router,
            "Known_Profile",
            json!({
                "platformId": "AWSLambda-SHA384-ECDSA",
                "signingMaterial": signing_material(),
            })
        )
        .await
        .status,
        200
    );
    let good_source =
        json!({ "s3": { "bucketName": "src-bucket", "key": "code.bin", "version": "1" } });
    let good_destination = json!({ "s3": { "bucketName": "dst-bucket" } });

    // One refusal row: the case, the request (method, uri, body) and what the
    // emulator must answer (status, wire error code).
    type RefusalCase = (
        &'static str,
        &'static str,
        String,
        Option<Value>,
        u16,
        &'static str,
    );
    let cases: Vec<RefusalCase> = vec![
        (
            "an unknown profile name",
            "GET",
            "/signing-profiles/Nope_X".to_string(),
            None,
            404,
            "ResourceNotFoundException",
        ),
        (
            "a profile owned by another account",
            "GET",
            "/signing-profiles/Known_Profile?profileOwner=123456789012".to_string(),
            None,
            404,
            "ResourceNotFoundException",
        ),
        (
            "cancelling an unknown profile",
            "DELETE",
            "/signing-profiles/Nope_X".to_string(),
            None,
            404,
            "ResourceNotFoundException",
        ),
        (
            "an unknown job id",
            "GET",
            "/signing-jobs/00000000-0000-0000-0000-000000000000".to_string(),
            None,
            404,
            "ResourceNotFoundException",
        ),
        (
            "a job against an unknown profile",
            "POST",
            "/signing-jobs".to_string(),
            Some(json!({
                "source": good_source,
                "destination": good_destination,
                "profileName": "No_Such_Profile",
                "clientRequestToken": "tok-unknown",
            })),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a job whose source names no S3 object",
            "POST",
            "/signing-jobs".to_string(),
            Some(json!({
                "source": {},
                "destination": good_destination,
                "profileName": "Known_Profile",
                "clientRequestToken": "tok-no-source",
            })),
            400,
            "ValidationException",
        ),
        (
            "a job whose destination names no bucket",
            "POST",
            "/signing-jobs".to_string(),
            Some(json!({
                "source": good_source,
                "destination": { "s3": { "prefix": "signed/" } },
                "profileName": "Known_Profile",
                "clientRequestToken": "tok-no-destination",
            })),
            400,
            "ValidationException",
        ),
        (
            "maxResults above the modelled ceiling",
            "GET",
            "/signing-profiles?maxResults=99".to_string(),
            None,
            400,
            "ValidationException",
        ),
        (
            "maxResults below the modelled floor",
            "GET",
            "/signing-profiles?maxResults=0".to_string(),
            None,
            400,
            "ValidationException",
        ),
        (
            "a nextToken no page ever returned",
            "GET",
            "/signing-profiles?nextToken=not%20a%20profile%20name".to_string(),
            None,
            400,
            "ValidationException",
        ),
        (
            "a profileName shorter than the modelled minimum",
            "PUT",
            "/signing-profiles/A".to_string(),
            Some(json!({ "platformId": "AWSLambda-SHA384-ECDSA" })),
            400,
            "ValidationException",
        ),
        (
            "a profileName outside the modelled pattern",
            "PUT",
            "/signing-profiles/Bad-Name".to_string(),
            Some(json!({ "platformId": "AWSLambda-SHA384-ECDSA" })),
            400,
            "ValidationException",
        ),
        (
            "a put with no platformId",
            "PUT",
            "/signing-profiles/No_Platform".to_string(),
            Some(json!({})),
            400,
            "ValidationException",
        ),
    ];

    for (case, method, uri, body, status, code) in cases {
        let answer = call(&router, method, &uri, body).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {} {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.error_code().contains(code),
            "case {case:?}: expected error code {code}, got {:?}",
            answer.body
        );
    }

    // Both published bounds of `maxResults` are accepted.
    for page_size in [1, MAX_RESULTS_CEILING] {
        let answer = list_profiles(&router, &format!("?maxResults={page_size}")).await;
        assert_eq!(
            answer.status, 200,
            "maxResults {page_size} is a published bound and must be accepted, got {} {:?}",
            answer.status, answer.body
        );
    }

    // And nothing the refusals touched was written: the one profile is still the
    // only record, still Active.
    let listed = listed_names(&list_profiles(&router, "").await);
    assert_eq!(
        listed,
        vec!["Known_Profile".to_string()],
        "expected the refused calls to have stored nothing, got {listed:?}"
    );
}
