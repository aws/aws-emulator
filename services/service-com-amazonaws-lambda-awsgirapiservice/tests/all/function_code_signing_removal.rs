// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteFunctionCodeSigningConfig`: "Removes the code signing
//! configuration from the function."
//!
//! The rule under test: a function's code signing configuration is ONE function-level
//! association, and this call removes exactly that one value from the function its
//! `FunctionName` addresses — so the function then reports no configuration at all, the
//! configuration it named is no longer in use by that function (and can be deleted once
//! nothing else names it), no other function's association is touched, the configuration
//! record itself survives, and a function that holds none is already in the state the call
//! asks for, so the removal is answered the same way however often it is repeated. A
//! `FunctionName` naming no function is the modelled 404, one naming a version or alias of
//! a function is the modelled 400, and a function still being created is the modelled 409
//! — each of which removes nothing.
//!
//! Every row drives real HTTP requests through the router and reads the association back
//! through `GetFunctionCodeSigningConfig`, so a removal that was answered but never
//! persisted fails here, and so does one that removed more than it was asked about. The
//! rows differ along what the rule generalises over: whether the function holds an
//! association at all, how it came to hold one (stated on `CreateFunction` or set by
//! `PutFunctionCodeSigningConfig`), which of several stored configurations it holds, how
//! many removals are called (one, two, three), and the documented `FunctionName` form the
//! requests address the function by.

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
/// `POST /2020-04-22/code-signing-configs`), and the prefix `GetCodeSigningConfig`
/// (`GET .../{CodeSigningConfigArn}`) and `DeleteCodeSigningConfig`
/// (`DELETE .../{CodeSigningConfigArn}`) are bound under.
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

/// The status `DeleteFunctionCodeSigningConfig` answers a removal with, from
/// `smithy_query(query="operation", operation="DeleteFunctionCodeSigningConfig")`
/// (`http.code: 204`).
const REMOVED: u16 = 204;

/// The `CodeSigningPolicy` the configurations below carry, from
/// `smithy_query(query="shape", shape="CodeSigningPolicy")` (`Warn | Enforce`): the value
/// that ADMITS a deployment whose signature checks fail. These rows deploy an unsigned
/// package under the configuration they name, which the other value blocks outright (that
/// enforcement is its own subject, in `code_signing_enforcement`), and the configurations
/// still differ from one another in their allowed publishers and description, so no two of
/// them are the same record read twice.
const ADMITTING_POLICY: &str = "Warn";

/// How many configurations each row creates, so a row naming the second or third cannot
/// pass by reporting "the only one there is".
const CONFIGS_PER_CASE: usize = 3;

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

/// Where the three association operations are bound
/// (`smithy_query(query="operation", operation="DeleteFunctionCodeSigningConfig")`:
/// `DELETE|PUT|GET /2020-06-30/functions/{FunctionName}/code-signing-config`).
fn association_uri(reference: &str) -> String {
    format!("/2020-06-30/functions/{reference}/code-signing-config")
}

/// `DeleteFunctionCodeSigningConfig`.
async fn delete_association(router: &MultiServiceRouter, reference: &str) -> Answer {
    call(router, "DELETE", &association_uri(reference), &Value::Null).await
}

/// `PutFunctionCodeSigningConfig`.
async fn put_association(router: &MultiServiceRouter, reference: &str, config_arn: &str) -> Answer {
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

/// `GetCodeSigningConfig` on the configuration `arn` names.
async fn get_config(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{CODE_SIGNING_CONFIGS_URI}/{arn}"),
        &Value::Null,
    )
    .await
}

/// `DeleteCodeSigningConfig` on the configuration `arn` names.
async fn delete_config(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("{CODE_SIGNING_CONFIGS_URI}/{arn}"),
        &Value::Null,
    )
    .await
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

/// `CreateFunction` that must succeed, brought out of `Pending` the way waiting would.
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

/// One row of the removal table: the function, how it came to hold an association, which
/// configuration that is, how the removals address it and how many of them are called.
struct Case {
    name: &'static str,
    /// The function the removals address.
    function: &'static str,
    /// The `FunctionName` form the removals use; `None` is the bare name.
    reference: Option<String>,
    /// The configuration `CreateFunction` states, as an index into the row's
    /// configurations.
    created_with: Option<usize>,
    /// The configuration `PutFunctionCodeSigningConfig` associates before the removal.
    put: Option<usize>,
    /// The configuration the function therefore holds; `None` means none at all.
    held: Option<usize>,
    /// How many times the removal is called.
    removals: usize,
}

