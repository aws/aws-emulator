// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the infrastructure-configuration slice of this service.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: `CreateInfrastructureConfiguration` persists the configuration
//! the request describes under an ARN derived from a normalized form of its name, and
//! `GetInfrastructureConfiguration` reports that stored record — so every member a
//! create carried comes back unchanged, every member it omitted comes back as the
//! documented default or as absent, and an ARN nothing was stored under is reported
//! missing.
//!
//! Most of the EC2 values a configuration names are not resolved at create time: the
//! modelled `s3BucketName` documentation says the bucket "isn't validated when you create or
//! update the configuration", and the same holds for the instance types, the subnet and the
//! security groups — so the rows below name values no other service in this emulator holds
//! and still expect a 200. The one value that IS resolved is `instanceProfileName`, against
//! the account's own IAM, and a refusal is grounded in IAM's answer: this suite mounts
//! imagebuilder ALONE, so nothing answers that lookup, nothing about the profile is
//! established and no row is refused for it. What that resolution does when an IAM IS
//! mounted beside this service is asserted in `infrastructure_configuration_listing.rs`.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region a case signs for when it only needs one; the ARNs it creates carry it.
const REGION: &str = "us-west-2";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// Members a request carries that the stored configuration does not report back:
/// `clientToken` is echoed by the create response instead, and `dryRun` describes the
/// call rather than the resource.
const NOT_CONFIGURATION_MEMBERS: &[&str] = &["clientToken", "dryRun"];

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

/// A request signed for `imagebuilder` in `region` — the signal the router routes on
/// and the handlers read the Region from.
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
        .and_then(|v| v.to_str().ok())
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

/// `CreateInfrastructureConfiguration` with `body`, in `region`.
async fn create_in(router: &MultiServiceRouter, region: &str, body: &Value) -> (u16, Value) {
    call_in(
        router,
        region,
        "PUT",
        "/CreateInfrastructureConfiguration",
        Some(body),
    )
    .await
}

/// `CreateInfrastructureConfiguration` with `body`, in [`REGION`].
async fn create(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    create_in(router, REGION, body).await
}

/// `GetInfrastructureConfiguration` for `arn`, in `region`.
async fn get_in(router: &MultiServiceRouter, region: &str, arn: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "GET",
        &format!("/GetInfrastructureConfiguration?infrastructureConfigurationArn={arn}"),
        None,
    )
    .await
}

/// `GetInfrastructureConfiguration` for `arn`, in [`REGION`].
async fn get(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    get_in(router, REGION, arn).await
}

/// A create request body: the two required members, `clientToken`, and whatever else
/// `extra` names.
fn create_body(name: &str, client_token: &str, extra: Value) -> Value {
    let mut body = Map::new();
    body.insert("name".to_string(), json!(name));
    body.insert("clientToken".to_string(), json!(client_token));
    body.insert(
        "instanceProfileName".to_string(),
        json!("EC2InstanceProfileForImageBuilder"),
    );
    if let Value::Object(extra) = extra {
        for (member, value) in extra {
            body.insert(member, value);
        }
    }
    Value::Object(body)
}

