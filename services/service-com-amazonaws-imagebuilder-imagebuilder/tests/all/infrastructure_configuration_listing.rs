// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the IAM instance profile a configuration names, and for the
//! listing that reports the configurations this service holds.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — with the emulator's own IAM
//! mounted beside imagebuilder on one engine, exactly the way the assembled emulator runs
//! them. An instance profile a client created with `CreateInstanceProfile` is therefore the
//! profile Image Builder resolves.
//!
//! The rules under test:
//!
//! * `instanceProfileName` is "the instance profile to associate with the instance used to
//!   customize your Amazon EC2 AMI" — an IAM resource of the caller's account, whose model
//!   constrains only the SHAPE of the name — so `CreateInfrastructureConfiguration` RESOLVES
//!   it against IAM before writing anything: a name that account holds a profile under is
//!   accepted, and one it holds none under is refused HTTP 400
//!   `InvalidParameterValueException` naming that member, with nothing stored. A refusal is
//!   GROUNDED in IAM's answer: where no IAM answers at all, nothing about the profile is
//!   established and the create is not refused.
//! * a create whose name is already taken in the Region is `ResourceAlreadyExistsException`
//!   stated in the fixed sentence AWS states it in, which names neither the request's name
//!   nor the configuration that holds it; the same `clientToken` carrying different
//!   creation parameters is the modelled mismatch, in its own fixed sentence.
//! * `ListInfrastructureConfigurations` "returns a list of infrastructure configurations" —
//!   the ones this service's state holds for the Region the request was served in, each
//!   rendered as the modelled summary, filtered by `name` when the request asks, paged at
//!   `maxResults` with a `nextToken` only while an entry remains, and refusing a token this
//!   store did not issue.
//!
//! The rows span what the rules generalise over: how the profile is named (a profile held
//! under the default path, the same name in another case, a profile held under another path,
//! a name nothing created, a name IAM holds a role but no profile under), how many
//! configurations the listing holds, the page size it is read at, which Region each
//! configuration lives in, and which filter the request carries.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every imagebuilder call here is signed for unless a case says otherwise.
const REGION: &str = "us-west-2";

/// A second Region, so the listing's own Region can be told from another's.
const OTHER_REGION: &str = "eu-west-1";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// The error this service refuses a parameter value with
/// (`smithy_query(query="shape",shape="InvalidParameterValueException")`: a client error,
/// `@httpError` 400).
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValueException";

/// The instance profile this account holds under the default path.
const HELD_PROFILE: &str = "EC2InstanceProfileForImageBuilder";

/// An instance profile this account holds under a path of its own.
const PATHED_PROFILE: &str = "ImageBuilderPathedProfile";

/// The path [`PATHED_PROFILE`] is created under.
const PROFILE_PATH: &str = "/service-role/";

/// A name this account holds an IAM ROLE under, and no instance profile: Image Builder
/// resolves the PROFILE, so a configuration naming it is refused.
const ROLE_ONLY: &str = "ImageBuilderRoleWithoutProfile";

/// A trust policy admitting Amazon EC2, so each role below is one an instance profile may
/// carry. Written compactly and with no `&`, `=`, `+` or `%`, so it travels in an `awsQuery`
/// form body as written.
const EC2_TRUST_POLICY: &str = "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\
     \"Principal\":{\"Service\":\"ec2.amazonaws.com\"},\"Action\":\"sts:AssumeRole\"}]}";

/// One emulator: imagebuilder and IAM on a single shared engine behind the production
/// router — what `build_router` assembles, narrowed to the two services under test.
fn emulator_with_iam() -> MultiServiceRouter {
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

/// One emulator with imagebuilder ALONE, so nothing answers a lookup of an IAM resource.
fn emulator_without_iam() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_imagebuilder_imagebuilder::EmulatedService::new(engine),
    );
    router
}

/// Status and parsed body of one imagebuilder call, signed for `region`.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
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
    let request = builder.body(payload).expect("a well-formed test request");
    let response = router.dispatch(request).await;
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
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// Status and parsed body of one imagebuilder call in [`REGION`].
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    call_in(router, REGION, method, uri, body).await
}