/// The rows.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "the configuration a put associated, addressed by the bare name",
            function: "removal-put",
            reference: None,
            created_with: None,
            put: Some(0),
            held: Some(0),
            removals: 1,
        },
        Case {
            name: "the third configuration, addressed by the function: form",
            function: "removal-third",
            reference: Some("function:removal-third".to_string()),
            created_with: None,
            put: Some(2),
            held: Some(2),
            removals: 1,
        },
        Case {
            name: "the configuration CreateFunction stated, addressed by the partial ARN",
            function: "removal-at-create",
            reference: Some(format!("{ACCOUNT}:function:removal-at-create")),
            created_with: Some(1),
            put: None,
            held: Some(1),
            removals: 1,
        },
        Case {
            name: "a put over the configuration CreateFunction stated, by the function ARN",
            function: "removal-replaced",
            reference: Some(format!(
                "arn:aws:lambda:{REGION}:{ACCOUNT}:function:removal-replaced"
            )),
            created_with: Some(0),
            put: Some(2),
            held: Some(2),
            removals: 1,
        },
        Case {
            name: "two removals of the same association, by the $LATEST-qualified name",
            function: "removal-twice",
            reference: Some("removal-twice:$LATEST".to_string()),
            created_with: None,
            put: Some(1),
            held: Some(1),
            removals: 2,
        },
        Case {
            name: "three removals of a configuration stated at creation",
            function: "removal-thrice",
            reference: None,
            created_with: Some(2),
            put: None,
            held: Some(2),
            removals: 3,
        },
        Case {
            name: "a function that holds no association at all",
            function: "removal-unset",
            reference: None,
            created_with: None,
            put: None,
            held: None,
            removals: 1,
        },
        Case {
            name: "two removals on a function that holds no association",
            function: "removal-unset-twice",
            reference: Some("function:removal-unset-twice".to_string()),
            created_with: None,
            put: None,
            held: None,
            removals: 2,
        },
    ]
}

#[tokio::test]
async fn removing_a_functions_code_signing_configuration_leaves_it_holding_none() {
    for case in cases() {
        // One account per row, so each row's configurations are the only ones it holds.
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let label = case.name;

        let mut config_arns: Vec<String> = Vec::new();
        for index in 0..CONFIGS_PER_CASE {
            config_arns.push(create_config(&router, index).await);
        }
        create_active_function(
            &engine,
            &router,
            case.function,
            case.created_with.map(|index| config_arns[index].as_str()),
        )
        .await;
        if let Some(index) = case.put {
            let put = put_association(&router, case.function, &config_arns[index]).await;
            assert_eq!(
                put.status,
                200,
                "case {label:?}: the association under test must be set, got {} {}",
                put.status,
                put.text()
            );
        }

        // A neighbour function on the same account holds a DIFFERENT configuration, so a
        // removal that reached more than the function it addressed fails below.
        let neighbour_index = case.held.map_or(0, |index| (index + 1) % CONFIGS_PER_CASE);
        let neighbour_arn = config_arns[neighbour_index].clone();
        create_active_function(&engine, &router, "removal-neighbour", Some(&neighbour_arn)).await;

        let expected_before = case.held.map(|index| config_arns[index].clone());
        let before = reported(label, &router, case.function).await;
        assert_eq!(
            before, expected_before,
            "case {label:?}: expected the association {expected_before:?} before any \
             removal, got {before:?}"
        );

        // While the function holds it, the configuration is IN USE, so the configuration's
        // own delete is refused: that is the state the removal below releases.
        if let Some(arn) = expected_before.as_deref() {
            let in_use = delete_config(&router, arn).await;
            assert_eq!(
                in_use.status,
                409,
                "case {label:?}: expected 409 deleting {arn:?} while the function holds \
                 it, got {} {}",
                in_use.status,
                in_use.text()
            );
        }

        // The removal itself, repeated as the row asks: every call is answered the same
        // way, carries no body, and leaves the function holding nothing.
        for attempt in 1..=case.removals {
            let reference = case.reference.as_deref().unwrap_or(case.function);
            let removed = delete_association(&router, reference).await;
            assert_eq!(
                removed.status,
                REMOVED,
                "case {label:?}: expected {REMOVED} from removal {attempt}, got {} {}",
                removed.status,
                removed.text()
            );
            assert!(
                removed.body.is_empty(),
                "case {label:?}: removal {attempt} must carry no body, got {}",
                removed.text()
            );
            let after = reported(label, &router, case.function).await;
            assert_eq!(
                after, None,
                "case {label:?}: after removal {attempt} the function must report no \
                 configuration, got {after:?}"
            );
            // "No configuration" is the member's ABSENCE: `CodeSigningConfigArn` names a
            // configuration and its own pattern admits no string standing for none, so
            // the body carries the member only while the function holds one — and carries
            // nothing else, the function being named by the request's own URI.
            let raw = get_association(&router, case.function).await;
            assert_eq!(
                (
                    raw.json().get("CodeSigningConfigArn").cloned(),
                    raw.json().get("FunctionName").cloned()
                ),
                (None, None),
                "case {label:?}: after removal {attempt} the body must carry neither a \
                 CodeSigningConfigArn nor a FunctionName member, got {}",
                raw.text()
            );
        }

        // The configuration record itself survives the removal of the association, and is
        // now deletable because no function names it any more.
        if let Some(arn) = expected_before.as_deref() {
            let still_there = get_config(&router, arn).await;
            assert_eq!(
                still_there.status,
                200,
                "case {label:?}: the configuration {arn:?} must survive the removal, got \
                 {} {}",
                still_there.status,
                still_there.text()
            );
            let now_deletable = delete_config(&router, arn).await;
            assert_eq!(
                now_deletable.status,
                REMOVED,
                "case {label:?}: expected {REMOVED} deleting {arn:?} once no function \
                 holds it, got {} {}",
                now_deletable.status,
                now_deletable.text()
            );
        }

        // The neighbour's association is untouched, and its configuration is still in use.
        let neighbour = reported(label, &router, "removal-neighbour").await;
        assert_eq!(
            neighbour,
            Some(neighbour_arn.clone()),
            "case {label:?}: another function's association must survive the removal, got \
             {neighbour:?}"
        );
        let neighbour_in_use = delete_config(&router, &neighbour_arn).await;
        assert_eq!(
            neighbour_in_use.status,
            409,
            "case {label:?}: expected 409 deleting {neighbour_arn:?}, which the neighbour \
             still holds, got {} {}",
            neighbour_in_use.status,
            neighbour_in_use.text()
        );

        // The removal is not permanent: the function takes an association again.
        let reassociated = config_arns[neighbour_index].clone();
        let put_again = put_association(&router, case.function, &reassociated).await;
        assert_eq!(
            put_again.status,
            200,
            "case {label:?}: the function must take an association again after a removal, \
             got {} {}",
            put_again.status,
            put_again.text()
        );
        let again = reported(label, &router, case.function).await;
        assert_eq!(
            again,
            Some(reassociated),
            "case {label:?}: the re-set association must be stored, got {again:?}"
        );
    }
}