/// The ARN a configuration named `normalized` carries in `region`.
fn arn_of(region: &str, normalized: &str) -> String {
    format!("arn:aws:imagebuilder:{region}:{ACCOUNT}:infrastructure-configuration/{normalized}")
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

/// Assert `status` and the error code of a refusal.
fn assert_refused(case: &str, status: u16, body: &Value, expect_status: u16, expect_code: &str) {
    assert_eq!(
        status, expect_status,
        "case {case:?}: expected HTTP {expect_status}, got {status} with {body}"
    );
    let code = error_code(case, body);
    assert_eq!(
        code, expect_code,
        "case {case:?}: expected the error code {expect_code:?}, got {code:?}"
    );
}

/// The `infrastructureConfiguration` of a successful read.
fn configuration(case: &str, body: &Value) -> Value {
    body.get("infrastructureConfiguration")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected an infrastructureConfiguration in {body}"))
}

/// Every member of `sent` that describes the RESOURCE, as the stored configuration
/// should report it back.
fn round_tripped_members(sent: &Value) -> Vec<(String, Value)> {
    sent.as_object()
        .expect("a create body is an object")
        .iter()
        .filter(|(member, _)| !NOT_CONFIGURATION_MEMBERS.contains(&member.as_str()))
        .map(|(member, value)| (member.clone(), value.clone()))
        .collect()
}

/// One row of the round-trip table: a create body and the normalized name its ARN
/// carries.
struct Row {
    case: &'static str,
    name: &'static str,
    normalized: &'static str,
    extra: fn() -> Value,
}

/// The round-trip table. The rows differ along what the rule generalises over: which
/// optional members are present at all, how many entries a list or a map carries, and
/// whether the values name anything that exists.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "only the required members",
            name: "minimal-infra",
            normalized: "minimal-infra",
            extra: || json!({}),
        },
        Row {
            case: "a subnet with one security group",
            name: "Single-Group Infra",
            normalized: "single-group-infra",
            extra: || {
                json!({
                    "subnetId": "subnet-0123456789abcdef0",
                    "securityGroupIds": ["sg-0123456789abcdef0"],
                })
            },
        },
        Row {
            case: "a subnet with three security groups and three instance types",
            name: "many_values_infra",
            normalized: "many-values-infra",
            extra: || {
                json!({
                    "subnetId": "subnet-00000000000000001",
                    "securityGroupIds": ["sg-00000000000000001", "sg-00000000000000002", "sg-00000000000000003"],
                    "instanceTypes": ["m5.large", "m6i.xlarge", "c7g.2xlarge"],
                })
            },
        },
        Row {
            case: "instance types and groups that name nothing in this account",
            name: "unknown-values-infra",
            normalized: "unknown-values-infra",
            extra: || {
                json!({
                    "instanceTypes": ["zz99.nonexistent"],
                    "subnetId": "subnet-ffffffffffffffffff",
                    "securityGroupIds": ["sg-ffffffffffffffffff"],
                })
            },
        },
        Row {
            case: "logging, a key pair and a description",
            name: "logged-infra",
            normalized: "logged-infra",
            extra: || {
                json!({
                    "description": "builds the golden image",
                    "keyPair": "image-builder-debug",
                    "logging": {"s3Logs": {"s3BucketName": "no-such-bucket", "s3KeyPrefix": "build-logs/"}},
                })
            },
        },
        Row {
            case: "both tag maps, at one and at three entries",
            name: "tagged-infra",
            normalized: "tagged-infra",
            extra: || {
                json!({
                    "tags": {"owner": "platform"},
                    "resourceTags": {"stage": "build", "team": "images", "costCenter": "4711"},
                })
            },
        },
        Row {
            case: "placement, instance metadata options and a parent instance",
            name: "placed-infra",
            normalized: "placed-infra",
            extra: || {
                json!({
                    // `tenancy` is one of the modelled `TenancyType` values
                    // (`default`, `dedicated`, `host`) and `httpTokens` one of the two
                    // the modelled `HttpTokens` pattern admits.
                    "placement": {"availabilityZone": "us-west-2c", "tenancy": "dedicated"},
                    "instanceMetadataOptions": {"httpTokens": "required", "httpPutResponseHopLimit": 2},
                    "parentInstance": {"autoScalingGroup": {"name": "image-builders", "capacityWaitTimeoutSeconds": 600}},
                })
            },
        },
        Row {
            case: "an SNS topic and both terminate flags turned off",
            name: "retained-infra",
            normalized: "retained-infra",
            extra: || {
                json!({
                    "snsTopicArn": format!("arn:aws:sns:{REGION}:{ACCOUNT}:image-builder-events"),
                    "terminateInstanceOnFailure": false,
                    "terminateInstanceOnCompletion": false,
                })
            },
        },
    ]
}