/// Drive one `awsQuery` IAM action: `POST /` with the parameters in the form body. Every
/// value here is free of `&`, `=`, `+` and `%`, so it needs no escaping.
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

/// Create the IAM entities every case below resolves against: two instance profiles, each
/// built the way AWS builds one, and a role that is never added to any profile.
async fn stage_iam(router: &MultiServiceRouter) {
    for (profile, path) in [(HELD_PROFILE, "/"), (PATHED_PROFILE, PROFILE_PATH)] {
        for (action, parameters) in [
            (
                "CreateRole",
                vec![
                    ("RoleName", profile),
                    ("AssumeRolePolicyDocument", EC2_TRUST_POLICY),
                ],
            ),
            (
                "CreateInstanceProfile",
                vec![("InstanceProfileName", profile), ("Path", path)],
            ),
            (
                "AddRoleToInstanceProfile",
                vec![("InstanceProfileName", profile), ("RoleName", profile)],
            ),
        ] {
            let status = iam(router, action, &parameters).await;
            assert_eq!(
                status, 200,
                "setup: IAM {action} for {profile:?} must answer 200, got {status}"
            );
        }
    }
    let status = iam(
        router,
        "CreateRole",
        &[
            ("RoleName", ROLE_ONLY),
            ("AssumeRolePolicyDocument", EC2_TRUST_POLICY),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: IAM CreateRole for {ROLE_ONLY:?} must answer 200, got {status}"
    );
}

/// A `CreateInfrastructureConfiguration` body naming `profile`.
fn create_body(name: &str, profile: &str, token: &str) -> Value {
    json!({
        "name": name,
        "instanceProfileName": profile,
        "clientToken": token,
    })
}

/// The ARN a configuration named `normalized` carries in `region`.
fn arn_of(region: &str, normalized: &str) -> String {
    format!("arn:aws:imagebuilder:{region}:{ACCOUNT}:infrastructure-configuration/{normalized}")
}

/// The error shape a refusal names, reduced to its shape name.
fn error_code(case: &str, body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an error shape, got {body}"))
        .rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The sentence a refusal states.
fn error_message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `ListInfrastructureConfigurations` in `region` with `body`.
async fn list_in(router: &MultiServiceRouter, region: &str, body: &Value) -> (u16, Value) {
    call_in(
        router,
        region,
        "POST",
        "/ListInfrastructureConfigurations",
        Some(body),
    )
    .await
}

/// The ARNs one listed page carries, in the order the page reports them.
fn listed_arns(case: &str, body: &Value) -> Vec<String> {
    body.get("infrastructureConfigurationSummaryList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a summary list, got {body}"))
        .iter()
        .map(|summary| {
            summary["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected an arn in {summary}"))
                .to_string()
        })
        .collect()
}

/// A filter on one configuration name, as `ListInfrastructureConfigurations` takes it.
fn name_filter(name: &str) -> Value {
    json!([{ "name": "name", "values": [name] }])
}

/// One row of the instance-profile table: how the profile is named, and whether this account
/// holds one under it.
struct ProfileCase {
    case: &'static str,
    /// The configuration each row creates, so one row's outcome cannot answer another's.
    configuration: &'static str,
    /// The `instanceProfileName` the create carries.
    profile: String,
    /// Whether IAM holds an instance profile of that name.
    held: bool,
}

