// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateInfrastructureConfiguration`.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes.
//!
//! The rules under test:
//!
//! * an accepted update REPLACES the stored configuration: "This operation doesn't
//!   support selective updates. The request replaces the configuration, so include every
//!   setting that you want to keep. Omitted optional properties are cleared" — so every
//!   setting the request carried is what the following read reports, as submitted, and
//!   every optional one it omitted comes back cleared;
//! * none of those settings is resolved against anything: the model puts no existence
//!   constraint on `instanceTypes`, `subnetId` or `securityGroupIds` and declares no
//!   "not found" error for them, so a syntactically valid identifier that names nothing in
//!   the account is stored and read back verbatim;
//! * `dateUpdated` is "the date on which the infrastructure configuration was last
//!   updated", so the read reports no such member until an update has run and the instant
//!   of the last one after that, while `dateCreated` never moves.
//!
//! This suite mounts imagebuilder ALONE, so nothing answers the `instanceProfileName`
//! lookup, nothing about the profile is established and no row is refused for it — the
//! same ground `infrastructure_configuration.rs` states.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region a case signs for; the ARNs it creates carry it.
const REGION: &str = "us-west-2";

/// A Region no case creates anything in, used to show an update reaches only its own
/// Region's configurations.
const OTHER_REGION: &str = "eu-west-1";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// The instance profile every body names; nothing resolves it in this suite.
const PROFILE: &str = "EC2InstanceProfileForImageBuilder";

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
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// `CreateInfrastructureConfiguration` with `body`, in [`REGION`].
async fn create(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call_in(
        router,
        REGION,
        "PUT",
        "/CreateInfrastructureConfiguration",
        Some(body),
    )
    .await
}

/// `UpdateInfrastructureConfiguration` with `body`, in `region`.
async fn update_in(router: &MultiServiceRouter, region: &str, body: &Value) -> (u16, Value) {
    call_in(
        router,
        region,
        "PUT",
        "/UpdateInfrastructureConfiguration",
        Some(body),
    )
    .await
}

/// `UpdateInfrastructureConfiguration` with `body`, in [`REGION`].
async fn update(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    update_in(router, REGION, body).await
}

/// `GetInfrastructureConfiguration` for `arn`, in [`REGION`].
async fn get(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call_in(
        router,
        REGION,
        "GET",
        &format!("/GetInfrastructureConfiguration?infrastructureConfigurationArn={arn}"),
        None,
    )
    .await
}

/// A create body: the required members, `clientToken`, and whatever else `extra` names.
fn create_body(name: &str, client_token: &str, extra: Value) -> Value {
    let mut body = Map::new();
    body.insert("name".to_string(), json!(name));
    body.insert("clientToken".to_string(), json!(client_token));
    body.insert("instanceProfileName".to_string(), json!(PROFILE));
    merge(&mut body, extra);
    Value::Object(body)
}

/// An update body: the required members, `clientToken`, and whatever else `extra` names.
fn update_body(arn: &str, client_token: &str, extra: Value) -> Value {
    let mut body = Map::new();
    body.insert(
        "infrastructureConfigurationArn".to_string(),
        json!(arn),
    );
    body.insert("clientToken".to_string(), json!(client_token));
    body.insert("instanceProfileName".to_string(), json!(PROFILE));
    merge(&mut body, extra);
    Value::Object(body)
}