#[tokio::test]
async fn a_created_configuration_is_reported_back_member_for_member() {
    let router = emulator();

    for (n, row) in rows().into_iter().enumerate() {
        let body = create_body(row.name, &format!("token-{n:04}"), (row.extra)());
        let (status, created) = create(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from the create, got {status} with {created}",
            row.case
        );

        let expected_arn = arn_of(REGION, row.normalized);
        let actual_arn = created
            .get("infrastructureConfigurationArn")
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
        let config = configuration(row.case, &read);

        for (member, sent) in round_tripped_members(&body) {
            let reported = config.get(&member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                reported, sent,
                "case {:?}: expected the stored {member} to be {sent}, got {reported}",
                row.case
            );
        }

        let reported_arn = config.get("arn").and_then(Value::as_str).unwrap_or_default();
        assert_eq!(
            reported_arn, expected_arn,
            "case {:?}: expected the stored arn {expected_arn:?}, got {reported_arn:?}",
            row.case
        );
        let stamp = config
            .get("dateCreated")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            stamp.len() >= 20 && stamp.ends_with('Z') && stamp.contains('T'),
            "case {:?}: expected dateCreated to be an ISO 8601 UTC instant, got {stamp:?}",
            row.case
        );
        // `dateUpdated` is "the date on which the infrastructure configuration was last
        // updated", and no update has run, so the read reports no such member.
        assert_eq!(
            config.get("dateUpdated"),
            None,
            "case {:?}: expected no dateUpdated before any update, got {:?}",
            row.case,
            config.get("dateUpdated")
        );
    }
}