fn profile_cases() -> Vec<ProfileCase> {
    vec![
        ProfileCase {
            case: "an instance profile this account holds",
            configuration: "held-profile-config",
            profile: HELD_PROFILE.to_string(),
            held: true,
        },
        ProfileCase {
            // IAM entity names "are not distinguished by case".
            case: "the same instance profile in another case",
            configuration: "held-profile-other-case-config",
            profile: HELD_PROFILE.to_lowercase(),
            held: true,
        },
        ProfileCase {
            case: "an instance profile this account holds under a path of its own",
            configuration: "pathed-profile-config",
            profile: PATHED_PROFILE.to_string(),
            held: true,
        },
        ProfileCase {
            case: "a name nothing ever created",
            configuration: "absent-profile-config",
            profile: "ImageBuilderProfileThatWasNeverCreated".to_string(),
            held: false,
        },
        ProfileCase {
            case: "a name this account holds a role under but no instance profile",
            configuration: "role-only-profile-config",
            profile: ROLE_ONLY.to_string(),
            held: false,
        },
        ProfileCase {
            // `@pattern ^[\w+=,.@-]+$` admits this, so the model does not refuse it and the
            // lookup is what places it.
            case: "a name made of the punctuation the member's pattern admits",
            configuration: "punctuation-profile-config",
            profile: "profile+=,.@-name".to_string(),
            held: false,
        },
    ]
}

