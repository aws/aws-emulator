// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a function's code signing configuration:
//! `PutFunctionCodeSigningConfig` "Update[s] the code signing configuration for the
//! function" and `GetFunctionCodeSigningConfig` "Returns the code signing configuration
//! for the specified function".
//!
//! The rule under test: a function's code signing configuration is ONE function-level
//! association — absent until a call sets one, and from then on the ARN of the
//! configuration the most recent such call on THAT function named, replacing rather than
//! accumulating — the ARN must name a code signing configuration this account holds, and
//! every reader of the function reports exactly the stored association. A call naming no
//! such configuration, no such function, or a version or alias of one, is refused and
//! stores nothing.
//!
//! Every row drives real HTTP requests through the router and reads the association back
//! through `GetFunctionCodeSigningConfig`, so an association that was answered but never
//! stored fails here. The rows differ along what the rule generalises over: which of
//! several stored configurations is named, how the association came to be (stated on
//! `CreateFunction`, set by `PutFunctionCodeSigningConfig`, or never set), how many calls
//! have been made (none, one, several — including a repeat of the configuration already
//! held and a replacement by another), and the documented `FunctionName` form the requests
//! address the function by.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound
/// (`smithy_query(query="operation", operation="CreateFunction")`:
/// `POST /2015-03-31/functions`).
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// Where `CreateCodeSigningConfig` is bound
/// (`smithy_query(query="operation", operation="CreateCodeSigningConfig")`:
/// `POST /2020-04-22/code-signing-configs`).
const CODE_SIGNING_CONFIGS_URI: &str = "/2020-04-22/code-signing-configs";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in
/// `us-east-1`, which is what routes a request here and what region the ARNs carry.
/// Nothing in it is verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the scope above names.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// A well-formed `CodeSigningConfigArn` for a configuration this account does not hold:
/// `csc-` and 17 lowercase alphanumerics, as the member's own pattern
/// (`code-signing-config:csc-[a-z0-9]{17}`) spells it, from
/// `smithy_query(query="shape", shape="CodeSigningConfigArn")`.
const ABSENT_ARN: &str =
    "arn:aws:lambda:us-east-1:000000000000:code-signing-config:csc-0123456789abcdef0";

/// A second well-formed ARN naming no configuration, differing from [`ABSENT_ARN`] only in
/// its id, so a refusal cannot be keyed to one particular string.
const OTHER_ABSENT_ARN: &str =
    "arn:aws:lambda:us-east-1:000000000000:code-signing-config:csc-zzzz9999zzzz9999z";

/// The `CodeSigningPolicy` the configurations below carry, from
/// `smithy_query(query="shape", shape="CodeSigningPolicy")` (`Warn | Enforce`): the value
/// that ADMITS a deployment whose signature checks fail. These rows deploy an unsigned
/// package under the configuration they name, which the other value blocks outright (that
/// enforcement is its own subject, in `code_signing_enforcement`), and the configurations
/// still differ from one another in their allowed publishers and description, so no two of
/// them are the same record read twice.
const ADMITTING_POLICY: &str = "Warn";

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    /// The body as JSON, or `Value::Null` for an empty body.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// A string member of the body, when it carries one.
    fn string(&self, member: &str) -> Option<String> {
        self.json()
            .get(member)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The wire error code (`__type`, or the `x-amzn-errortype` header).
    fn error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                self.headers
                    .get("x-amzn-errortype")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| String::from_utf8_lossy(&self.body).to_string())
    }

    /// The body text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One service mounted on its own engine, reached the way the production router reaches
/// it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one JSON request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// Where both operations under test are bound
/// (`smithy_query(query="operation", operation="PutFunctionCodeSigningConfig")`:
/// `PUT|GET /2020-06-30/functions/{FunctionName}/code-signing-config`).
fn association_uri(reference: &str) -> String {
    format!("/2020-06-30/functions/{reference}/code-signing-config")
}