#[tokio::test]
async fn a_read_reports_the_same_configuration_every_time() {
    let router = emulator();
    let body = create_body(
        "stable-infra",
        "token-stable",
        json!({"tags": {"owner": "platform"}}),
    );
    let (status, created) = create(&router, &body).await;
    assert_eq!(status, 200, "case the create: got {status} with {created}");
    let arn = arn_of(REGION, "stable-infra");

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
        "Golden Image Infra",
        "golden_image_infra",
        "GOLDEN-IMAGE-INFRA",
        "Golden-Image_Infra",
    ];
    let expected = arn_of(REGION, "golden-image-infra");

    for (n, name) in equivalent.iter().enumerate() {
        let router = emulator();
        let (status, created) = create(
            &router,
            &create_body(name, &format!("token-{n:04}"), json!({})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200, got {status} with {created}"
        );
        let actual = created
            .get("infrastructureConfigurationArn")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            actual, expected,
            "case {name:?}: expected the ARN {expected:?}, got {actual:?}"
        );

        // The second create under an equivalent name is the same name, so it is
        // refused rather than overwriting the first.
        for other in equivalent {
            let (status, body) = create(
                &router,
                &create_body(other, &format!("token-retry-{other}"), json!({})),
            )
            .await;
            assert_refused(
                other,
                status,
                &body,
                400,
                "ResourceAlreadyExistsException",
            );
            // The already-exists refusal of THIS create names no resource: the whole
            // sentence is fixed text, so neither the submitted name nor the stored ARN
            // appears in it.
            let message = body
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert_eq!(
                message, "The resource already exists.",
                "case {other:?}: expected the fixed already-exists sentence, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_different_name_is_a_different_configuration() {
    let router = emulator();
    let names: &[(&str, &str)] = &[
        ("alpha-infra", "alpha-infra"),
        ("beta infra", "beta-infra"),
        ("gamma_infra", "gamma-infra"),
    ];
    for (n, (name, _normalized)) in names.iter().enumerate() {
        let (status, created) = create(
            &router,
            &create_body(
                name,
                &format!("token-{n:04}"),
                json!({"description": format!("the {name} environment")}),
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200, got {status} with {created}"
        );
    }
    for (name, normalized) in names {
        let (status, read) = get(&router, &arn_of(REGION, normalized)).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200 from the read, got {status} with {read}"
        );
        let config = configuration(name, &read);
        let reported = config.get("name").and_then(Value::as_str).unwrap_or_default();
        assert_eq!(
            reported, *name,
            "case {name:?}: expected the stored name {name:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn the_terminate_flags_report_what_the_store_holds() {
    // `terminateInstanceOnFailure` documents "Defaults to true" and the service writes
    // that default into the stored configuration, so the read reports it however the
    // request left it. `terminateInstanceOnCompletion` carries no value of the service's
    // own: the store holds one only once a request supplies it, and a read of a
    // configuration no request set it on reports no such member at all.
    //
    // (case, what the create carried, the reported failure flag, the reported completion
    // flag — `None` for a member the body does not carry)
    let cases: &[(&str, Value, bool, Option<bool>)] = &[
        ("neither flag sent", json!({}), true, None),
        (
            "both sent true",
            json!({"terminateInstanceOnFailure": true, "terminateInstanceOnCompletion": true}),
            true,
            Some(true),
        ),
        (
            "both sent false",
            json!({"terminateInstanceOnFailure": false, "terminateInstanceOnCompletion": false}),
            false,
            Some(false),
        ),
        (
            "only failure sent false",
            json!({"terminateInstanceOnFailure": false}),
            false,
            None,
        ),
        (
            "only completion sent false",
            json!({"terminateInstanceOnCompletion": false}),
            true,
            Some(false),
        ),
        (
            "only completion sent true",
            json!({"terminateInstanceOnCompletion": true}),
            true,
            Some(true),
        ),
    ];

    for (n, (case, extra, on_failure, on_completion)) in cases.iter().enumerate() {
        let router = emulator();
        let name = format!("flags-infra-{n:02}");
        let (status, created) = create(
            &router,
            &create_body(&name, &format!("token-{n:04}"), extra.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {created}"
        );

        let (_, read) = get(&router, &arn_of(REGION, &name)).await;
        let config = configuration(case, &read);
        let reported_failure = config
            .get("terminateInstanceOnFailure")
            .and_then(Value::as_bool);
        assert_eq!(
            reported_failure,
            Some(*on_failure),
            "case {case:?}: expected terminateInstanceOnFailure to be {on_failure}, got \
             {reported_failure:?}"
        );
        let reported_completion = config
            .get("terminateInstanceOnCompletion")
            .and_then(Value::as_bool);
        assert_eq!(
            reported_completion, *on_completion,
            "case {case:?}: expected terminateInstanceOnCompletion to be {on_completion:?}, got \
             {reported_completion:?}"
        );
    }
}

#[tokio::test]
async fn a_subnet_needs_at_least_one_security_group() {
    // `subnetId`'s own rule: "If you specify subnetId, you must also specify one or
    // more security group IDs in securityGroupIds. Otherwise, the request fails."
    let cases: &[(&str, Value, Option<u16>)] = &[
        ("no subnet and no groups", json!({}), None),
        (
            "no subnet but groups anyway",
            json!({"securityGroupIds": ["sg-0123456789abcdef0"]}),
            None,
        ),
        (
            "a subnet and one group",
            json!({"subnetId": "subnet-0123456789abcdef0", "securityGroupIds": ["sg-0123456789abcdef0"]}),
            None,
        ),
        (
            "a subnet and two groups",
            json!({"subnetId": "subnet-0123456789abcdef0", "securityGroupIds": ["sg-1", "sg-2"]}),
            None,
        ),
        (
            "a subnet and no groups member",
            json!({"subnetId": "subnet-0123456789abcdef0"}),
            Some(400),
        ),
        (
            "a subnet and an empty group list",
            json!({"subnetId": "subnet-0123456789abcdef0", "securityGroupIds": []}),
            Some(400),
        ),
    ];

    for (n, (case, extra, refused)) in cases.iter().enumerate() {
        let router = emulator();
        let name = format!("subnet-infra-{n:02}");
        let (status, body) = create(
            &router,
            &create_body(&name, &format!("token-{n:04}"), extra.clone()),
        )
        .await;
        match refused {
            Some(expect_status) => {
                assert_refused(case, status, &body, *expect_status, "InvalidRequestException");
                // A refused create stores nothing.
                let (status, read) = get(&router, &arn_of(REGION, &name)).await;
                assert_eq!(
                    status, 404,
                    "case {case:?}: expected a refused create to store nothing, got {status} with \
                     {read}"
                );
            }
            None => assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {body}"
            ),
        }
    }
}

#[tokio::test]
async fn a_reserved_instance_tag_key_is_refused() {
    // From `resourceTags`: "Tag keys can't begin with aws: or match one of the
    // following reserved keys: CreatedBy, Ec2ImageBuilderArn, Name, or Tags." The
    // `aws:` prefix is already refused by the modelled key pattern; these four are
    // refused by the handler. `tags` carries no such rule, so the same keys are
    // accepted there.
    let cases: &[(&str, Value, Option<u16>)] = &[
        (
            "the reserved key CreatedBy",
            json!({"resourceTags": {"CreatedBy": "me"}}),
            Some(400),
        ),
        (
            "the reserved key Ec2ImageBuilderArn",
            json!({"resourceTags": {"Ec2ImageBuilderArn": "x"}}),
            Some(400),
        ),
        (
            "the reserved key Name",
            json!({"resourceTags": {"Name": "x"}}),
            Some(400),
        ),
        (
            "the reserved key Tags",
            json!({"resourceTags": {"Tags": "x"}}),
            Some(400),
        ),
        (
            "a reserved key beside an allowed one",
            json!({"resourceTags": {"stage": "build", "Name": "x"}}),
            Some(400),
        ),
        (
            "a lowercase near-miss of a reserved key",
            json!({"resourceTags": {"createdby": "me"}}),
            None,
        ),
        (
            "the same keys on the resource's own tags",
            json!({"tags": {"Name": "x", "CreatedBy": "me"}}),
            None,
        ),
    ];

    for (n, (case, extra, refused)) in cases.iter().enumerate() {
        let router = emulator();
        let name = format!("tag-infra-{n:02}");
        let (status, body) = create(
            &router,
            &create_body(&name, &format!("token-{n:04}"), extra.clone()),
        )
        .await;
        match refused {
            Some(expect_status) => {
                assert_refused(case, status, &body, *expect_status, "InvalidRequestException");
                let (status, read) = get(&router, &arn_of(REGION, &name)).await;
                assert_eq!(
                    status, 404,
                    "case {case:?}: expected a refused create to store nothing, got {status} with \
                     {read}"
                );
            }
            None => assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {body}"
            ),
        }
    }
}

#[tokio::test]
async fn a_retry_with_the_same_client_token_answers_the_original_response() {
    // "If you retry a request with the same client token, Image Builder returns the
    // original response without running the operation again."
    let router = emulator();
    let body = create_body(
        "idempotent-infra",
        "token-idempotent",
        json!({"description": "the original request", "instanceTypes": ["m5.large"]}),
    );

    let (status, first) = create(&router, &body).await;
    assert_eq!(status, 200, "case the first create: got {status} with {first}");

    for attempt in 0..3 {
        let (status, again) = create(&router, &body).await;
        assert_eq!(
            status, 200,
            "case retry {attempt}: expected HTTP 200, got {status} with {again}"
        );
        assert_eq!(
            again, first,
            "case retry {attempt}: expected the original response, got {again} after {first}"
        );
    }
}

#[tokio::test]
async fn the_same_client_token_with_different_parameters_is_a_mismatch() {
    // Each row differs from the original request in ONE creation parameter, along what
    // the rule generalises over: a string, a list, a map, a flag and a nested
    // structure.
    let original = || {
        json!({
            "description": "the original request",
            "instanceTypes": ["m5.large"],
            "tags": {"owner": "platform"},
            "terminateInstanceOnFailure": true,
            "logging": {"s3Logs": {"s3BucketName": "logs", "s3KeyPrefix": "build/"}},
        })
    };
    let variants: &[(&str, &str, Value)] = &[
        (
            "a different description",
            "idempotent-infra",
            json!({"description": "a different request"}),
        ),
        (
            "a different name",
            "other-infra",
            json!({"description": "the original request"}),
        ),
        (
            "an extra instance type",
            "idempotent-infra",
            json!({"instanceTypes": ["m5.large", "m6i.large"]}),
        ),
        (
            "a reordered instance type list",
            "idempotent-infra",
            json!({"instanceTypes": ["m6i.large", "m5.large"]}),
        ),
        (
            "a different tag value",
            "idempotent-infra",
            json!({"tags": {"owner": "security"}}),
        ),
        (
            "a flipped flag",
            "idempotent-infra",
            json!({"terminateInstanceOnFailure": false}),
        ),
        (
            "a different log prefix",
            "idempotent-infra",
            json!({"logging": {"s3Logs": {"s3BucketName": "logs", "s3KeyPrefix": "other/"}}}),
        ),
    ];

    for (case, name, change) in variants {
        let router = emulator();
        let (status, first) = create(
            &router,
            &create_body("idempotent-infra", "token-shared", original()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first create to succeed, got {status} with {first}"
        );

        let mut extra = original();
        if let (Value::Object(extra), Value::Object(change)) = (&mut extra, change) {
            for (member, value) in change {
                extra.insert(member.clone(), value.clone());
            }
        }
        let (status, body) = create(&router, &create_body(name, "token-shared", extra)).await;
        assert_refused(
            case,
            status,
            &body,
            400,
            "IdempotentParameterMismatchException",
        );
    }

    // The same parameters under a FRESH token mint a second configuration, so the
    // mismatch is about the token and not about the parameters.
    let router = emulator();
    let (status, _) = create(
        &router,
        &create_body("idempotent-infra", "token-one", original()),
    )
    .await;
    assert_eq!(status, 200, "case the first token: got {status}");
    let (status, body) = create(
        &router,
        &create_body("second-infra", "token-two", original()),
    )
    .await;
    assert_eq!(
        status, 200,
        "case a fresh token: expected HTTP 200, got {status} with {body}"
    );
}

#[tokio::test]
async fn a_dry_run_validates_and_stores_nothing() {
    // `dryRun`: "Validates the required permissions and request parameters without
    // performing the operation. If validation succeeds, the operation returns a
    // DryRunOperationException error response."
    let router = emulator();
    let arn = arn_of(REGION, "dry-run-infra");

    let (status, body) = create(
        &router,
        &create_body(
            "dry-run-infra",
            "token-dry",
            json!({"dryRun": true, "subnetId": "subnet-1", "securityGroupIds": ["sg-1"]}),
        ),
    )
    .await;
    assert_refused(
        "a valid dry run",
        status,
        &body,
        412,
        "DryRunOperationException",
    );

    let (status, read) = get(&router, &arn).await;
    assert_eq!(
        status, 404,
        "case after a dry run: expected nothing stored, got {status} with {read}"
    );

    // A dry run of an INVALID request is refused on the request, not reported as a
    // successful validation.
    let (status, body) = create(
        &router,
        &create_body(
            "dry-run-infra",
            "token-dry-invalid",
            json!({"dryRun": true, "subnetId": "subnet-1"}),
        ),
    )
    .await;
    assert_refused(
        "an invalid dry run",
        status,
        &body,
        400,
        "InvalidRequestException",
    );

    // The dry run left the name and the token free, so the real request succeeds.
    let (status, created) = create(
        &router,
        &create_body(
            "dry-run-infra",
            "token-dry",
            json!({"subnetId": "subnet-1", "securityGroupIds": ["sg-1"]}),
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the real create: expected HTTP 200, got {status} with {created}"
    );
    let (status, read) = get(&router, &arn).await;
    assert_eq!(
        status, 200,
        "case after the real create: expected HTTP 200, got {status} with {read}"
    );
}

#[tokio::test]
async fn an_arn_that_names_no_configuration_is_reported_missing() {
    let router = emulator();
    let (status, created) = create(&router, &create_body("stored-infra", "token-1", json!({}))).await;
    assert_eq!(status, 200, "case the create: got {status} with {created}");

    let cases: &[(&str, String)] = &[
        ("a name nothing was stored under", arn_of(REGION, "absent-infra")),
        (
            "the stored name in another Region",
            arn_of("eu-west-1", "stored-infra"),
        ),
        (
            "a name that differs only in its suffix",
            arn_of(REGION, "stored-infra-2"),
        ),
    ];
    for (case, arn) in cases {
        let (status, body) = get(&router, arn).await;
        assert_refused(case, status, &body, 404, "ResourceNotFoundException");
    }
}

#[tokio::test]
async fn a_configuration_belongs_to_the_region_that_created_it() {
    // This service is regional, so the same name is free in every Region and a
    // configuration is reachable only through its own.
    let router = emulator();
    let regions: &[&str] = &["us-west-2", "us-east-1", "eu-central-1"];

    for (n, region) in regions.iter().enumerate() {
        let (status, created) = create_in(
            &router,
            region,
            &create_body(
                "regional-infra",
                &format!("token-{n:04}"),
                json!({"description": format!("the {region} environment")}),
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected HTTP 200, got {status} with {created}"
        );
        let actual = created
            .get("infrastructureConfigurationArn")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let expected = arn_of(region, "regional-infra");
        assert_eq!(
            actual, expected,
            "case {region:?}: expected the ARN {expected:?}, got {actual:?}"
        );
    }

    for region in regions {
        let arn = arn_of(region, "regional-infra");
        let (status, read) = get_in(&router, region, &arn).await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected its own configuration to be readable, got {status} with \
             {read}"
        );
        let config = configuration(region, &read);
        let description = config
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            description,
            format!("the {region} environment"),
            "case {region:?}: expected the Region's own record, got {description:?}"
        );

        for other in regions.iter().filter(|other| *other != region) {
            let (status, body) = get_in(&router, other, &arn).await;
            assert_refused(
                &format!("{arn} read from {other}"),
                status,
                &body,
                404,
                "ResourceNotFoundException",
            );
        }
    }
}