#[tokio::test]
async fn a_removal_naming_no_function_or_a_version_of_one_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let held = create_config(&router, 0).await;
    create_active_function(&engine, &router, "removal-present", Some(&held)).await;

    // Publishing a version gives the function a qualifier that does exist, so the refusal
    // below is about the association being function-level and not about a missing version.
    let published = call(
        &router,
        "POST",
        "/2015-03-31/functions/removal-present/versions",
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
        "removal-absent".to_string(),
        format!("{ACCOUNT}:function:removal-absent"),
        format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:removal-absent"),
    ];
    for reference in &missing {
        let answer = delete_association(&router, reference).await;
        assert_eq!(
            answer.status,
            404,
            "case {reference:?}: expected 404 for a function that does not exist, got {} {}",
            answer.status,
            answer.text()
        );
        assert!(
            answer.error_code().contains("ResourceNotFoundException"),
            "case {reference:?}: expected ResourceNotFoundException, got {:?}",
            answer.error_code()
        );
    }

    // A qualified name: the configuration applies to the function as a whole, which is
    // what a code package is deployed to, so a version or an alias is not a resource it
    // can be removed from — including the version just published.
    let qualified = [
        "removal-present:1".to_string(),
        "removal-present:live".to_string(),
        format!("{ACCOUNT}:function:removal-present:1"),
        format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:removal-present:1"),
    ];
    for reference in &qualified {
        let answer = delete_association(&router, reference).await;
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

    // None of the refusals removed the association the function does hold.
    let stored = reported("removal-present", &router, "removal-present").await;
    assert_eq!(
        stored,
        Some(held),
        "a refused removal must leave the stored association alone, got {stored:?}"
    );
}

#[tokio::test]
async fn an_association_cannot_be_removed_while_the_function_is_still_being_created() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let held = create_config(&router, 0).await;
    // Deliberately NOT brought out of `Pending`: "When the code is Creating, you can't
    // invoke or modify the function."
    let created = create_function(&router, "removal-pending", Some(&held)).await;
    assert_eq!(
        created.status,
        201,
        "CreateFunction must succeed, got {} {}",
        created.status,
        created.text()
    );

    let refused = delete_association(&router, "removal-pending").await;
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
    let during = reported("while provisioning", &router, "removal-pending").await;
    assert_eq!(
        during,
        Some(held),
        "a refused removal must remove nothing, got {during:?}"
    );

    // Once the clock has made the activation due, the same call is served.
    make_activation_due(&engine);
    let accepted = delete_association(&router, "removal-pending").await;
    assert_eq!(
        accepted.status,
        REMOVED,
        "expected {REMOVED} once the function is Active, got {} {}",
        accepted.status,
        accepted.text()
    );
    let after = reported("once active", &router, "removal-pending").await;
    assert_eq!(
        after, None,
        "the accepted removal must leave the function holding none, got {after:?}"
    );
}