/// `PutFunctionCodeSigningConfig`.
async fn put_association(
    router: &MultiServiceRouter,
    reference: &str,
    config_arn: &str,
) -> Answer {
    call(
        router,
        "PUT",
        &association_uri(reference),
        &json!({ "CodeSigningConfigArn": config_arn }),
    )
    .await
}

/// `GetFunctionCodeSigningConfig`.
async fn get_association(router: &MultiServiceRouter, reference: &str) -> Answer {
    call(router, "GET", &association_uri(reference), &Value::Null).await
}

/// A signing profile version ARN satisfying the model's `Arn` pattern, in the form AWS
/// Signer vends one.
fn signing_profile(index: usize) -> String {
    format!("arn:aws:signer:{REGION}:{ACCOUNT}:/signing-profiles/Profile{index}/{index:08x}")
}

/// `CreateCodeSigningConfig`, answering the ARN of the configuration it persisted.
async fn create_config(router: &MultiServiceRouter, index: usize) -> String {
    let answer = call(
        router,
        "POST",
        CODE_SIGNING_CONFIGS_URI,
        &json!({
            "AllowedPublishers": { "SigningProfileVersionArns": [signing_profile(index)] },
            "CodeSigningPolicies": {
                "UntrustedArtifactOnDeployment": ADMITTING_POLICY,
            },
            "Description": format!("configuration {index}"),
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "CreateCodeSigningConfig {index} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
    answer.json()["CodeSigningConfig"]["CodeSigningConfigArn"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "CreateCodeSigningConfig {index} must answer an ARN, got {}",
                answer.text()
            )
        })
        .to_string()
}

/// `CreateFunction`, optionally stating the code signing configuration to deploy under.
async fn create_function(
    router: &MultiServiceRouter,
    name: &str,
    config_arn: Option<&str>,
) -> Answer {
    let package: Vec<u8> = (0..64u16)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect();
    let mut body = json!({
        "FunctionName": name,
        "Role": ROLE,
        "Runtime": "python3.12",
        "Handler": "index.handler",
        "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package) },
    });
    if let Some(arn) = config_arn {
        body["CodeSigningConfigArn"] = json!(arn);
    }
    call(router, "POST", FUNCTIONS_URI, &body).await
}

/// `CreateFunction` that must succeed.
async fn create_active_function(
    engine: &Arc<engine::InMemoryEngine>,
    router: &MultiServiceRouter,
    name: &str,
    config_arn: Option<&str>,
) {
    let answer = create_function(router, name, config_arn).await;
    assert_eq!(
        answer.status,
        201,
        "CreateFunction {name:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
    make_activation_due(engine);
}

/// Move every stored function's activation instant into the past, so the next read
/// materialises the `Pending` -> `Active` transition the clock owes. This is what waiting
/// would do, without waiting.
fn make_activation_due(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        function.active_due = past;
    }
}

/// The association `GetFunctionCodeSigningConfig` reports: the ARN, or `None` when the
/// body carries no `CodeSigningConfigArn` member because the function is associated with
/// no configuration.
async fn reported(case: &str, router: &MultiServiceRouter, reference: &str) -> Option<String> {
    let answer = get_association(router, reference).await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: expected 200 from GetFunctionCodeSigningConfig on {reference:?}, \
         got {} {}",
        answer.status,
        answer.text()
    );
    let arn = answer.string("CodeSigningConfigArn");
    assert!(
        !arn.as_deref().is_some_and(str::is_empty),
        "case {case:?}: GetFunctionCodeSigningConfig must omit CodeSigningConfigArn for a \
         function holding none rather than report a value the member's own pattern does \
         not admit, got {}",
        answer.text()
    );
    arn
}