/// Fold the members of `extra` into `body`.
fn merge(body: &mut Map<String, Value>, extra: Value) {
    if let Value::Object(extra) = extra {
        for (member, value) in extra {
            body.insert(member, value);
        }
    }
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

/// Create one configuration named `name` carrying `extra`, and answer its ARN.
async fn stored(router: &MultiServiceRouter, name: &str, extra: Value) -> String {
    let (status, created) = create(router, &create_body(name, &format!("create-{name}"), extra)).await;
    assert_eq!(
        status, 200,
        "case {name:?}: expected HTTP 200 from the create, got {status} with {created}"
    );
    arn_of(REGION, name)
}

/// What the configuration this suite creates before each update carries, so every row
/// starts from the same stored state and the replacement is visible member by member.
fn created_settings() -> Value {
    json!({
        "description": "the configuration as created",
        "instanceTypes": ["t3.medium"],
        "subnetId": "subnet-00000000000000001",
        "securityGroupIds": ["sg-00000000000000001"],
        "keyPair": "created-key",
        "terminateInstanceOnFailure": false,
        "terminateInstanceOnCompletion": false,
        "resourceTags": {"stage": "created"},
        "logging": {"s3Logs": {"s3BucketName": "created-logs", "s3KeyPrefix": "created/"}},
    })
}

/// One row of the replacement table: what the update submits, and the members the
/// following read must report.
struct Row {
    case: &'static str,
    /// The settings the update carries beside the two required members.
    submitted: fn() -> Value,
}

/// The replacement table. The rows differ along what the rule generalises over: which
/// member is replaced, whether its value names anything that exists, how many entries a
/// list carries, and whether the request carries the member at all.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an instance type no account holds",
            submitted: || json!({"instanceTypes": ["t3.nonesuch"]}),
        },
        Row {
            case: "several instance types, one of them unknown",
            submitted: || json!({"instanceTypes": ["m5.large", "c6g.xlarge", "zz9.plural-z"]}),
        },
        Row {
            case: "a subnet and a security group that resolve to nothing",
            submitted: || {
                json!({
                    "subnetId": "subnet-0123456789abcdef0",
                    "securityGroupIds": ["sg-0123456789abcdef0"],
                })
            },
        },
        Row {
            case: "several security groups beside a subnet",
            submitted: || {
                json!({
                    "subnetId": "subnet-0fedcba9876543210",
                    "securityGroupIds": [
                        "sg-0123456789abcdef0",
                        "sg-0fedcba9876543210",
                        "sg-00000000000000002",
                    ],
                })
            },
        },
        Row {
            case: "every member the update can carry at once",
            submitted: || {
                json!({
                    "description": "the configuration as updated",
                    "instanceTypes": ["t3.nonesuch", "m6i.2xlarge"],
                    "subnetId": "subnet-0123456789abcdef0",
                    "securityGroupIds": ["sg-0123456789abcdef0"],
                    "keyPair": "updated-key",
                    "terminateInstanceOnFailure": true,
                    "terminateInstanceOnCompletion": true,
                    "resourceTags": {"stage": "updated", "owner": "platform"},
                    "snsTopicArn": format!("arn:aws:sns:{REGION}:{ACCOUNT}:build-events"),
                    "logging": {
                        "s3Logs": {"s3BucketName": "updated-logs", "s3KeyPrefix": "updated/"},
                    },
                    "instanceMetadataOptions": {
                        "httpTokens": "required",
                        "httpPutResponseHopLimit": 2,
                    },
                    "placement": {"availabilityZone": format!("{REGION}a"), "tenancy": "default"},
                })
            },
        },
        Row {
            case: "nothing but the required members, clearing every optional one",
            submitted: || json!({}),
        },
    ]
}

/// The members an update body describes the RESOURCE with: the ones a following read
/// reports back. `clientToken` is echoed by the update response instead, and
/// `infrastructureConfigurationArn` is the resource's own `arn`.
const NOT_CONFIGURATION_MEMBERS: &[&str] =
    &["clientToken", "infrastructureConfigurationArn"];

/// Every optional member of the stored configuration an update can carry, so a row that
/// omits one can be checked for the clearing the operation documents.
const REPLACED_MEMBERS: &[&str] = &[
    "description",
    "instanceTypes",
    "subnetId",
    "securityGroupIds",
    "keyPair",
    "terminateInstanceOnCompletion",
    "snsTopicArn",
    "resourceTags",
    "instanceMetadataOptions",
    "placement",
    "parentInstance",
    "logging",
];

