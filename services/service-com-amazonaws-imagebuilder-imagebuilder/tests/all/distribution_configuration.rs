// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the distribution-configuration slice of this service.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes.
//!
//! The rule under test: `CreateDistributionConfiguration` persists the configuration the
//! request describes under an ARN derived from a normalized form of its name, and
//! `GetDistributionConfiguration` reports that stored record — so every member a create
//! carried comes back unchanged, whatever the shape of its per-Region settings, an ARN
//! nothing was stored under is reported missing, and a stored configuration is what a
//! pipeline's reference to one resolves to.
//!
//! None of the EC2, ECR, License Manager or S3 values a distribution names is resolved at
//! create time, so the rows below name values no other service in this emulator holds and
//! still expect a 200.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region a case signs for when it only needs one; the ARNs it creates carry it.
const REGION: &str = "us-west-2";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// The `timeoutMinutes` every read reports: the member is required on the modelled
/// `DistributionConfiguration` and is no creation parameter — "You can't set this property
/// when you create or update a distribution configuration, and it has no effect on
/// distribution behavior" — so one value serves every configuration, the maximum of the
/// member's own modelled range (`DistributionTimeoutMinutes`, 30..720).
const TIMEOUT_MINUTES: i64 = 720;

/// Members a create request carries that the stored configuration does not report back:
/// `clientToken` is echoed by the create response instead, and `dryRun` describes the call
/// rather than the resource.
const NOT_CONFIGURATION_MEMBERS: &[&str] = &["clientToken", "dryRun"];

