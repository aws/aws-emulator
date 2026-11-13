// SPDX-License-Identifier: CC0-1.0

//! AppStream's app-block-builder role check, across the two services it spans.
//!
//! RULE: an app block builder RUNS AS the role the request hands AppStream
//! (`CreateAppBlockBuilder$IamRoleArn` is "the Amazon Resource Name (ARN) of the IAM role
//! to apply to the app block builder"), so a role this account's IAM does not hold, or one
//! whose trust policy does not let `appstream.amazonaws.com` assume it, is refused with
//! `InvalidRoleException` (HTTP 400) — and nothing is created, so the read that follows
//! answers `ResourceNotFoundException` (HTTP 400). The role is resolved against the
//! ACCOUNT's IAM records, which is why this suite lives here: the role is created through
//! the real `CreateRole` on the same engine that serves AppStream, so what the check reads
//! is the document IAM persisted and not a copy.
//!
//! The rows differ along the form of the trust policy (which service principal it names,
//! whether it names several, whether the grant is an `Allow`, whether an explicit `Deny`
//! stands beside it, which action it names and which kind of principal it names), along
//! whether the IAM store holds the role at all, and along whether the request names a role
//! in the first place.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The percent-encoder that builds the form body of an awsQuery call and the reader that
// pulls one element's text out of the XML answer are the test crate's own, written once in
// `all/query_wire.rs`: a suite that writes its own drifts from the others.
use crate::query_wire::{element, form_encode};

/// The region every AppStream request below is signed for.
const REGION: &str = "us-east-1";

/// The account every emulated resource belongs to.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The service principal AppStream assumes a caller-supplied role as, as the crate states
/// it.
const SERVICE_PRINCIPAL: &str =
    service_com_amazonaws_appstream_photonadminproxyservice::state::SERVICE_PRINCIPAL;

/// One emulator carrying both services over ONE shared engine, exactly as the production
/// router builds them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_appstream_photonadminproxyservice::EmulatedService::new(engine),
    );
    router
}

/// One awsQuery call to IAM, answered with the HTTP status and the XML body.
async fn iam(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    (status, body)
}

/// One awsJson1_1 call to AppStream, answered with the HTTP status and the JSON body.
async fn appstream(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("appstream2.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("PhotonAdminProxyService.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/appstream/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=abc"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(&body).expect("the test body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code of a failure body (`__type`, possibly namespace-qualified).
fn error_code(payload: &Value) -> String {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `Message` of a failure body.
fn error_message(payload: &Value) -> String {
    payload
        .get("Message")
        .or_else(|| payload.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// A trust policy naming `principals` in one `Allow` statement.
fn trust_allowing(principals: &[&str]) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": principals },
            "Action": "sts:AssumeRole"
        }]
    })
    .to_string()
}

/// Create a role in IAM carrying `document` and hand back its ARN.
async fn role_with_trust(router: &MultiServiceRouter, name: &str, document: &str) -> String {
    let (status, created) = iam(
        router,
        "CreateRole",
        &[("RoleName", name), ("AssumeRolePolicyDocument", document)],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateRole {name:?}: expected the role to be persisted, got {status}: {created}"
    );
    element(&created, "Arn")
}

/// A `CreateAppBlockBuilder` request for `name` that runs as `role_arn`, if any.
fn create_builder_body(name: &str, role_arn: Option<&str>) -> Value {
    let mut body = json!({
        "Name": name,
        "Platform": "WINDOWS_SERVER_2019",
        "InstanceType": "stream.standard.small",
        "VpcConfig": { "SubnetIds": ["subnet-00000000", "subnet-11111111"] },
    });
    if let Some(role_arn) = role_arn {
        body["IamRoleArn"] = json!(role_arn);
    }
    body
}

/// AWS's sentence for a role the service cannot assume, as the crate states it.
fn invalid_role_sentence(role_arn: &str) -> String {
    service_com_amazonaws_appstream_photonadminproxyservice::state::invalid_role_message(role_arn)
}

/// AWS's sentence for an app block builder this account does not hold, as the crate
/// states it.
fn not_found_sentence(name: &str) -> String {
    service_com_amazonaws_appstream_photonadminproxyservice::state::resource_not_found_message(
        service_com_amazonaws_appstream_photonadminproxyservice::state::APP_BLOCK_BUILDER_KIND,
        name,
    )
}

/// Assert that this account holds no app block builder under `name`, which is what a
/// refused create leaves behind.
async fn assert_no_builder(router: &MultiServiceRouter, case: &str, name: &str) {
    let (status, payload) =
        appstream(router, "DescribeAppBlockBuilders", json!({ "Names": [name] })).await;
    assert_eq!(
        status, 400,
        "case {case:?}: a refused create must leave no builder to describe, got {status} \
         {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "ResourceNotFoundException",
        "case {case:?}: expected the read to report the builder missing, got {payload}"
    );
    assert_eq!(
        error_message(&payload),
        not_found_sentence(name),
        "case {case:?}: expected AWS's sentence for a builder that does not exist, got \
         {payload}"
    );
}

/// One row: a trust policy, and whether a builder that runs as a role carrying it is
/// created.
struct Row {
    case: &'static str,
    document: String,
    accepted: bool,
}