/// RULE: an accepted update replaces the configuration, so the read reports what the
/// update submitted — verbatim, whatever the value names — and nothing of what the
/// create had stored under a member the update omitted.
#[tokio::test]
async fn an_accepted_update_is_what_the_read_reports() {
    for (n, row) in rows().into_iter().enumerate() {
        let router = emulator();
        let name = format!("replaced-infra-{n:02}");
        let arn = stored(&router, &name, created_settings()).await;

        let body = update_body(&arn, &format!("update-{n:04}"), (row.submitted)());
        let (status, updated) = update(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from the update, got {status} with {updated}",
            row.case
        );
        assert_eq!(
            updated.get("infrastructureConfigurationArn"),
            Some(&json!(arn)),
            "case {:?}: expected the update to answer with the configuration's own arn, got \
             {updated}",
            row.case
        );

        let (status, read) = get(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from the read, got {status} with {read}",
            row.case
        );
        let config = configuration(row.case, &read);

        let sent = body.as_object().expect("an update body is an object");
        for (member, value) in sent
            .iter()
            .filter(|(member, _)| !NOT_CONFIGURATION_MEMBERS.contains(&member.as_str()))
        {
            let reported = config.get(member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                reported, *value,
                "case {:?}: expected the stored {member} to be {value}, got {reported}",
                row.case
            );
        }
        // "Omitted optional properties are cleared": a member this update did not carry
        // is absent from the read, whatever the create had stored under it.
        for member in REPLACED_MEMBERS
            .iter()
            .filter(|member| !sent.contains_key(**member))
        {
            assert!(
                config.get(*member).is_none(),
                "case {:?}: expected {member} cleared by the update, got {}",
                row.case,
                config[*member]
            );
        }
        // The name and the ARN are not update parameters, so they are what the create
        // established.
        assert_eq!(
            config.get("name"),
            Some(&json!(name)),
            "case {:?}: expected the create's name to survive the update, got {config}",
            row.case
        );
        assert_eq!(
            config.get("arn"),
            Some(&json!(arn)),
            "case {:?}: expected the configuration's own arn, got {config}",
            row.case
        );
    }
}