/// A component document the `CreateComponent` a pipeline case needs can store.
const BUILD_DOCUMENT: &str = "name: HelloWorld\nschemaVersion: 1.0\nphases:\n  - name: build\n    steps:\n      - name: Greet\n        action: ExecuteBash\n        inputs:\n          commands:\n            - echo hello\n";

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_imagebuilder_imagebuilder::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `imagebuilder` in `region` — the signal the router routes on and
/// the handlers read the Region from.
fn request(
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("imagebuilder.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDIMAGEBUILDER/20260101/{region}/imagebuilder/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(region, method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// `CreateDistributionConfiguration` with `body`, in `region`.
async fn create_in(router: &MultiServiceRouter, region: &str, body: &Value) -> (u16, Value) {
    call_in(
        router,
        region,
        "PUT",
        "/CreateDistributionConfiguration",
        Some(body),
    )
    .await
}

/// `CreateDistributionConfiguration` with `body`, in [`REGION`].
async fn create(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    create_in(router, REGION, body).await
}

/// `GetDistributionConfiguration` for `arn`, in `region`.
async fn get_in(router: &MultiServiceRouter, region: &str, arn: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "GET",
        &format!("/GetDistributionConfiguration?distributionConfigurationArn={arn}"),
        None,
    )
    .await
}

/// `GetDistributionConfiguration` for `arn`, in [`REGION`].
async fn get(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    get_in(router, REGION, arn).await
}

/// A create request body: the three required members, and whatever else `extra` names.
fn create_body(name: &str, client_token: &str, distributions: Value, extra: Value) -> Value {
    let mut body = Map::new();
    body.insert("name".to_string(), json!(name));
    body.insert("clientToken".to_string(), json!(client_token));
    body.insert("distributions".to_string(), distributions);
    if let Value::Object(extra) = extra {
        for (member, value) in extra {
            body.insert(member, value);
        }
    }
    Value::Object(body)
}

/// The ARN a distribution configuration named `normalized` carries in `region`.
fn arn_of(region: &str, normalized: &str) -> String {
    format!("arn:aws:imagebuilder:{region}:{ACCOUNT}:distribution-configuration/{normalized}")
}

/// The error code a refusal carries, from `__type` (restJson1 renders the shape there,
/// optionally namespace-qualified).
fn error_code(case: &str, body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an error shape, got {body}"))
        .rsplit(['#', '/'])
        .next()
        .expect("a non-empty error type")
        .to_string()
}

/// The `message` of a refusal.
fn error_message(case: &str, body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a message member in {body}"))
        .to_string()
}

/// Assert the status and the error code of a refusal.
fn assert_refused(case: &str, status: u16, body: &Value, expect_status: u16, expect_code: &str) {
    assert_eq!(
        status, expect_status,
        "case {case:?}: expected HTTP {expect_status}, got {status} with {body}"
    );
    let code = error_code(case, body);
    assert_eq!(
        code, expect_code,
        "case {case:?}: expected the error code {expect_code:?}, got {code:?} with {body}"
    );
}

/// The `distributionConfiguration` of a successful read.
fn configuration(case: &str, body: &Value) -> Value {
    body.get("distributionConfiguration")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a distributionConfiguration in {body}"))
}

/// Every member of `sent` that describes the RESOURCE, as the stored configuration should
/// report it back.
fn round_tripped_members(sent: &Value) -> Vec<(String, Value)> {
    sent.as_object()
        .expect("a create body is an object")
        .iter()
        .filter(|(member, _)| !NOT_CONFIGURATION_MEMBERS.contains(&member.as_str()))
        .map(|(member, value)| (member.clone(), value.clone()))
        .collect()
}

/// One row of the round-trip table: a create body and the normalized name its ARN carries.
struct Row {
    case: &'static str,
    name: &'static str,
    normalized: &'static str,
    distributions: fn() -> Value,
    extra: fn() -> Value,
}

/// The round-trip table. The rows differ along what the rule generalises over: how many
/// target Regions one configuration names, which per-Region settings an entry carries at
/// all, how many entries its own lists and maps hold, and whether the values name anything
/// that exists.
///
/// Every enumerated value a row names is the model's own: `targetRepository.service` is the
/// one value of `ContainerRepositoryService` (`ECR`), `diskImageFormat` one of
/// `DiskImageFormat` (`VMDK`, `RAW`, `VHD`) and `dataType` one of `SsmParameterDataType`
/// (`text`, `aws:ec2:image`).
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one target Region carrying nothing but its name",
            name: "minimal-distribution",
            normalized: "minimal-distribution",
            distributions: || json!([{ "region": "us-west-2" }]),
            extra: || json!({}),
        },
        Row {
            case: "an AMI distribution with two target accounts and three AMI tags",
            name: "AMI Distribution",
            normalized: "ami-distribution",
            distributions: || {
                json!([{
                    "region": "us-east-1",
                    "amiDistributionConfiguration": {
                        "name": "golden-{{ imagebuilder:buildDate }}",
                        "description": "the golden AMI",
                        "targetAccountIds": ["111111111111", "222222222222"],
                        "amiTags": {"stage": "prod", "team": "images", "costCenter": "4711"},
                        "kmsKeyId": format!("arn:aws:kms:us-east-1:{ACCOUNT}:key/absent"),
                        "launchPermission": {
                            "userIds": ["333333333333"],
                            "userGroups": ["all"],
                        },
                    },
                }])
            },
            extra: || json!({ "description": "distributes the golden image" }),
        },
        Row {
            case: "a container distribution with two repository image tags",
            name: "container_distribution",
            normalized: "container-distribution",
            distributions: || {
                json!([{
                    "region": "eu-central-1",
                    "containerDistributionConfiguration": {
                        "description": "the app image",
                        "containerTags": ["latest", "v2"],
                        "targetRepository": {"service": "ECR", "repositoryName": "team-a/my-repo"},
                    },
                }])
            },
            extra: || json!({}),
        },
        Row {
            case: "two target Regions, with licenses and launch templates",
            name: "two-region-distribution",
            normalized: "two-region-distribution",
            distributions: || {
                json!([
                    {
                        "region": "us-west-2",
                        "licenseConfigurationArns": [format!(
                            "arn:aws:license-manager:us-west-2:{ACCOUNT}:license-configuration:\
                             lic-00000000000000000000000000000001"
                        )],
                        "launchTemplateConfigurations": [
                            {"launchTemplateId": "lt-00000000000000001", "setDefaultVersion": true},
                            {
                                "launchTemplateId": "lt-00000000000000002",
                                "accountId": "111111111111",
                                "setDefaultVersion": false,
                            },
                        ],
                    },
                    {
                        "region": "ap-southeast-3",
                        "amiDistributionConfiguration": {"description": "the far copy"},
                    },
                ])
            },
            extra: || json!({ "tags": {"owner": "platform"} }),
        },
        Row {
            case: "three target Regions, with exports, fast launch and SSM parameters",
            name: "three-region-distribution",
            normalized: "three-region-distribution",
            distributions: || {
                json!([
                    {
                        "region": "us-west-2",
                        "s3ExportConfiguration": {
                            "roleName": "vmimport",
                            "diskImageFormat": "VHD",
                            "s3Bucket": "no-such-bucket",
                            "s3Prefix": "exports/",
                        },
                    },
                    {
                        "region": "us-east-2",
                        "fastLaunchConfigurations": [{
                            "enabled": true,
                            "maxParallelLaunches": 6,
                            "accountId": "111111111111",
                            "snapshotConfiguration": {"targetResourceCount": 5},
                            "launchTemplate": {
                                "launchTemplateId": "lt-00000000000000003",
                                "launchTemplateVersion": "2",
                            },
                        }],
                    },
                    {
                        "region": "eu-west-1",
                        "ssmParameterConfigurations": [
                            {"parameterName": "/images/golden", "dataType": "aws:ec2:image"},
                            {
                                "parameterName": "/images/golden-text",
                                "amiAccountId": "222222222222",
                                "dataType": "text",
                            },
                        ],
                    },
                ])
            },
            extra: || json!({}),
        },
        Row {
            case: "a configuration carrying every optional member of its own",
            name: "Fully_Described Distribution",
            normalized: "fully-described-distribution",
            distributions: || json!([{ "region": "ca-central-1" }]),
            extra: || {
                json!({
                    "description": "every member a configuration itself carries",
                    "tags": {"owner": "platform", "stage": "test"},
                })
            },
        },
    ]
}

