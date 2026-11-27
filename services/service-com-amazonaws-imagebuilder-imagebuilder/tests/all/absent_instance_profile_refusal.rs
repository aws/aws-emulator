// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the words a configuration's refusal states when the IAM
//! instance profile it names is one this account holds none of.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, this service's front door and the handlers — with the emulator's own IAM
//! mounted beside imagebuilder on one engine, the way the assembled emulator runs them, so
//! an instance profile a client created with `CreateInstanceProfile` is the profile Image
//! Builder resolves and an absent one is absent to IAM itself.
//!
//! The rule under test: a request whose `instanceProfileName` names no instance profile of
//! this account is refused HTTP 400 `InvalidParameterValueException`
//! (`smithy_query(query="shape",shape="InvalidParameterValueException")`: a client error,
//! `@httpError` 400) stating `The value supplied for parameter '<member>' is not valid. The
//! provided instance profile does not exist. Please specify a different instance profile and
//! try again.`, where `<member>` is the input member the value arrived in, quoted, and the
//! name the caller submitted is withheld — the same sentence from every operation that
//! carries that member, with nothing of the store changed.
//!
//! The rows span what the rule generalises over: which operation carried the value (the
//! create that mints a configuration and the update that replaces one), and the form of the
//! name (a plain name, the same name in another case, a name of the punctuation the member's
//! `@pattern` admits, a name at IAM's own length maximum, and a name this account holds a
//! ROLE but no instance profile under). The accepted control is in the same table, so the
//! refusal is not what every row would answer.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// The error a value this service cannot accept for a parameter is refused with.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValueException";

/// The member the refused value arrives in, as the API addresses it: the input member both
/// `CreateInfrastructureConfiguration` and `UpdateInfrastructureConfiguration` declare
/// (`smithy_query(query="operation",operation="CreateInfrastructureConfiguration,\
/// UpdateInfrastructureConfiguration")`).
const MEMBER: &str = "instanceProfileName";

/// The requirement the refusal states for a name this account holds no profile under.
const REQUIREMENT: &str = "The provided instance profile does not exist. Please specify a \
                           different instance profile and try again.";

/// The instance profile this account holds, so the accepted row and the configuration the
/// update rows replace exist at all.
const HELD_PROFILE: &str = "EC2InstanceProfileForImageBuilder";

/// A name this account holds an IAM ROLE under and no instance profile: Image Builder
/// resolves the PROFILE, so a request naming it is refused.
const ROLE_ONLY: &str = "ImageBuilderRoleWithoutProfile";

/// A trust policy admitting Amazon EC2, so each staged role is one an instance profile may
/// carry. Free of `&`, `=`, `+` and `%`, so it travels in an `awsQuery` form body as written.
const EC2_TRUST_POLICY: &str = "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\
     \"Principal\":{\"Service\":\"ec2.amazonaws.com\"},\"Action\":\"sts:AssumeRole\"}]}";

/// The configuration the update rows replace, created with the profile this account holds.
const UPDATED_CONFIGURATION: &str = "absent-profile-update-target";

/// The sentence the refusal of `member` has to state: the ONE sentence this service refuses
/// a parameter's value in, naming the member quoted, then the requirement that value had to
/// satisfy.
fn expected_message(member: &str) -> String {
    format!("The value supplied for parameter '{member}' is not valid. {REQUIREMENT}")
}

/// One emulator: imagebuilder and IAM on a single shared engine behind the production
/// router — what `build_router` assembles, narrowed to the two services under test.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_imagebuilder_imagebuilder::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// Status, parsed body and the error shape the header names, for one imagebuilder call.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value, String) {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("imagebuilder.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDIMAGEBUILDER/20260101/{REGION}/imagebuilder/\
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
    let request = builder.body(payload).expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(shape_name)
        .unwrap_or_default();
    let raw = response.into_body();
    let parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    (status, parsed, error_type)
}