/// RULE: `terminateInstanceOnFailure` documents the value the service applies when a
/// request omits it, so a replacement that omits it carries that value; its sibling
/// carries no value of the service's own, so a replacement that omits it leaves the
/// configuration with none.
#[tokio::test]
async fn the_terminate_flags_follow_the_replacement() {
    // (case, what the update carried, the reported failure flag, the reported completion
    // flag — `None` for a member the body does not carry)
    let cases: &[(&str, Value, bool, Option<bool>)] = &[
        ("neither flag sent", json!({}), true, None),
        (
            "both sent false",
            json!({"terminateInstanceOnFailure": false, "terminateInstanceOnCompletion": false}),
            false,
            Some(false),
        ),
        (
            "both sent true",
            json!({"terminateInstanceOnFailure": true, "terminateInstanceOnCompletion": true}),
            true,
            Some(true),
        ),
        (
            "only completion sent",
            json!({"terminateInstanceOnCompletion": false}),
            true,
            Some(false),
        ),
        (
            "only failure sent",
            json!({"terminateInstanceOnFailure": false}),
            false,
            None,
        ),
    ];

    for (n, (case, submitted, on_failure, on_completion)) in cases.iter().enumerate() {
        let router = emulator();
        let name = format!("flagged-infra-{n:02}");
        let arn = stored(&router, &name, created_settings()).await;

        let (status, updated) = update(
            &router,
            &update_body(&arn, &format!("flags-{n:04}"), submitted.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the update, got {status} with {updated}"
        );

        let (_, read) = get(&router, &arn).await;
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

/// RULE: `dateUpdated` reports when an update last replaced the configuration — absent
/// until one has, and the instant of the latest one after that — while `dateCreated`
/// never moves.
#[tokio::test]
async fn date_updated_appears_only_once_an_update_has_run() {
    let router = emulator();
    let arn = stored(&router, "dated-infra", created_settings()).await;

    let (_, before) = get(&router, &arn).await;
    let before = configuration("before any update", &before);
    assert!(
        before.get("dateUpdated").is_none(),
        "case \"before any update\": expected no dateUpdated, got {before}"
    );
    let created_at = before
        .get("dateCreated")
        .and_then(Value::as_str)
        .expect("a created configuration reports when it was created")
        .to_string();

    // Two updates in a row: each one is the last write, so each is what `dateUpdated`
    // reports, and the instant never goes backwards.
    let mut stamps: Vec<String> = Vec::new();
    for (n, description) in ["the first update", "the second update"].iter().enumerate() {
        let (status, updated) = update(
            &router,
            &update_body(
                &arn,
                &format!("dated-{n:04}"),
                json!({"description": *description}),
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {description:?}: expected HTTP 200, got {status} with {updated}"
        );

        let (_, read) = get(&router, &arn).await;
        let config = configuration(description, &read);
        let stamp = config
            .get("dateUpdated")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {description:?}: expected a dateUpdated in {config}"))
            .to_string();
        assert!(
            stamp.len() >= 20 && stamp.ends_with('Z') && stamp.contains('T'),
            "case {description:?}: expected an ISO 8601 UTC instant, got {stamp:?}"
        );
        assert_eq!(
            config.get("dateCreated"),
            Some(&json!(created_at)),
            "case {description:?}: expected dateCreated to stay {created_at:?}, got {config}"
        );
        stamps.push(stamp);
    }
    assert!(
        stamps[1] >= stamps[0],
        "case \"two updates in a row\": expected the later dateUpdated {:?} not to precede the \
         earlier {:?}",
        stamps[1],
        stamps[0]
    );
}

/// RULE: an ARN this Region holds no configuration under names nothing to replace, so the
/// update reports it missing and writes nothing.
#[tokio::test]
async fn an_arn_that_names_no_configuration_is_reported_missing() {
    let router = emulator();
    let arn = stored(&router, "present-infra", created_settings()).await;

    // (case, the ARN the update names, the Region it is signed for)
    let cases: &[(&str, String, &str)] = &[
        (
            "a name nothing was stored under",
            arn_of(REGION, "absent-infra"),
            REGION,
        ),
        (
            "another Region's endpoint for a stored configuration",
            arn.clone(),
            OTHER_REGION,
        ),
        (
            "another Region's own ARN",
            arn_of(OTHER_REGION, "present-infra"),
            REGION,
        ),
    ];

    for (n, (case, named, region)) in cases.iter().enumerate() {
        let (status, body) = update_in(
            &router,
            region,
            &update_body(
                named,
                &format!("missing-{n:04}"),
                json!({"description": "an update nothing receives"}),
            ),
        )
        .await;
        assert_refused(case, status, &body, 404, "ResourceNotFoundException");
        let message = body.get("message").and_then(Value::as_str).unwrap_or_default();
        assert!(
            message.contains(named.as_str())
                && message.contains("InfrastructureConfiguration"),
            "case {case:?}: expected the missing resource and its kind named, got {message:?}"
        );
    }

    // The configuration that does exist is untouched by any of them.
    let (_, read) = get(&router, &arn).await;
    let config = configuration("the stored configuration afterwards", &read);
    assert_eq!(
        config.get("description"),
        Some(&json!("the configuration as created")),
        "case \"the stored configuration afterwards\": expected the create's settings, got {config}"
    );
    assert!(
        config.get("dateUpdated").is_none(),
        "case \"the stored configuration afterwards\": expected no dateUpdated, got {config}"
    );
}

/// RULE: a request the operation refuses writes nothing — the stored configuration is
/// still the one the create wrote.
#[tokio::test]
async fn a_refused_update_leaves_the_configuration_as_it_was() {
    // (case, what the update carried, the status, the error code)
    let cases: &[(&str, Value, u16, &str)] = &[
        (
            // `subnetId`: "If you specify subnetId, you must also specify one or more
            // security group IDs in securityGroupIds. Otherwise, the request fails."
            "a subnet with no security groups",
            json!({"subnetId": "subnet-0123456789abcdef0"}),
            400,
            "InvalidRequestException",
        ),
        (
            "a subnet with an empty security group list",
            json!({"subnetId": "subnet-0123456789abcdef0", "securityGroupIds": []}),
            400,
            "InvalidRequestException",
        ),
        (
            // From `resourceTags`: "Tag keys can't begin with aws: or match one of the
            // following reserved keys: CreatedBy, Ec2ImageBuilderArn, Name, or Tags."
            "a reserved instance-tag key",
            json!({"resourceTags": {"Ec2ImageBuilderArn": "anything"}}),
            400,
            "InvalidRequestException",
        ),
        (
            "an ARN of the right shape naming another resource kind",
            json!({}),
            400,
            "InvalidParameterValueException",
        ),
    ];

    for (n, (case, submitted, expect_status, expect_code)) in cases.iter().enumerate() {
        let router = emulator();
        let name = format!("refused-infra-{n:02}");
        let arn = stored(&router, &name, created_settings()).await;
        // The last row refuses the ARN itself, which the modelled pattern walks before the
        // handler runs; every other row carries the stored configuration's own ARN.
        let named = if *expect_code == "InvalidParameterValueException" {
            format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:image-pipeline/{name}")
        } else {
            arn.clone()
        };

        let (status, body) = update(
            &router,
            &update_body(&named, &format!("refused-{n:04}"), submitted.clone()),
        )
        .await;
        assert_refused(case, status, &body, *expect_status, expect_code);

        let (_, read) = get(&router, &arn).await;
        let config = configuration(case, &read);
        for (member, expected) in [
            ("description", json!("the configuration as created")),
            ("instanceTypes", json!(["t3.medium"])),
            ("subnetId", json!("subnet-00000000000000001")),
            ("securityGroupIds", json!(["sg-00000000000000001"])),
            ("resourceTags", json!({"stage": "created"})),
        ] {
            let reported = config.get(member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                reported, expected,
                "case {case:?}: expected the create's {member} {expected}, got {reported}"
            );
        }
        assert!(
            config.get("dateUpdated").is_none(),
            "case {case:?}: expected no dateUpdated after a refused update, got {config}"
        );
    }
}

/// RULE: `clientToken` is an idempotency token — "If you retry a request with the same
/// client token, Image Builder returns the original response without running the
/// operation again", and the same token carrying different settings is a mismatch. The
/// update's tokens are its own: one spent on the create does not answer for it.
#[tokio::test]
async fn a_client_token_answers_for_one_update_only() {
    let router = emulator();
    let arn = stored(&router, "idempotent-infra", created_settings()).await;

    let first = update_body(
        &arn,
        "shared-token",
        json!({"description": "the original update", "instanceTypes": ["t3.nonesuch"]}),
    );
    let (status, original) = update(&router, &first).await;
    assert_eq!(
        status, 200,
        "case \"the original update\": expected HTTP 200, got {status} with {original}"
    );

    // The retry answers the ORIGINAL response, down to its requestId.
    let (status, retried) = update(&router, &first).await;
    assert_eq!(
        status, 200,
        "case \"the retry\": expected HTTP 200, got {status} with {retried}"
    );
    assert_eq!(
        retried, original,
        "case \"the retry\": expected the original response {original}, got {retried}"
    );

    // Each row reuses the token with ONE setting changed, along what the rule generalises
    // over: a string, a list, a flag and a nested structure.
    let mismatched: &[(&str, Value)] = &[
        (
            "a different description",
            json!({"description": "a different update", "instanceTypes": ["t3.nonesuch"]}),
        ),
        (
            "a different instance type",
            json!({"description": "the original update", "instanceTypes": ["t3.medium"]}),
        ),
        (
            "an added flag",
            json!({
                "description": "the original update",
                "instanceTypes": ["t3.nonesuch"],
                "terminateInstanceOnCompletion": true,
            }),
        ),
        (
            "an added logging configuration",
            json!({
                "description": "the original update",
                "instanceTypes": ["t3.nonesuch"],
                "logging": {"s3Logs": {"s3BucketName": "other-logs"}},
            }),
        ),
    ];
    for (case, submitted) in mismatched {
        let (status, body) = update(&router, &update_body(&arn, "shared-token", submitted.clone()))
            .await;
        assert_refused(
            case,
            status,
            &body,
            400,
            "IdempotentParameterMismatchException",
        );
    }

    // The token the CREATE spent is free for an update: the two operations file their
    // receipts apart.
    let (status, body) = update(
        &router,
        &update_body(
            &arn,
            "create-idempotent-infra",
            json!({"description": "an update carrying the create's token"}),
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the create's token on an update\": expected HTTP 200, got {status} with {body}"
    );
    let (_, read) = get(&router, &arn).await;
    let config = configuration("the create's token on an update", &read);
    assert_eq!(
        config.get("description"),
        Some(&json!("an update carrying the create's token")),
        "case \"the create's token on an update\": expected the update applied, got {config}"
    );
}