#[tokio::test]
async fn a_created_configuration_is_reported_back_member_for_member() {
    let router = emulator();

    for (n, row) in rows().into_iter().enumerate() {
        let body = create_body(
            row.name,
            &format!("token-{n:04}"),
            (row.distributions)(),
            (row.extra)(),
        );
        let (status, created) = create(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from the create, got {status} with {created}",
            row.case
        );

        let expected_arn = arn_of(REGION, row.normalized);
        let actual_arn = created
            .get("distributionConfigurationArn")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            actual_arn, expected_arn,
            "case {:?}: expected the ARN {expected_arn:?}, got {actual_arn:?}",
            row.case
        );
        let echoed = created.get("clientToken").and_then(Value::as_str);
        assert_eq!(
            echoed,
            Some(format!("token-{n:04}").as_str()),
            "case {:?}: expected the create to echo the client token, got {echoed:?}",
            row.case
        );
        let request_id = created.get("requestId").and_then(Value::as_str);
        assert!(
            request_id.is_some_and(|id| !id.is_empty()),
            "case {:?}: expected the create to name a requestId, got {request_id:?}",
            row.case
        );

        let (status, read) = get(&router, &expected_arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from the read, got {status} with {read}",
            row.case
        );
        let stored = configuration(row.case, &read);

        for (member, sent) in round_tripped_members(&body) {
            let reported = stored.get(&member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                reported, sent,
                "case {:?}: expected the stored {member} to be {sent}, got {reported}",
                row.case
            );
        }

        let reported_arn = stored.get("arn").and_then(Value::as_str).unwrap_or_default();
        assert_eq!(
            reported_arn, expected_arn,
            "case {:?}: expected the stored arn {expected_arn:?}, got {reported_arn:?}",
            row.case
        );
        let timeout = stored.get("timeoutMinutes").and_then(Value::as_i64);
        assert_eq!(
            timeout,
            Some(TIMEOUT_MINUTES),
            "case {:?}: expected timeoutMinutes {TIMEOUT_MINUTES}, got {timeout:?}",
            row.case
        );
        let stamp = stored
            .get("dateCreated")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            stamp.len() >= 20 && stamp.ends_with('Z') && stamp.contains('T'),
            "case {:?}: expected dateCreated to be an ISO 8601 UTC instant, got {stamp:?}",
            row.case
        );
        // `dateUpdated` reports when an update last replaced the configuration, and no
        // update has run, so the read reports no such member.
        assert_eq!(
            stored.get("dateUpdated"),
            None,
            "case {:?}: expected no dateUpdated before any update, got {:?}",
            row.case,
            stored.get("dateUpdated")
        );
        // A configuration no create tagged carries no tags, which is an empty map rather
        // than an absent member.
        if !body.as_object().is_some_and(|body| body.contains_key("tags")) {
            let tags = stored.get("tags").cloned().unwrap_or(Value::Null);
            assert_eq!(
                tags,
                json!({}),
                "case {:?}: expected an untagged configuration to report no tags, got {tags}",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn a_read_reports_the_same_configuration_every_time() {
    let router = emulator();
    let body = create_body(
        "stable-distribution",
        "token-stable",
        json!([{ "region": "us-west-2" }]),
        json!({ "tags": {"owner": "platform"} }),
    );
    let (status, created) = create(&router, &body).await;
    assert_eq!(status, 200, "case the create: got {status} with {created}");
    let arn = arn_of(REGION, "stable-distribution");

    let (_, first) = get(&router, &arn).await;
    let (_, second) = get(&router, &arn).await;
    let (one, two) = (
        configuration("the first read", &first),
        configuration("the second read", &second),
    );
    assert_eq!(
        one, two,
        "case two reads: expected the same configuration twice, got {one} then {two}"
    );
}

#[tokio::test]
async fn the_arn_carries_a_normalized_name_and_that_name_is_taken_once() {
    // Every row is the SAME name under the rule the `name` member states: names that
    // differ only in case, spaces or underscores count as the same name, and the ARN
    // carries that normalized form.
    let equivalent: &[&str] = &[
        "Spread Wide",
        "spread_wide",
        "SPREAD-WIDE",
        "spread wide",
        "Spread_Wide",
    ];
    let router = emulator();
    let expected_arn = arn_of(REGION, "spread-wide");

    let body = create_body(
        equivalent[0],
        "token-first",
        json!([{ "region": "us-west-2" }]),
        json!({}),
    );
    let (status, created) = create(&router, &body).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected HTTP 200, got {status} with {created}",
        equivalent[0]
    );
    let actual_arn = created
        .get("distributionConfigurationArn")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert_eq!(
        actual_arn, expected_arn,
        "case {:?}: expected the ARN {expected_arn:?}, got {actual_arn:?}",
        equivalent[0]
    );

    for (n, name) in equivalent.iter().enumerate().skip(1) {
        let body = create_body(
            name,
            &format!("token-same-{n}"),
            json!([{ "region": "us-west-2" }]),
            json!({}),
        );
        let (status, answer) = create(&router, &body).await;
        assert_refused(name, status, &answer, 400, "ResourceAlreadyExistsException");
        // The already-exists refusal names the kind of resource the create would have
        // minted and the ARN of the one that already carries it — the ARN derived from the
        // FIRST name, which this name normalizes to.
        let expected =
            format!("The following resource 'DistributionConfiguration' already exists: '{expected_arn}'.");
        let actual = error_message(name, &answer);
        assert_eq!(
            actual, expected,
            "case {name:?}: expected the message {expected:?}, got {actual:?}"
        );
    }

    // The name is taken in THIS Region only: another Region's endpoint stores its own
    // configuration under its own ARN, and neither Region reads the other's.
    let other = "us-east-1";
    let body = create_body(
        equivalent[0],
        "token-other-region",
        json!([{ "region": "us-west-2" }]),
        json!({}),
    );
    let (status, created) = create_in(&router, other, &body).await;
    assert_eq!(
        status, 200,
        "case \"the same name in another Region\": got {status} with {created}"
    );
    let other_arn = arn_of(other, "spread-wide");
    let (status, read) = get_in(&router, other, &other_arn).await;
    assert_eq!(
        status, 200,
        "case \"a read in the Region that stored it\": got {status} with {read}"
    );
    let (status, read) = get_in(&router, other, &expected_arn).await;
    assert_refused(
        "another Region's configuration",
        status,
        &read,
        404,
        "ResourceNotFoundException",
    );
}

#[tokio::test]
async fn a_region_appears_at_most_once_in_the_distributions_list() {
    let router = emulator();

    // (case, the distributions, the Region the refusal names — `None` for a list that
    // names each Region once and is stored)
    let rows: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "one Region, named once",
            json!([{ "region": "us-west-2" }]),
            None,
        ),
        (
            "three Regions, each named once",
            json!([
                { "region": "us-west-2" },
                { "region": "us-east-1" },
                { "region": "eu-west-1" },
            ]),
            None,
        ),
        (
            "the same Region twice, adjacent",
            json!([{ "region": "us-west-2" }, { "region": "us-west-2" }]),
            Some("us-west-2"),
        ),
        (
            "the same Region twice, with another between them",
            json!([
                { "region": "eu-west-1" },
                { "region": "us-east-1" },
                { "region": "eu-west-1" },
            ]),
            Some("eu-west-1"),
        ),
        (
            "the same Region twice, carrying different settings",
            json!([
                {
                    "region": "ap-south-1",
                    "amiDistributionConfiguration": {"description": "one"},
                },
                {
                    "region": "ap-south-1",
                    "amiDistributionConfiguration": {"description": "two"},
                },
            ]),
            Some("ap-south-1"),
        ),
    ];

    for (n, (case, distributions, duplicate)) in rows.into_iter().enumerate() {
        let name = format!("duplicate-case-{n}");
        let body = create_body(&name, &format!("token-dup-{n}"), distributions, json!({}));
        let (status, answer) = create(&router, &body).await;
        match duplicate {
            None => assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {answer}"
            ),
            Some(region) => {
                assert_refused(
                    case,
                    status,
                    &answer,
                    400,
                    "InvalidParameterCombinationException",
                );
                let expected = format!(
                    "The distributions list can name each target Region at most once, and it \
                     names {region} more than once."
                );
                let actual = error_message(case, &answer);
                assert_eq!(
                    actual, expected,
                    "case {case:?}: expected the message {expected:?}, got {actual:?}"
                );
                // A refused create stores nothing: the name it asked for is still free.
                let (status, read) = get(&router, &arn_of(REGION, &name)).await;
                assert_refused(case, status, &read, 404, "ResourceNotFoundException");
            }
        }
    }
}