/// One row of the association table: the function, how its association is set, and the
/// configuration it must then report.
struct Case {
    name: &'static str,
    /// The function name.
    function: &'static str,
    /// The `FunctionName` form the association calls address it by; `None` is the bare
    /// name.
    reference: Option<String>,
    /// The configuration `CreateFunction` states, as an index into the configurations
    /// created for the row.
    created_with: Option<usize>,
    /// The configurations `PutFunctionCodeSigningConfig` associates, in order.
    puts: &'static [usize],
    /// The configuration the function must then report; `None` means none at all.
    expected: Option<usize>,
}

/// The rows. Three configurations are created for each, so a row naming the second or
/// third cannot pass by reporting "the only one there is".
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "a function nobody associated a configuration with reports none",
            function: "signing-unset",
            reference: None,
            created_with: None,
            puts: &[],
            expected: None,
        },
        Case {
            name: "one configuration associated, addressed by the bare name",
            function: "signing-one",
            reference: None,
            created_with: None,
            puts: &[0],
            expected: Some(0),
        },
        Case {
            name: "the third configuration associated, addressed by the function: form",
            function: "signing-third",
            reference: Some("function:signing-third".to_string()),
            created_with: None,
            puts: &[2],
            expected: Some(2),
        },
        Case {
            name: "a second association replaces the first, addressed by the partial ARN",
            function: "signing-replaced",
            reference: Some(format!("{ACCOUNT}:function:signing-replaced")),
            created_with: None,
            puts: &[0, 1],
            expected: Some(1),
        },
        Case {
            name: "the same configuration twice, addressed by the function ARN",
            function: "signing-repeat",
            reference: Some(format!(
                "arn:aws:lambda:{REGION}:{ACCOUNT}:function:signing-repeat"
            )),
            created_with: None,
            puts: &[1, 1],
            expected: Some(1),
        },
        Case {
            name: "several associations end on the last one",
            function: "signing-sequence",
            reference: None,
            created_with: None,
            puts: &[0, 2, 1, 2],
            expected: Some(2),
        },
        Case {
            name: "the configuration CreateFunction stated, never put",
            function: "signing-at-create",
            reference: None,
            created_with: Some(1),
            puts: &[],
            expected: Some(1),
        },
        Case {
            name: "a put replaces the configuration CreateFunction stated",
            function: "signing-create-then-put",
            reference: Some("signing-create-then-put:$LATEST".to_string()),
            created_with: Some(0),
            puts: &[2],
            expected: Some(2),
        },
    ]
}

#[tokio::test]
async fn a_function_reports_the_code_signing_configuration_it_holds() {
    for case in cases() {
        // One account per row, so each row's configurations are the only ones it holds.
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let label = case.name;

        let mut config_arns: Vec<String> = Vec::new();
        for index in 0..3 {
            config_arns.push(create_config(&router, index).await);
        }
        create_active_function(
            &engine,
            &router,
            case.function,
            case.created_with.map(|index| config_arns[index].as_str()),
        )
        .await;
        let reference = case
            .reference
            .clone()
            .unwrap_or_else(|| case.function.to_string());

        // What the function starts with: the configuration `CreateFunction` stated, or
        // none at all.
        let initial = reported(label, &router, &reference).await;
        let expected_initial = case.created_with.map(|index| config_arns[index].clone());
        assert_eq!(
            initial, expected_initial,
            "case {label:?}: expected the association {expected_initial:?} straight after \
             creation, got {initial:?}"
        );

        for index in case.puts {
            let arn = &config_arns[*index];
            let put = put_association(&router, &reference, arn).await;
            assert_eq!(
                put.status,
                200,
                "case {label:?}: expected 200 associating {arn:?}, got {} {}",
                put.status,
                put.text()
            );
            assert_eq!(
                put.string("CodeSigningConfigArn"),
                Some(arn.clone()),
                "case {label:?}: PutFunctionCodeSigningConfig must answer the ARN it \
                 stored ({arn:?}), got {}",
                put.text()
            );
            assert_eq!(
                put.string("FunctionName"),
                Some(case.function.to_string()),
                "case {label:?}: PutFunctionCodeSigningConfig must answer the stored \
                 function's name ({:?}), got {}",
                case.function,
                put.text()
            );

            // Read it back after every call: an association that was answered but never
            // persisted fails here.
            let fetched = reported(label, &router, &reference).await;
            assert_eq!(
                fetched,
                Some(arn.clone()),
                "case {label:?}: after associating {arn:?} the function must report it, \
                 got {fetched:?}"
            );
        }

        // The final association, read by the bare name whichever form the calls used, and
        // read twice: a read must not change what the next caller sees.
        let expected = case.expected.map(|index| config_arns[index].clone());
        let first = reported(label, &router, case.function).await;
        assert_eq!(
            first, expected,
            "case {label:?}: expected {expected:?}, got {first:?}"
        );
        let second = reported(label, &router, case.function).await;
        assert_eq!(
            second, expected,
            "case {label:?}: a second read must report {expected:?} unchanged, got \
             {second:?}"
        );

        // The association is the function's own: a second function on the same account
        // reports none of it.
        create_active_function(&engine, &router, "signing-neighbour", None).await;
        let neighbour = reported(label, &router, "signing-neighbour").await;
        assert_eq!(
            neighbour, None,
            "case {label:?}: a function nobody associated a configuration with must \
             report none, got {neighbour:?}"
        );
    }
}