#[tokio::test]
async fn create_app_block_builder_refuses_a_role_whose_trust_policy_withholds_appstream() {
    let router = emulator();

    let rows = [
        Row {
            case: "appstream alone",
            document: trust_allowing(&[SERVICE_PRINCIPAL]),
            accepted: true,
        },
        Row {
            case: "appstream among several services",
            document: trust_allowing(&["ec2.amazonaws.com", SERVICE_PRINCIPAL]),
            accepted: true,
        },
        Row {
            case: "appstream named as a single string rather than a list",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "Service": SERVICE_PRINCIPAL },
                    "Action": "sts:AssumeRole"
                }]
            })
            .to_string(),
            accepted: true,
        },
        Row {
            case: "appstream allowed through an sts wildcard",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "Service": SERVICE_PRINCIPAL },
                    "Action": "sts:*"
                }]
            })
            .to_string(),
            accepted: true,
        },
        Row {
            case: "another service only",
            document: trust_allowing(&["ec2.amazonaws.com"]),
            accepted: false,
        },
        Row {
            case: "appstream denied",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": { "Service": SERVICE_PRINCIPAL },
                    "Action": "sts:AssumeRole"
                }]
            })
            .to_string(),
            accepted: false,
        },
        Row {
            // IAM's deny precedence: an explicit `Deny` wins over any `Allow`.
            case: "appstream both allowed and denied",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Effect": "Allow",
                        "Principal": { "Service": SERVICE_PRINCIPAL },
                        "Action": "sts:AssumeRole"
                    },
                    {
                        "Effect": "Deny",
                        "Principal": { "Service": SERVICE_PRINCIPAL },
                        "Action": "sts:AssumeRole"
                    }
                ]
            })
            .to_string(),
            accepted: false,
        },
        Row {
            // The statement names appstream, but for an action that is not assuming the
            // role, so it grants nothing: a trust policy governs `sts:AssumeRole`.
            case: "appstream allowed an action that is not assuming the role",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "Service": SERVICE_PRINCIPAL },
                    "Action": "s3:GetObject"
                }]
            })
            .to_string(),
            accepted: false,
        },
        Row {
            case: "an account principal rather than a service one",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "AWS": format!("arn:aws:iam::{ACCOUNT}:root") },
                    "Action": "sts:AssumeRole"
                }]
            })
            .to_string(),
            accepted: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let role_arn =
            role_with_trust(&router, &format!("appstream-trust-{index}"), &row.document).await;
        let name = format!("trust-builder-{index}");

        let (status, payload) =
            appstream(&router, "CreateAppBlockBuilder", create_builder_body(&name, Some(&role_arn)))
                .await;
        if row.accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: a role this service may assume must be accepted, got {status} \
                 {payload}"
            );
            assert_eq!(
                payload.pointer("/AppBlockBuilder/IamRoleArn").and_then(Value::as_str),
                Some(role_arn.as_str()),
                "case {case:?}: the created builder must carry the role it runs as, got \
                 {payload}"
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case:?}: a role this service cannot assume must be refused, got {status} \
             {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "InvalidRoleException",
            "case {case:?}: expected the modelled role refusal, got {payload}"
        );
        assert_eq!(
            error_message(&payload),
            invalid_role_sentence(&role_arn),
            "case {case:?}: expected AWS's sentence for an invalid role, got {payload}"
        );
        assert_no_builder(&router, case, &name).await;
    }
}

#[tokio::test]
async fn create_app_block_builder_refuses_a_role_this_account_does_not_hold() {
    let router = emulator();

    // Rows: role ARNs that pass the modelled `Arn` pattern and name no role this
    // account's IAM holds — a name nothing was created under, one under a path, and one
    // that is a prefix of a role that does exist.
    role_with_trust(&router, "held-role", &trust_allowing(&[SERVICE_PRINCIPAL])).await;
    for (index, (case, role_arn)) in [
        (
            "a role nothing was created under",
            format!("arn:aws:iam::{ACCOUNT}:role/does-not-exist"),
        ),
        (
            "a role under a path",
            format!("arn:aws:iam::{ACCOUNT}:role/service-role/does-not-exist"),
        ),
        (
            "a role name that is a prefix of one that exists",
            format!("arn:aws:iam::{ACCOUNT}:role/held-rol"),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("absent-role-builder-{index}");
        let (status, payload) =
            appstream(&router, "CreateAppBlockBuilder", create_builder_body(&name, Some(&role_arn)))
                .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a role this account does not hold must be refused, got {status} \
             {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "InvalidRoleException",
            "case {case:?}: expected the modelled role refusal, got {payload}"
        );
        assert_eq!(
            error_message(&payload),
            invalid_role_sentence(&role_arn),
            "case {case:?}: expected AWS's sentence for an invalid role, got {payload}"
        );
        assert_no_builder(&router, case, &name).await;
    }

    // The positive control: the same request naming NO role at all is created, because
    // there is no role to resolve.
    let (status, payload) =
        appstream(&router, "CreateAppBlockBuilder", create_builder_body("no-role", None)).await;
    assert_eq!(
        status, 200,
        "case \"no role at all\": a request that names no role must be created, got {status} \
         {payload}"
    );
    assert_eq!(
        payload.pointer("/AppBlockBuilder/IamRoleArn"),
        None,
        "case \"no role at all\": the created builder carries no role, got {payload}"
    );
}