/// The shape name a discriminator value names, without its namespace or locator.
fn shape_name(raw: &str) -> String {
    raw.rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .split(':')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// One `awsQuery` IAM action: `POST /` with the parameters in the form body. Every value
/// here is free of `&`, `=`, `+` and `%`, so it needs no escaping.
async fn iam(router: &MultiServiceRouter, action: &str, parameters: &[(&str, &str)]) -> u16 {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (key, value) in parameters {
        body.push_str(&format!("&{key}={value}"));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://iam.{REGION}.amazonaws.com/"))
        .header("host", format!("iam.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/iam/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .header("content-type", "application/x-www-form-urlencoded")
        .body(bytes::Bytes::from(body))
        .expect("a well-formed IAM request");
    router.dispatch(request).await.status().as_u16()
}

/// Stage IAM: the one instance profile this account holds, built the way AWS builds one, and
/// a role that is never added to any profile.
async fn stage_iam(router: &MultiServiceRouter) {
    for (action, parameters) in [
        (
            "CreateRole",
            vec![
                ("RoleName", HELD_PROFILE),
                ("AssumeRolePolicyDocument", EC2_TRUST_POLICY),
            ],
        ),
        (
            "CreateInstanceProfile",
            vec![("InstanceProfileName", HELD_PROFILE), ("Path", "/")],
        ),
        (
            "AddRoleToInstanceProfile",
            vec![
                ("InstanceProfileName", HELD_PROFILE),
                ("RoleName", HELD_PROFILE),
            ],
        ),
        (
            "CreateRole",
            vec![
                ("RoleName", ROLE_ONLY),
                ("AssumeRolePolicyDocument", EC2_TRUST_POLICY),
            ],
        ),
    ] {
        let status = iam(router, action, &parameters).await;
        assert_eq!(
            status, 200,
            "setup: IAM {action} must answer 200, got {status}"
        );
    }
}

/// The ARN a configuration named `name` carries in [`REGION`].
fn configuration_arn(name: &str) -> String {
    format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:infrastructure-configuration/{name}")
}

/// Which operation a row carries its `instanceProfileName` in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Carrier {
    /// `PUT /CreateInfrastructureConfiguration`, which mints a configuration.
    Create,
    /// `PUT /UpdateInfrastructureConfiguration`, which replaces a stored one.
    Update,
}

/// One row: the operation the value rode in, and the name it carried.
struct Row {
    case: &'static str,
    carrier: Carrier,
    /// The `instanceProfileName` the request names.
    profile: String,
    /// Whether this account holds an instance profile of that name.
    held: bool,
}

/// A name of `length` characters that nothing ever created, built from the word characters
/// the member's `@pattern ^[\w+=,.@-]+$` admits. 128 is IAM's own maximum for an instance
/// profile name (`iam#instanceProfileNameType`, `@length` 1..128), so a name that long is
/// one IAM still answers about.
fn absent_name_of_length(length: usize) -> String {
    "ImageBuilderProfileNothingHolds"
        .chars()
        .cycle()
        .take(length)
        .collect()
}

/// The rows: every form of a name this account holds no profile under, in each operation
/// that carries the member, plus the accepted control.
fn rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for carrier in [Carrier::Create, Carrier::Update] {
        for (case, profile, held) in [
            (
                "a name nothing ever created",
                "ImageBuilderProfileThatWasNeverCreated".to_string(),
                false,
            ),
            (
                "that name in another case, which IAM does not distinguish either",
                "imagebuilderprofilethatwasnevercreated".to_string(),
                false,
            ),
            (
                "a name made of the punctuation the member's pattern admits",
                "profile+=,.@-name".to_string(),
                false,
            ),
            (
                "a name at IAM's own length maximum",
                absent_name_of_length(128),
                false,
            ),
            (
                "a name this account holds a role but no instance profile under",
                ROLE_ONLY.to_string(),
                false,
            ),
            (
                "the instance profile this account does hold",
                HELD_PROFILE.to_string(),
                true,
            ),
        ] {
            rows.push(Row {
                case,
                carrier,
                profile,
                held,
            });
        }
    }
    rows
}

/// `GetInfrastructureConfiguration` for `arn`.
async fn read(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    let (status, body, _) = call(
        router,
        "GET",
        &format!("/GetInfrastructureConfiguration?infrastructureConfigurationArn={arn}"),
        None,
    )
    .await;
    (status, body)
}

#[tokio::test]
async fn an_instance_profile_the_account_holds_none_of_is_refused_in_aws_s_own_sentence() {
    let router = emulator();
    stage_iam(&router).await;

    // The configuration the update rows replace: created with the profile this account DOES
    // hold, so each refusal below is about the value that request carried.
    let target = configuration_arn(UPDATED_CONFIGURATION);
    let (status, created, _) = call(
        &router,
        "PUT",
        "/CreateInfrastructureConfiguration",
        Some(&json!({
            "name": UPDATED_CONFIGURATION,
            "instanceProfileName": HELD_PROFILE,
            "clientToken": "aiptok-setup",
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: the create must succeed, got {status} with {created}"
    );

    let expected = expected_message(MEMBER);
    for (index, row) in rows().into_iter().enumerate() {
        let token = format!("aiptok-{index}");
        // Each create row mints its own configuration, so no row's outcome answers another's.
        let name = format!("absent-profile-row-{index}");
        let (status, answered, error_type) = match row.carrier {
            Carrier::Create => {
                call(
                    &router,
                    "PUT",
                    "/CreateInfrastructureConfiguration",
                    Some(&json!({
                        "name": name,
                        "instanceProfileName": row.profile,
                        "clientToken": token,
                    })),
                )
                .await
            }
            Carrier::Update => {
                call(
                    &router,
                    "PUT",
                    "/UpdateInfrastructureConfiguration",
                    Some(&json!({
                        "infrastructureConfigurationArn": target,
                        "instanceProfileName": row.profile,
                        "clientToken": token,
                    })),
                )
                .await
            }
        };

        if row.held {
            assert_eq!(
                status, 200,
                "case {:?} on {:?}: a profile this account holds must be accepted, got \
                 {status} with {answered}",
                row.case, row.carrier
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {:?} on {:?}: expected HTTP 400, got {status} with {answered}",
            row.case, row.carrier
        );
        let code = answered
            .get("__type")
            .and_then(Value::as_str)
            .map(shape_name)
            .unwrap_or_default();
        assert_eq!(
            code, INVALID_PARAMETER_VALUE,
            "case {:?} on {:?}: expected $.__type {INVALID_PARAMETER_VALUE:?}, got {code:?}",
            row.case, row.carrier
        );
        assert_eq!(
            error_type, INVALID_PARAMETER_VALUE,
            "case {:?} on {:?}: expected x-amzn-errortype {INVALID_PARAMETER_VALUE:?}, got \
             {error_type:?}",
            row.case, row.carrier
        );
        let message = answered
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            message, expected,
            "case {:?} on {:?}: expected the message {expected:?}, got {message:?}",
            row.case, row.carrier
        );
        // The name the caller submitted is WITHHELD: the requirement is that a profile of
        // that name exists, not the one name that missed it.
        assert!(
            !message.contains(row.profile.as_str()),
            "case {:?} on {:?}: expected the submitted name {:?} to be withheld, got \
             {message:?}",
            row.case,
            row.carrier,
            row.profile
        );

        // Nothing of the store changed: a refused create minted no configuration, and a
        // refused update left the stored one reporting the profile it was created with.
        match row.carrier {
            Carrier::Create => {
                let arn = configuration_arn(&name);
                let (status, missing) = read(&router, &arn).await;
                assert_eq!(
                    status, 404,
                    "case {:?} on {:?}: a refused create must store nothing, got {status} with \
                     {missing}",
                    row.case, row.carrier
                );
            }
            Carrier::Update => {
                let (status, read_back) = read(&router, &target).await;
                assert_eq!(
                    status, 200,
                    "case {:?} on {:?}: the configuration must read back, got {status} with \
                     {read_back}",
                    row.case, row.carrier
                );
                let reported = read_back["infrastructureConfiguration"]["instanceProfileName"]
                    .as_str()
                    .unwrap_or_default();
                assert_eq!(
                    reported, HELD_PROFILE,
                    "case {:?} on {:?}: expected the stored profile {HELD_PROFILE:?}, got \
                     {reported:?}",
                    row.case, row.carrier
                );
            }
        }
    }
}