#[tokio::test]
async fn an_arn_naming_no_code_signing_configuration_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // One configuration exists and is associated, so the refusals below are about the ARN
    // naming no configuration rather than about the account holding none.
    let held = create_config(&router, 0).await;
    create_active_function(&engine, &router, "signing-subject", None).await;
    let put = put_association(&router, "signing-subject", &held).await;
    assert_eq!(
        put.status,
        200,
        "the association under test must succeed, got {} {}",
        put.status,
        put.text()
    );

    for arn in [ABSENT_ARN, OTHER_ABSENT_ARN] {
        let refused = put_association(&router, "signing-subject", arn).await;
        assert_eq!(
            refused.status,
            404,
            "case {arn:?}: expected 404 associating a configuration this account does not \
             hold, got {} {}",
            refused.status,
            refused.text()
        );
        assert!(
            refused
                .error_code()
                .contains("CodeSigningConfigNotFoundException"),
            "case {arn:?}: expected CodeSigningConfigNotFoundException, got {:?}",
            refused.error_code()
        );
        assert!(
            refused.text().contains(arn),
            "case {arn:?}: the refusal must name the ARN the request carried, got {}",
            refused.text()
        );

        // A refused call stores nothing: the function still reports the association it
        // did hold.
        let stored = reported(arn, &router, "signing-subject").await;
        assert_eq!(
            stored,
            Some(held.clone()),
            "case {arn:?}: a refused association must leave the stored one alone, got \
             {stored:?}"
        );

        // The same rule on the other call that sets the association: `CreateFunction`
        // refuses the configuration it cannot find, and creates no function.
        let created = create_function(&router, "signing-at-create-absent", Some(arn)).await;
        assert_eq!(
            created.status,
            404,
            "case {arn:?}: expected 404 from CreateFunction naming a configuration this \
             account does not hold, got {} {}",
            created.status,
            created.text()
        );
        assert!(
            created
                .error_code()
                .contains("CodeSigningConfigNotFoundException"),
            "case {arn:?}: expected CodeSigningConfigNotFoundException from \
             CreateFunction, got {:?}",
            created.error_code()
        );
        let absent = get_association(&router, "signing-at-create-absent").await;
        assert_eq!(
            absent.status,
            404,
            "case {arn:?}: a refused CreateFunction must create no function, got {} {}",
            absent.status,
            absent.text()
        );
    }
}