#[tokio::test]
async fn one_client_token_creates_at_most_one_configuration() {
    let router = emulator();
    let body = create_body(
        "idempotent-distribution",
        "token-reused",
        json!([{ "region": "us-west-2" }, { "region": "us-east-1" }]),
        json!({ "description": "the first request" }),
    );

    let (status, first) = create(&router, &body).await;
    assert_eq!(status, 200, "case the first create: got {status} with {first}");

    // A retry of the SAME request answers the original response, down to its requestId,
    // rather than refusing the name the first one took.
    let (status, retry) = create(&router, &body).await;
    assert_eq!(status, 200, "case the retry: got {status} with {retry}");
    assert_eq!(
        retry, first,
        "case the retry: expected the original response, got {retry} after {first}"
    );

    // The same token carrying different creation parameters is the modelled mismatch.
    // Each row changes ONE parameter of the request above.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a different name",
            create_body(
                "other-distribution",
                "token-reused",
                json!([{ "region": "us-west-2" }, { "region": "us-east-1" }]),
                json!({ "description": "the first request" }),
            ),
        ),
        (
            "a different description",
            create_body(
                "idempotent-distribution",
                "token-reused",
                json!([{ "region": "us-west-2" }, { "region": "us-east-1" }]),
                json!({ "description": "a second request" }),
            ),
        ),
        (
            "the target Regions in another order",
            create_body(
                "idempotent-distribution",
                "token-reused",
                json!([{ "region": "us-east-1" }, { "region": "us-west-2" }]),
                json!({ "description": "the first request" }),
            ),
        ),
        (
            "one target Region fewer",
            create_body(
                "idempotent-distribution",
                "token-reused",
                json!([{ "region": "us-west-2" }]),
                json!({ "description": "the first request" }),
            ),
        ),
        (
            "one more setting inside an entry",
            create_body(
                "idempotent-distribution",
                "token-reused",
                json!([
                    {
                        "region": "us-west-2",
                        "amiDistributionConfiguration": {"description": "added"},
                    },
                    { "region": "us-east-1" },
                ]),
                json!({ "description": "the first request" }),
            ),
        ),
        (
            "a tag the first request did not carry",
            create_body(
                "idempotent-distribution",
                "token-reused",
                json!([{ "region": "us-west-2" }, { "region": "us-east-1" }]),
                json!({"description": "the first request", "tags": {"owner": "platform"}}),
            ),
        ),
    ];

    for (case, body) in rows {
        let (status, answer) = create(&router, &body).await;
        assert_refused(
            case,
            status,
            &answer,
            400,
            "IdempotentParameterMismatchException",
        );
        let expected = "The request parameters do not match those in the original idempotent \
                        request.";
        let actual = error_message(case, &answer);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the message {expected:?}, got {actual:?}"
        );
    }

    // Nothing above created a second configuration: the one the first request stored is
    // still the only one its name reaches, and no mismatched name was stored.
    let (status, read) = get(&router, &arn_of(REGION, "other-distribution")).await;
    assert_refused(
        "a name only a mismatched retry named",
        status,
        &read,
        404,
        "ResourceNotFoundException",
    );
}