#[tokio::test]
async fn an_instance_profile_the_account_does_not_hold_is_refused_as_that_parameters_value() {
    let router = emulator_with_iam();
    stage_iam(&router).await;

    let expected_refusal = "Invalid parameter value for instanceProfileName: The provided \
                            instance profile does not exist. Please specify a different instance \
                            profile and try again.";

    for (row, case) in profile_cases().into_iter().enumerate() {
        let token = format!("iptok-{row}");
        let (status, answered) = call(
            &router,
            "PUT",
            "/CreateInfrastructureConfiguration",
            Some(&create_body(case.configuration, &case.profile, &token)),
        )
        .await;
        let arn = arn_of(REGION, case.configuration);
        if case.held {
            assert_eq!(
                status, 200,
                "case {:?}: expected HTTP 200 for {:?}, got {status} with {answered}",
                case.case, case.profile
            );
            assert_eq!(
                answered["infrastructureConfigurationArn"].as_str(),
                Some(arn.as_str()),
                "case {:?}: expected the create to mint {arn:?}, got {answered}",
                case.case
            );
            // Round trip: the stored configuration reports the profile it was created with.
            let (status, read) = call(
                &router,
                "GET",
                &format!("/GetInfrastructureConfiguration?infrastructureConfigurationArn={arn}"),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: the created configuration must read back, got {status} with {read}",
                case.case
            );
            let reported = read["infrastructureConfiguration"]["instanceProfileName"]
                .as_str()
                .unwrap_or_default();
            assert_eq!(
                reported, case.profile,
                "case {:?}: expected the stored profile {:?}, got {reported:?}",
                case.case, case.profile
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400 for {:?}, got {status} with {answered}",
            case.case, case.profile
        );
        let code = error_code(case.case, &answered);
        assert_eq!(
            code, INVALID_PARAMETER_VALUE,
            "case {:?}: expected the error code {INVALID_PARAMETER_VALUE:?}, got {code:?}",
            case.case
        );
        let actual = error_message(&answered);
        assert_eq!(
            actual, expected_refusal,
            "case {:?}: expected the message {expected_refusal:?}, got {actual:?}",
            case.case
        );
        // NOTHING was created: the ARN the create would have minted names nothing, and the
        // listing filtered by that name reports no summary.
        let (status, missing) = call(
            &router,
            "GET",
            &format!("/GetInfrastructureConfiguration?infrastructureConfigurationArn={arn}"),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {:?}: a refused create must store nothing, got {status} with {missing}",
            case.case
        );
        let (status, listed) = list_in(
            &router,
            REGION,
            &json!({ "filters": name_filter(case.configuration) }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the listing must answer 200, got {status} with {listed}",
            case.case
        );
        let arns = listed_arns(case.case, &listed);
        assert!(
            arns.is_empty(),
            "case {:?}: a refused create must be listed nowhere, got {arns:?}",
            case.case
        );
    }
}

#[tokio::test]
async fn a_create_refuses_nothing_when_no_iam_answers_the_lookup() {
    // A refusal has to be GROUNDED in IAM's answer: with no IAM mounted, nothing is
    // established about the profile, so the create proceeds instead of refusing a value no
    // answer placed.
    let router = emulator_without_iam();
    let (status, answered) = call(
        &router,
        "PUT",
        "/CreateInfrastructureConfiguration",
        Some(&create_body(
            "unresolved-profile-config",
            "ImageBuilderProfileNothingHolds",
            "iptok-unresolved",
        )),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"no IAM to ask\": expected HTTP 200, got {status} with {answered}"
    );
    let arn = arn_of(REGION, "unresolved-profile-config");
    assert_eq!(
        answered["infrastructureConfigurationArn"].as_str(),
        Some(arn.as_str()),
        "case \"no IAM to ask\": expected the create to mint {arn:?}, got {answered}"
    );
}

#[tokio::test]
async fn a_name_already_taken_and_a_reused_token_are_stated_in_their_own_fixed_sentences() {
    let router = emulator_with_iam();
    stage_iam(&router).await;

    let (status, created) = call(
        &router,
        "PUT",
        "/CreateInfrastructureConfiguration",
        Some(&create_body("taken-name-config", HELD_PROFILE, "dup-0001")),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: the first create must succeed, got {status} with {created}"
    );

    // "Infrastructure configuration names must be unique to your account in each Amazon Web
    // Services Region", compared in the normalized form the ARN carries — so a name that
    // differs only in case, spaces or underscores is the SAME name. The refusal is the fixed
    // sentence AWS states, naming neither the request's name nor the configuration that
    // holds it.
    const ALREADY_EXISTS: &str = "The resource already exists.";
    for (row, (case, name)) in [
        ("the same name again", "taken-name-config"),
        ("the same name in another case", "Taken-Name-Config"),
        ("the same name with underscores for its hyphens", "taken_name_config"),
    ]
    .into_iter()
    .enumerate()
    {
        let (status, refused) = call(
            &router,
            "PUT",
            "/CreateInfrastructureConfiguration",
            Some(&create_body(name, HELD_PROFILE, &format!("dup-010{row}"))),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused}"
        );
        let code = error_code(case, &refused);
        assert_eq!(
            code, "ResourceAlreadyExistsException",
            "case {case:?}: expected ResourceAlreadyExistsException, got {code:?}"
        );
        let actual = error_message(&refused);
        assert_eq!(
            actual, ALREADY_EXISTS,
            "case {case:?}: expected the message {ALREADY_EXISTS:?}, got {actual:?}"
        );
    }

    // The same `clientToken` carrying different creation parameters is the modelled
    // mismatch, in its own fixed sentence — whichever member the retry changed.
    const MISMATCH: &str =
        "The request parameters do not match those in the original idempotent request.";
    let (status, original) = call(
        &router,
        "PUT",
        "/CreateInfrastructureConfiguration",
        Some(&{
            let mut body = create_body("token-config", HELD_PROFILE, "dup-0200");
            body["description"] = json!("original description");
            body
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: the token's original create must succeed, got {status} with {original}"
    );
    for (case, changed) in [
        ("another description under the same token", {
            let mut body = create_body("token-config", HELD_PROFILE, "dup-0200");
            body["description"] = json!("a totally different description");
            body
        }),
        ("another name under the same token", {
            let mut body = create_body("token-config-2", HELD_PROFILE, "dup-0200");
            body["description"] = json!("original description");
            body
        }),
    ] {
        let (status, refused) = call(
            &router,
            "PUT",
            "/CreateInfrastructureConfiguration",
            Some(&changed),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused}"
        );
        let code = error_code(case, &refused);
        assert_eq!(
            code, "IdempotentParameterMismatchException",
            "case {case:?}: expected IdempotentParameterMismatchException, got {code:?}"
        );
        let actual = error_message(&refused);
        assert_eq!(
            actual, MISMATCH,
            "case {case:?}: expected the message {MISMATCH:?}, got {actual:?}"
        );
    }

    // A retry carrying the SAME parameters answers the original response, down to the ARN it
    // named: the refusals above are about parameters that differ, not about the token.
    let (status, retried) = call(
        &router,
        "PUT",
        "/CreateInfrastructureConfiguration",
        Some(&{
            let mut body = create_body("token-config", HELD_PROFILE, "dup-0200");
            body["description"] = json!("original description");
            body
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the same request again\": expected HTTP 200, got {status} with {retried}"
    );
    assert_eq!(
        retried["infrastructureConfigurationArn"], original["infrastructureConfigurationArn"],
        "case \"the same request again\": expected the original response, got {retried}"
    );
}

#[tokio::test]
async fn the_listing_reports_the_stored_configurations_of_its_own_region() {
    let router = emulator_with_iam();
    stage_iam(&router).await;

    // Three configurations in this Region and one in another, each created through the
    // operation that stores it — so what the listing reports is what a create wrote.
    let names = ["listed-alpha", "listed-beta", "listed-gamma"];
    for (row, name) in names.iter().enumerate() {
        let (status, created) = call(
            &router,
            "PUT",
            "/CreateInfrastructureConfiguration",
            Some(&create_body(name, HELD_PROFILE, &format!("ltok-{row}"))),
        )
        .await;
        assert_eq!(
            status, 200,
            "setup: creating {name:?} must succeed, got {status} with {created}"
        );
    }
    let (status, elsewhere) = call_in(
        &router,
        OTHER_REGION,
        "PUT",
        "/CreateInfrastructureConfiguration",
        Some(&create_body("listed-elsewhere", HELD_PROFILE, "ltok-other")),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: creating the other Region's configuration must succeed, got {status} with \
         {elsewhere}"
    );

    let expected: Vec<String> = names.iter().map(|name| arn_of(REGION, name)).collect();

    // The whole listing, in the ARN order the store holds it in, and nothing of the other
    // Region.
    let (status, listed) = list_in(&router, REGION, &json!({})).await;
    assert_eq!(
        status, 200,
        "case \"the whole listing\": expected HTTP 200, got {status} with {listed}"
    );
    let arns = listed_arns("the whole listing", &listed);
    assert_eq!(
        arns, expected,
        "case \"the whole listing\": expected {expected:?}, got {arns:?}"
    );
    assert!(
        listed.get("nextToken").is_none_or(Value::is_null),
        "case \"the whole listing\": a complete page must carry no nextToken, got {listed}"
    );

    // One summary read in full: the members the modelled summary carries, off the record the
    // create wrote.
    let first = &listed["infrastructureConfigurationSummaryList"][0];
    for (member, expected) in [
        ("arn", json!(arn_of(REGION, names[0]))),
        ("name", json!(names[0])),
        ("instanceProfileName", json!(HELD_PROFILE)),
    ] {
        assert_eq!(
            first[member], expected,
            "case \"the first summary\": expected {member} {expected}, got {first}"
        );
    }
    assert!(
        first["dateCreated"].as_str().is_some_and(|at| !at.is_empty()),
        "case \"the first summary\": expected dateCreated to report when it was written, got \
         {first}"
    );
    // A summary reports the same stored record a read does, so a configuration no update
    // has replaced carries no `dateUpdated` here either.
    assert!(
        first.get("dateUpdated").is_none(),
        "case \"the first summary\": expected no dateUpdated before any update, got {first}"
    );

    // The other Region reports its own configuration and none of this one's.
    let (status, other) = list_in(&router, OTHER_REGION, &json!({})).await;
    assert_eq!(
        status, 200,
        "case \"the other Region's listing\": expected HTTP 200, got {status} with {other}"
    );
    let arns = listed_arns("the other Region's listing", &other);
    assert_eq!(
        arns,
        vec![arn_of(OTHER_REGION, "listed-elsewhere")],
        "case \"the other Region's listing\": got {arns:?}"
    );

    // "You can filter on name to streamline results": the filter selects one entry, and a
    // name nothing was stored under selects none.
    for (case, name, expected) in [
        ("a filter on a stored name", names[1], vec![arn_of(REGION, names[1])]),
        ("a filter on a name nothing was stored under", "listed-nothing", Vec::new()),
    ] {
        let (status, filtered) =
            list_in(&router, REGION, &json!({ "filters": name_filter(name) })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {filtered}"
        );
        let arns = listed_arns(case, &filtered);
        assert_eq!(arns, expected, "case {case:?}: expected {expected:?}, got {arns:?}");
    }

    // Every page size the listing can be read at reports the same entries in the same order,
    // and a `nextToken` only while an entry remains.
    for page_size in 1..=4 {
        let case = format!("paging at {page_size} per page");
        let mut collected: Vec<String> = Vec::new();
        let mut next: Option<String> = None;
        loop {
            let mut body = json!({ "maxResults": page_size });
            if let Some(token) = &next {
                body["nextToken"] = json!(token);
            }
            let (status, page) = list_in(&router, REGION, &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {page}"
            );
            let arns = listed_arns(&case, &page);
            assert!(
                arns.len() <= page_size,
                "case {case:?}: a page must carry at most {page_size} entries, got {arns:?}"
            );
            collected.extend(arns);
            next = page
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if next.is_none() {
                break;
            }
        }
        assert_eq!(
            collected, expected,
            "case {case:?}: expected {expected:?} across the pages, got {collected:?}"
        );
    }

    // A filter naming a member this listing does not filter on is a value its `filters`
    // parameter does not admit; one carrying no values at all is an incomplete filter; and a
    // token this store did not issue is not a position in any listing.
    for (case, body, code, message) in [
        (
            "a filter on a member this listing does not filter on",
            json!({ "filters": [{ "name": "platform", "values": ["Linux"] }] }),
            INVALID_PARAMETER_VALUE,
            "The value supplied for parameter 'filters' is not valid. Allowed filters for \
             ListInfrastructureConfigurations are: name."
                .to_string(),
        ),
        (
            "a filter carrying no values",
            json!({ "filters": [{ "name": "name" }] }),
            "InvalidRequestException",
            "Each filter must specify a name and one or more values.".to_string(),
        ),
        (
            "a token this store did not issue",
            json!({ "nextToken": "not-a-token-this-store-issued" }),
            INVALID_PARAMETER_VALUE,
            "The value supplied for parameter 'nextToken' is not valid. The provided token does \
             not meet the required format."
                .to_string(),
        ),
    ] {
        let (status, refused) = list_in(&router, REGION, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused}"
        );
        let actual_code = error_code(case, &refused);
        assert_eq!(
            actual_code, code,
            "case {case:?}: expected the error code {code:?}, got {actual_code:?}"
        );
        let actual = error_message(&refused);
        assert_eq!(
            actual, message,
            "case {case:?}: expected the message {message:?}, got {actual:?}"
        );
    }

    // A token this listing issued is refused for ANOTHER listing: it names a position in the
    // query it was issued for.
    let (status, first_page) = list_in(&router, REGION, &json!({ "maxResults": 1 })).await;
    assert_eq!(
        status, 200,
        "setup: the first page must answer 200, got {status} with {first_page}"
    );
    let token = first_page["nextToken"]
        .as_str()
        .expect("the first of three entries must hand back a nextToken")
        .to_string();
    let (status, refused) = list_in(
        &router,
        REGION,
        &json!({ "maxResults": 1, "nextToken": token, "filters": name_filter(names[0]) }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a token spent on another query\": expected HTTP 400, got {status} with {refused}"
    );
    // The same token is still a position in the listing it WAS issued for.
    let (status, second_page) =
        list_in(&router, REGION, &json!({ "maxResults": 1, "nextToken": token })).await;
    assert_eq!(
        status, 200,
        "case \"the token's own listing\": expected HTTP 200, got {status} with {second_page}"
    );
    let arns = listed_arns("the token's own listing", &second_page);
    assert_eq!(
        arns,
        vec![arn_of(REGION, names[1])],
        "case \"the token's own listing\": got {arns:?}"
    );
}