#[tokio::test]
async fn a_name_naming_no_function_or_a_version_of_one_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let held = create_config(&router, 0).await;
    create_active_function(&engine, &router, "signing-present", None).await;
    let put = put_association(&router, "signing-present", &held).await;
    assert_eq!(
        put.status,
        200,
        "the association under test must succeed, got {} {}",
        put.status,
        put.text()
    );

    // Publishing a version gives the function a qualifier that does exist, so the refusal
    // below is about the association being function-level and not about a missing version.
    let published = call(
        &router,
        "POST",
        "/2015-03-31/functions/signing-present/versions",
        &json!({}),
    )
    .await;
    assert_eq!(
        published.status,
        201,
        "PublishVersion must succeed, got {} {}",
        published.status,
        published.text()
    );

    // Every `FunctionName` naming no function, in each documented form.
    let missing = [
        "signing-absent".to_string(),
        format!("{ACCOUNT}:function:signing-absent"),
        format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:signing-absent"),
    ];
    for reference in &missing {
        for answer in [
            get_association(&router, reference).await,
            put_association(&router, reference, &held).await,
        ] {
            assert_eq!(
                answer.status,
                404,
                "case {reference:?}: expected 404 for a function that does not exist, got \
                 {} {}",
                answer.status,
                answer.text()
            );
            assert!(
                answer.error_code().contains("ResourceNotFoundException"),
                "case {reference:?}: expected ResourceNotFoundException, got {:?}",
                answer.error_code()
            );
        }
    }

    // A qualified name: the configuration applies to the function as a whole, which is
    // what a code package is deployed to, so a version or an alias is not a resource it
    // can be read from or set on — including the version just published.
    let qualified = [
        "signing-present:1".to_string(),
        "signing-present:live".to_string(),
        format!("{ACCOUNT}:function:signing-present:1"),
        format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:signing-present:1"),
    ];
    for reference in &qualified {
        for answer in [
            get_association(&router, reference).await,
            put_association(&router, reference, &held).await,
        ] {
            assert_eq!(
                answer.status,
                400,
                "case {reference:?}: expected 400 for a qualified name, got {} {}",
                answer.status,
                answer.text()
            );
            assert!(
                answer
                    .error_code()
                    .contains("InvalidParameterValueException"),
                "case {reference:?}: expected InvalidParameterValueException, got {:?}",
                answer.error_code()
            );
        }
    }

    // None of the refusals changed the association the function does hold.
    let stored = reported("signing-present", &router, "signing-present").await;
    assert_eq!(
        stored,
        Some(held),
        "a refused call must leave the stored association alone, got {stored:?}"
    );
}

#[tokio::test]
async fn an_association_cannot_be_set_while_the_function_is_still_being_created() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let held = create_config(&router, 0).await;
    // Deliberately NOT brought out of `Pending`: "When the code is Creating, you can't
    // invoke or modify the function."
    let created = create_function(&router, "signing-pending", None).await;
    assert_eq!(
        created.status,
        201,
        "CreateFunction must succeed, got {} {}",
        created.status,
        created.text()
    );

    let refused = put_association(&router, "signing-pending", &held).await;
    assert_eq!(
        refused.status,
        409,
        "expected 409 while the function is still being created, got {} {}",
        refused.status,
        refused.text()
    );
    assert!(
        refused.error_code().contains("ResourceConflictException"),
        "expected ResourceConflictException, got {:?}",
        refused.error_code()
    );
    let during = reported("while provisioning", &router, "signing-pending").await;
    assert_eq!(
        during, None,
        "a refused association must store nothing, got {during:?}"
    );

    // Once the clock has made the activation due, the same call is served.
    make_activation_due(&engine);
    let accepted = put_association(&router, "signing-pending", &held).await;
    assert_eq!(
        accepted.status,
        200,
        "expected 200 once the function is Active, got {} {}",
        accepted.status,
        accepted.text()
    );
    let after = reported("once active", &router, "signing-pending").await;
    assert_eq!(
        after,
        Some(held),
        "the accepted association must be stored, got {after:?}"
    );
}