#[tokio::test]
async fn a_dry_run_validates_and_stores_nothing() {
    let router = emulator();
    let arn = arn_of(REGION, "dry-run-distribution");

    // A dry run whose request is valid reports the dry-run outcome rather than creating.
    let body = create_body(
        "dry-run-distribution",
        "token-dry-run",
        json!([{ "region": "us-west-2" }]),
        json!({ "dryRun": true }),
    );
    let (status, answer) = create(&router, &body).await;
    assert_refused(
        "a valid dry run",
        status,
        &answer,
        412,
        "DryRunOperationException",
    );
    let expected = "Request would have succeeded, but dryRun flag is set.";
    let actual = error_message("a valid dry run", &answer);
    assert_eq!(
        actual, expected,
        "case \"a valid dry run\": expected the message {expected:?}, got {actual:?}"
    );

    let (status, read) = get(&router, &arn).await;
    assert_refused(
        "a read after a dry run",
        status,
        &read,
        404,
        "ResourceNotFoundException",
    );

    // A dry run whose request is invalid reports what the real create would have, and the
    // dry-run outcome is reached only once every validation has passed.
    let duplicate = create_body(
        "dry-run-distribution",
        "token-dry-run-invalid",
        json!([{ "region": "us-west-2" }, { "region": "us-west-2" }]),
        json!({ "dryRun": true }),
    );
    let (status, answer) = create(&router, &duplicate).await;
    assert_refused(
        "an invalid dry run",
        status,
        &answer,
        400,
        "InvalidParameterCombinationException",
    );

    // The dry run filed no receipt either, so the token it carried is still free for the
    // real request.
    let real = create_body(
        "dry-run-distribution",
        "token-dry-run",
        json!([{ "region": "us-west-2" }]),
        json!({}),
    );
    let (status, created) = create(&router, &real).await;
    assert_eq!(
        status, 200,
        "case \"the real create after the dry run\": got {status} with {created}"
    );
    let (status, read) = get(&router, &arn).await;
    assert_eq!(
        status, 200,
        "case \"a read after the real create\": got {status} with {read}"
    );
}

