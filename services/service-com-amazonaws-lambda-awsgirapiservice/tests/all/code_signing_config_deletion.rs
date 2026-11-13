// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteCodeSigningConfig`: "Deletes the code signing
//! configuration. You can delete the code signing configuration only if no function is
//! using it."
//!
//! The rule under test: a delete removes exactly the ONE stored configuration its ARN
//! names — leaving every other configuration where it was — but only while no function is
//! associated with it; a configuration some function still names is refused the modelled
//! 409 and stays whole, and an ARN naming no configuration this account holds is the
//! modelled 404, which a repeat of a delete that succeeded is too.
//!
//! Every row drives real HTTP requests through the router and reads the configuration back
//! through `GetCodeSigningConfig`, so a delete that answered but removed nothing fails
//! here, and so does one that removed a configuration it was not asked about. The rows
//! differ along what the rule generalises over: how many functions name the configuration
//! (none, one, several), how each association was made (stated on `CreateFunction` or set
//! by `PutFunctionCodeSigningConfig`), which of several stored configurations is deleted,
//! and how the association is released before the delete is retried (the function is
//! deleted, or it is re-pointed at another configuration).

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateCodeSigningConfig` is bound
/// (`smithy_query(query="operation", operation="CreateCodeSigningConfig")`:
/// `POST /2020-04-22/code-signing-configs`), and the prefix
/// `GetCodeSigningConfig` (`GET .../{CodeSigningConfigArn}`) and
/// `DeleteCodeSigningConfig` (`DELETE .../{CodeSigningConfigArn}`) are bound under.
const CODE_SIGNING_CONFIGS_URI: &str = "/2020-04-22/code-signing-configs";

/// Where `CreateFunction` and `DeleteFunction` are bound
/// (`smithy_query(query="operation", operation="CreateFunction,DeleteFunction")`:
/// `POST /2015-03-31/functions` and `DELETE /2015-03-31/functions/{FunctionName}`).
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

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

/// The status `DeleteCodeSigningConfig` answers a delete it performed with, from
/// `smithy_query(query="operation", operation="DeleteCodeSigningConfig")`
/// (`http.code: 204`).
const DELETED: u16 = 204;

/// Lambda's sentence for a code signing configuration this account holds none of
/// (`state::code_signing_config_gone_message`), whose one variable part is the ARN the
/// request named: a change to that wording fails this suite.
fn not_found_message(arn: &str) -> String {
    format!("The Lambda code signing configuration {arn} can not be found.")
}

/// Lambda's sentence for a configuration some function still uses
/// (`state::CODE_SIGNING_CONFIG_IN_USE_MESSAGE`), as the recorded response to such a
/// delete carries it. It is the WHOLE message: it names neither the configuration nor a
/// function, so every row expects the same string however many functions hold it.
const IN_USE_MESSAGE: &str = "Lambda cannot delete the code signing configuration at this \
     time — another operation is in progress. Try again later.";

/// A well-formed `CodeSigningConfigArn` for a configuration this account does not hold:
/// `csc-` and 17 lowercase alphanumerics, as the member's own pattern
/// (`code-signing-config:csc-[a-z0-9]{17}`) spells it, from
/// `smithy_query(query="shape", shape="CodeSigningConfigArn")`.
const ABSENT_ARN: &str =
    "arn:aws:lambda:us-east-1:000000000000:code-signing-config:csc-0123456789abcdef0";

/// A second well-formed ARN naming no configuration, differing from [`ABSENT_ARN`] only
/// in its id, so a refusal cannot be keyed to one particular string.
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

/// How many configurations each row creates before deleting one of them.
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

    /// The `message` the refusal carries.
    fn message(&self) -> String {
        let body = self.json();
        body.get("message")
            .or_else(|| body.get("Message"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_default()
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
async fn create_function(router: &MultiServiceRouter, name: &str, config_arn: Option<&str>) {
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
    let answer = call(router, "POST", FUNCTIONS_URI, &body).await;
    assert_eq!(
        answer.status,
        201,
        "CreateFunction {name:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
}

/// `PutFunctionCodeSigningConfig` on `function`, which must succeed.
async fn put_association(router: &MultiServiceRouter, function: &str, config_arn: &str) {
    let answer = call(
        router,
        "PUT",
        &format!("/2020-06-30/functions/{function}/code-signing-config"),
        &json!({ "CodeSigningConfigArn": config_arn }),
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "PutFunctionCodeSigningConfig on {function:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
}

/// `DeleteFunction` on `function`, which must succeed.
async fn delete_function(router: &MultiServiceRouter, function: &str) {
    let answer = call(
        router,
        "DELETE",
        &format!("{FUNCTIONS_URI}/{function}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        answer.status,
        204,
        "DeleteFunction {function:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
}

/// Move every stored function's activation instant into the past, so the next call
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

/// How a row's function comes to name a configuration.
#[derive(Debug, Clone, Copy)]
enum Associate {
    /// Stated on `CreateFunction`'s own `CodeSigningConfigArn` member.
    AtCreate,
    /// Set afterwards by `PutFunctionCodeSigningConfig`.
    ByPut,
}

/// How a row releases a function's association, so the delete can be retried.
#[derive(Debug, Clone, Copy)]
enum Release {
    /// The function itself goes.
    DeleteFunction,
    /// The function is re-pointed at another configuration.
    RepointTo(usize),
}

/// One function of a row: its name, the configuration it names (an index into the row's
/// configurations), how the association was made and how it is released.
struct User {
    function: &'static str,
    config: usize,
    associate: Associate,
    release: Release,
}

/// One row of the deletion table.
struct Case {
    name: &'static str,
    /// Which of the row's configurations the delete addresses.
    target: usize,
    /// The functions created for the row.
    users: &'static [User],
}

/// The rows. Each creates [`CONFIGS_PER_CASE`] configurations, so a row deleting the
/// second or third cannot pass by removing "the only one there is".
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "no function exists at all",
            target: 0,
            users: &[],
        },
        Case {
            name: "a function names another configuration",
            target: 1,
            users: &[User {
                function: "csc-del-other",
                config: 0,
                associate: Associate::AtCreate,
                release: Release::DeleteFunction,
            }],
        },
        Case {
            name: "one function, associated at create, released by deleting it",
            target: 0,
            users: &[User {
                function: "csc-del-at-create",
                config: 0,
                associate: Associate::AtCreate,
                release: Release::DeleteFunction,
            }],
        },
        Case {
            name: "one function, associated by a put, released by re-pointing it",
            target: 2,
            users: &[User {
                function: "csc-del-by-put",
                config: 2,
                associate: Associate::ByPut,
                release: Release::RepointTo(0),
            }],
        },
        Case {
            name: "two functions name the configuration, released each way",
            target: 1,
            users: &[
                User {
                    function: "csc-del-pair-a",
                    config: 1,
                    associate: Associate::AtCreate,
                    release: Release::DeleteFunction,
                },
                User {
                    function: "csc-del-pair-b",
                    config: 1,
                    associate: Associate::ByPut,
                    release: Release::RepointTo(2),
                },
            ],
        },
        Case {
            name: "three functions, one of them naming another configuration",
            target: 2,
            users: &[
                User {
                    function: "csc-del-trio-a",
                    config: 2,
                    associate: Associate::ByPut,
                    release: Release::RepointTo(1),
                },
                User {
                    function: "csc-del-trio-b",
                    config: 0,
                    associate: Associate::AtCreate,
                    release: Release::DeleteFunction,
                },
                User {
                    function: "csc-del-trio-c",
                    config: 2,
                    associate: Associate::AtCreate,
                    release: Release::DeleteFunction,
                },
            ],
        },
    ]
}

#[tokio::test]
async fn a_code_signing_config_is_deleted_only_once_no_function_uses_it() {
    for case in cases() {
        // One account per row, so each row's configurations are the only ones it holds.
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let label = case.name;

        let mut arns: Vec<String> = Vec::new();
        for index in 0..CONFIGS_PER_CASE {
            arns.push(create_config(&router, index).await);
        }
        let target = arns[case.target].clone();

        for user in case.users {
            match user.associate {
                Associate::AtCreate => {
                    create_function(&router, user.function, Some(&arns[user.config])).await;
                    make_activation_due(&engine);
                }
                Associate::ByPut => {
                    create_function(&router, user.function, None).await;
                    make_activation_due(&engine);
                    put_association(&router, user.function, &arns[user.config]).await;
                }
            }
        }

        // The functions that name the configuration the row deletes are the ones that
        // make it in use; every other function is beside the point.
        let mut holders: Vec<&User> = case
            .users
            .iter()
            .filter(|user| user.config == case.target)
            .collect();

        // While any of them holds it, the delete is refused and removes nothing.
        while !holders.is_empty() {
            let refused = delete_config(&router, &target).await;
            assert_eq!(
                refused.status,
                409,
                "case {label:?}: expected 409 deleting {target:?} while {} function(s) \
                 name it, got {} {}",
                holders.len(),
                refused.status,
                refused.text()
            );
            assert!(
                refused.error_code().contains("ResourceConflictException"),
                "case {label:?}: expected ResourceConflictException, got {:?}",
                refused.error_code()
            );
            assert_eq!(
                refused.message(),
                IN_USE_MESSAGE,
                "case {label:?}: expected {IN_USE_MESSAGE:?}, got {:?}",
                refused.message()
            );

            // A refused delete left the configuration whole.
            let still_there = get_config(&router, &target).await;
            assert_eq!(
                still_there.status,
                200,
                "case {label:?}: a refused delete must leave {target:?} in place, got {} {}",
                still_there.status,
                still_there.text()
            );

            // Release exactly one holder and go round again: the delete stays refused
            // until the last of them has let go.
            let released = holders.remove(0);
            match released.release {
                Release::DeleteFunction => delete_function(&router, released.function).await,
                Release::RepointTo(index) => {
                    put_association(&router, released.function, &arns[index]).await;
                }
            }
        }

        // Nothing names it now, so the delete is served and carries no body.
        let deleted = delete_config(&router, &target).await;
        assert_eq!(
            deleted.status,
            DELETED,
            "case {label:?}: expected {DELETED} deleting {target:?} once no function \
             names it, got {} {}",
            deleted.status,
            deleted.text()
        );
        // The model's `DeleteCodeSigningConfigResponse` has no members, so the answer
        // reports none.
        let reported = deleted.json();
        assert!(
            reported.is_null()
                || reported
                    .as_object()
                    .is_some_and(|members| members.is_empty()),
            "case {label:?}: a served delete reports no members, got {}",
            deleted.text()
        );

        // It is gone from the store and from its reader, and a repeat of the delete says
        // the same thing.
        let stored: Vec<String> = engine
            .state::<ComAmazonawsLambdaAwsgirapiserviceState>()
            .lock()
            .code_signing_configs
            .keys()
            .cloned()
            .collect();
        let expected: Vec<String> = {
            let mut remaining: Vec<String> =
                arns.iter().filter(|arn| **arn != target).cloned().collect();
            // The store keys a configuration by its ARN, so it reports them in ARN order.
            remaining.sort();
            remaining
        };
        assert_eq!(
            stored, expected,
            "case {label:?}: expected the store to hold {expected:?} after deleting \
             {target:?}, got {stored:?}"
        );
        let gone = get_config(&router, &target).await;
        assert_eq!(
            (gone.status, gone.error_code().contains("ResourceNotFoundException")),
            (404, true),
            "case {label:?}: expected 404 ResourceNotFoundException reading the deleted \
             {target:?}, got {} {}",
            gone.status,
            gone.text()
        );
        let again = delete_config(&router, &target).await;
        assert_eq!(
            (again.status, again.message()),
            (404, not_found_message(&target)),
            "case {label:?}: expected 404 {:?} repeating the delete of {target:?}, got {} {}",
            not_found_message(&target),
            again.status,
            again.text()
        );

        // Every other configuration of the row is untouched: the delete removed exactly
        // the one it was asked about.
        for arn in &expected {
            let other = get_config(&router, arn).await;
            assert_eq!(
                other.status,
                200,
                "case {label:?}: deleting {target:?} must leave {arn:?} alone, got {} {}",
                other.status,
                other.text()
            );
        }
    }
}

#[tokio::test]
async fn deleting_a_code_signing_config_arn_naming_none_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // Nothing has been created, so even a well-formed ARN names no configuration.
    for arn in [ABSENT_ARN, OTHER_ABSENT_ARN] {
        let refused = delete_config(&router, arn).await;
        assert_eq!(
            (refused.status, refused.message()),
            (404, not_found_message(arn)),
            "case {arn:?}: expected 404 {:?} on an empty account, got {} {}",
            not_found_message(arn),
            refused.status,
            refused.text()
        );
        assert!(
            refused.error_code().contains("ResourceNotFoundException"),
            "case {arn:?}: expected ResourceNotFoundException, got {:?}",
            refused.error_code()
        );
    }

    // One configuration is held; a DIFFERENT well-formed ARN still names none of it, and
    // the refusal leaves the held one alone.
    let held = create_config(&router, 0).await;
    for arn in [ABSENT_ARN, OTHER_ABSENT_ARN] {
        let refused = delete_config(&router, arn).await;
        assert_eq!(
            refused.status,
            404,
            "case {arn:?}: expected 404 even once another configuration exists, got {} {}",
            refused.status,
            refused.text()
        );
        let survivor = get_config(&router, &held).await;
        assert_eq!(
            survivor.status,
            200,
            "case {arn:?}: a refused delete must leave {held:?} in place, got {} {}",
            survivor.status,
            survivor.text()
        );
    }

    // A value that is no code-signing-config ARN at all fails the member's own pattern,
    // so it is a client refusal rather than a delete of some other configuration.
    for malformed in [
        "not-a-code-signing-config-arn",
        "arn:aws:lambda:us-east-1:000000000000:code-signing-config:csc-TOO-SHORT",
    ] {
        let refused = delete_config(&router, malformed).await;
        assert_eq!(
            refused.status,
            400,
            "case {malformed:?}: expected 400 from DeleteCodeSigningConfig, got {} {}",
            refused.status,
            refused.text()
        );
        let survivor = get_config(&router, &held).await;
        assert_eq!(
            survivor.status,
            200,
            "case {malformed:?}: a refused delete must leave {held:?} in place, got {} {}",
            survivor.status,
            survivor.text()
        );
    }
}