#[tokio::test]
async fn an_arn_nothing_was_stored_under_is_reported_missing() {
    let router = emulator();
    let body = create_body(
        "present-distribution",
        "token-present",
        json!([{ "region": "us-west-2" }]),
        json!({}),
    );
    let (status, created) = create(&router, &body).await;
    assert_eq!(status, 200, "case the create: got {status} with {created}");

    // Each row is a well-formed distribution-configuration ARN of this Region that names
    // no stored configuration.
    for (case, name) in [
        ("a name nothing created", "absent-distribution"),
        (
            "a name that differs from a stored one",
            "present-distribution-2",
        ),
    ] {
        let arn = arn_of(REGION, name);
        let (status, read) = get(&router, &arn).await;
        assert_refused(case, status, &read, 404, "ResourceNotFoundException");
        let expected = format!(
            "The following required resource 'DistributionConfiguration' cannot be found: \
             '{arn}'."
        );
        let actual = error_message(case, &read);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the message {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_pipeline_resolves_a_reference_to_a_stored_configuration() {
    let router = emulator();

    // The resources a pipeline needs beside the distribution configuration.
    let component = {
        let body = json!({
            "name": "pipeline-component",
            "semanticVersion": "1.0.0",
            "platform": "Linux",
            "clientToken": "token-component",
            "data": BUILD_DOCUMENT,
        });
        let (status, answer) = call_in(&router, REGION, "PUT", "/CreateComponent", Some(&body)).await;
        assert_eq!(
            status, 200,
            "case \"create the component\": got {status} with {answer}"
        );
        answer
            .get("componentBuildVersionArn")
            .and_then(Value::as_str)
            .expect("the component's build version ARN")
            .to_string()
    };
    let recipe = {
        let body = json!({
            "name": "pipeline-recipe",
            "semanticVersion": "1.0.0",
            "parentImage": "ami-0123456789abcdef0",
            "clientToken": "token-recipe",
            "components": [{ "componentArn": component }],
        });
        let (status, answer) =
            call_in(&router, REGION, "PUT", "/CreateImageRecipe", Some(&body)).await;
        assert_eq!(
            status, 200,
            "case \"create the image recipe\": got {status} with {answer}"
        );
        answer
            .get("imageRecipeArn")
            .and_then(Value::as_str)
            .expect("the recipe's ARN")
            .to_string()
    };
    let infrastructure = {
        let body = json!({
            "name": "pipeline-infra",
            "clientToken": "token-infra",
            "instanceProfileName": "EC2InstanceProfileForImageBuilder",
        });
        let (status, answer) = call_in(
            &router,
            REGION,
            "PUT",
            "/CreateInfrastructureConfiguration",
            Some(&body),
        )
        .await;
        assert_eq!(
            status, 200,
            "case \"create the infrastructure configuration\": got {status} with {answer}"
        );
        answer
            .get("infrastructureConfigurationArn")
            .and_then(Value::as_str)
            .expect("the configuration's ARN")
            .to_string()
    };
    let body = create_body(
        "pipeline-distribution",
        "token-pipeline-distribution",
        json!([{ "region": "us-west-2" }, { "region": "us-east-1" }]),
        json!({}),
    );
    let (status, created) = create(&router, &body).await;
    assert_eq!(
        status, 200,
        "case \"create the distribution configuration\": got {status} with {created}"
    );
    let distribution = arn_of(REGION, "pipeline-distribution");

    // (case, the distribution configuration the pipeline names, whether it is stored)
    let rows: Vec<(&str, String, bool)> = vec![
        (
            "the stored configuration",
            distribution.clone(),
            true,
        ),
        (
            "an ARN nothing was stored under",
            arn_of(REGION, "absent-distribution"),
            false,
        ),
        (
            "a configuration of another Region",
            arn_of("us-east-1", "pipeline-distribution"),
            false,
        ),
    ];

    for (n, (case, reference, stored)) in rows.into_iter().enumerate() {
        let name = format!("pipeline-{n}");
        let pipeline = json!({
            "name": name,
            "clientToken": format!("token-pipeline-{n}"),
            "imageRecipeArn": recipe,
            "infrastructureConfigurationArn": infrastructure,
            "distributionConfigurationArn": reference,
        });
        let (status, answer) =
            call_in(&router, REGION, "PUT", "/CreateImagePipeline", Some(&pipeline)).await;
        if !stored {
            assert_refused(case, status, &answer, 404, "ResourceNotFoundException");
            continue;
        }
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {answer}"
        );
        let pipeline_arn = answer
            .get("imagePipelineArn")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let (status, read) = call_in(
            &router,
            REGION,
            "GET",
            &format!("/GetImagePipeline?imagePipelineArn={pipeline_arn}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the pipeline read, got {status} with {read}"
        );
        let reported = read
            .get("imagePipeline")
            .and_then(|pipeline| pipeline.get("distributionConfigurationArn"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            reported, reference,
            "case {case:?}: expected the pipeline to report {reference:?}, got {reported:?}"
        );
    }
}
