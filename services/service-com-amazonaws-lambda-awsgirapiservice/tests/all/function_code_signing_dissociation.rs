// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the function <-> code-signing-config association being torn down
//! on BOTH the sides that report it.
//!
//! The rule under test: the association is one function-level binding, so the two readers
//! of it answer out of that one value — `ListFunctionsByCodeSigningConfig` reports a
//! function's ARN for exactly as long as the function holds the configuration, and
//! `GetFunctionCodeSigningConfig` reports the function itself, so for a function holding
//! no configuration it is a 200 whose body simply carries no `CodeSigningConfigArn`
//! member rather than an error or a value the member's own pattern
//! (`^arn:…:code-signing-config:csc-[a-z0-9]{17}$`, from
//! `smithy_query(query="shape", shape="CodeSigningConfigArn")`) does not admit. After
//! `DeleteFunctionCodeSigningConfig` is answered, both readers report the function as
//! holding none, and nothing else's association moves.
//!
//! Every row drives real HTTP requests through the router and reads both surfaces back
//! across the full `Marker`/`NextMarker` sequence, so a removal answered but not
//! persisted, or persisted on one index only, fails here. The rows differ along what the
//! rule generalises over: how many functions hold the configuration (one, two, three,
//! four), which of them is dissociated (the first, a middle one, the last), how the
//! dissociated function came to hold it (stated on `CreateFunction`, set by
//! `PutFunctionCodeSigningConfig`, or a put over what creation stated), the documented
//! `FunctionName` form the dissociation and the read address it by, and the page size the
//! membership is paginated with (one entry per page, two, and no `MaxItems` at all).

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
/// `POST /2020-04-22/code-signing-configs`), and the prefix
/// `ListFunctionsByCodeSigningConfig` (`GET .../{CodeSigningConfigArn}/functions`) is
/// bound under.
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
/// still differ from one another in their allowed publishers and description, so neither is
/// the other read twice.
const ADMITTING_POLICY: &str = "Warn";

/// A ceiling on how many pages one pagination walk reads, so a `NextMarker` that never
/// terminates fails the test instead of hanging it.
const PAGE_LIMIT: usize = 32;

/// What a dispatched request answered.
struct Answer {
    status: u16,
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

    /// Whether the body carries `member` at all.
    fn has(&self, member: &str) -> bool {
        self.json().get(member).is_some()
    }

    /// A string member of the body, when it carries one.
    fn string(&self, member: &str) -> Option<String> {
        self.json()
            .get(member)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The `FunctionArns` a listing answers with.
    fn function_arns(&self) -> Vec<String> {
        match self.json().get("FunctionArns") {
            Some(Value::Array(entries)) => entries
                .iter()
                .map(|entry| entry.as_str().unwrap_or_default().to_string())
                .collect(),
            other => panic!(
                "expected a FunctionArns array, got {other:?}: {}",
                self.text()
            ),
        }
    }

    /// The `NextMarker` a listing answered, when it carried one.
    fn next_marker(&self) -> Option<String> {
        self.json()
            .get("NextMarker")
            .and_then(Value::as_str)
            .map(str::to_string)
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
        body,
    }
}

/// Where the three association operations are bound
/// (`smithy_query(query="operation", operation="DeleteFunctionCodeSigningConfig")`:
/// `DELETE|PUT|GET /2020-06-30/functions/{FunctionName}/code-signing-config`).
fn association_uri(reference: &str) -> String {
    format!("/2020-06-30/functions/{reference}/code-signing-config")
}

/// `GetFunctionCodeSigningConfig`.
async fn get_association(router: &MultiServiceRouter, reference: &str) -> Answer {
    call(router, "GET", &association_uri(reference), &Value::Null).await
}

/// `PutFunctionCodeSigningConfig`, which must succeed.
async fn associate(case: &str, router: &MultiServiceRouter, reference: &str, config_arn: &str) {
    let answer = call(
        router,
        "PUT",
        &association_uri(reference),
        &json!({ "CodeSigningConfigArn": config_arn }),
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: associating {reference:?} with {config_arn:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
}

/// The association `GetFunctionCodeSigningConfig` reports for the function `reference`
/// addresses: the ARN it holds, or `None` when it holds none.
///
/// The function existing is what the call reports on, so every answer is a 200; and what the
/// body carries is the association as a WHOLE — the configuration's ARN and the function
/// holding it for a function that holds one, and nothing at all for a function that holds
/// none, never an empty or otherwise non-conforming value standing in for "none".
async fn reported(
    case: &str,
    router: &MultiServiceRouter,
    reference: &str,
) -> Option<String> {
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
        "case {case:?}: GetFunctionCodeSigningConfig on {reference:?} must omit \
         CodeSigningConfigArn rather than report a value its own pattern does not admit, \
         got {}",
        answer.text()
    );
    assert_eq!(
        answer.has("CodeSigningConfigArn"),
        arn.is_some(),
        "case {case:?}: a CodeSigningConfigArn that is present must be a string, got {}",
        answer.text()
    );
    assert_eq!(
        answer.has("FunctionName"),
        arn.is_some(),
        "case {case:?}: the body reports the association as a whole — the function holding \
         it exactly when there is a configuration to report — got {}",
        answer.text()
    );
    arn
}

/// `DeleteFunctionCodeSigningConfig`: answered with the modelled 204 and no body.
async fn dissociate(case: &str, router: &MultiServiceRouter, reference: &str) {
    let answer = call(router, "DELETE", &association_uri(reference), &Value::Null).await;
    assert_eq!(
        answer.status,
        REMOVED,
        "case {case:?}: expected {REMOVED} dissociating {reference:?}, got {} {}",
        answer.status,
        answer.text()
    );
    assert!(
        answer.body.is_empty(),
        "case {case:?}: the dissociation must carry no body, got {}",
        answer.text()
    );
}

/// Every function ARN `ListFunctionsByCodeSigningConfig` reports for `config`, read across
/// the FULL `Marker`/`NextMarker` sequence with the row's page size.
async fn members(
    case: &str,
    router: &MultiServiceRouter,
    config: &str,
    page_size: Option<i64>,
) -> Vec<String> {
    let mut collected: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    for page_number in 1..=PAGE_LIMIT {
        let mut query: Vec<String> = Vec::new();
        if let Some(size) = page_size {
            query.push(format!("MaxItems={size}"));
        }
        if let Some(from) = marker.as_deref() {
            query.push(format!("Marker={from}"));
        }
        let suffix = if query.is_empty() {
            String::new()
        } else {
            format!("?{}", query.join("&"))
        };
        let answer = call(
            router,
            "GET",
            &format!("{CODE_SIGNING_CONFIGS_URI}/{config}/functions{suffix}"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: expected 200 from page {page_number} of \
             ListFunctionsByCodeSigningConfig on {config:?}, got {} {}",
            answer.status,
            answer.text()
        );
        let page = answer.function_arns();
        if let Some(size) = page_size {
            assert!(
                i64::try_from(page.len()).unwrap_or(i64::MAX) <= size,
                "case {case:?}: page {page_number} must hold at most {size} entries, got {}",
                answer.text()
            );
        }
        collected.extend(page);
        marker = answer.next_marker();
        if marker.is_none() {
            return collected;
        }
    }
    panic!("case {case:?}: the membership of {config:?} never paginated to an end");
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

/// `CreateFunction`, optionally stating the configuration to deploy under, answering the
/// function ARN it persisted. The function is brought out of `Pending` the way waiting
/// would, so the association calls below are not refused as modifications of a function
/// that is still being created.
async fn create_function(
    engine: &Arc<engine::InMemoryEngine>,
    router: &MultiServiceRouter,
    name: &str,
    config_arn: Option<&str>,
) -> String {
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
    make_activation_due(engine);
    answer
        .string("FunctionArn")
        .unwrap_or_else(|| panic!("CreateFunction {name:?} must answer a FunctionArn"))
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

/// How the dissociated function came to hold the configuration.
#[derive(Debug, Clone, Copy)]
enum Held {
    /// Stated on `CreateFunction`.
    AtCreate,
    /// Set by `PutFunctionCodeSigningConfig`.
    Put,
    /// Stated on `CreateFunction` and then put over, with the OTHER configuration in
    /// between, so the removal cannot pass by restoring a previous value.
    CreateThenPut,
}

/// One row of the dissociation table.
struct Case {
    name: &'static str,
    /// How many functions hold the configuration before the dissociation.
    bound: usize,
    /// Which of them is dissociated.
    detach: usize,
    /// How that one came to hold it.
    held: Held,
    /// The page size the membership is paginated with; `None` sends no `MaxItems`.
    page_size: Option<i64>,
    /// The documented `FunctionName` form the dissociation and the reads use. `None` is
    /// the bare name.
    reference: Option<fn(&str) -> String>,
}

/// The rows.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "the only function holding it, put, addressed by the bare name",
            bound: 1,
            detach: 0,
            held: Held::Put,
            page_size: None,
            reference: None,
        },
        Case {
            name: "the first of two, stated at creation, one entry per page",
            bound: 2,
            detach: 0,
            held: Held::AtCreate,
            page_size: Some(1),
            reference: Some(|function| format!("function:{function}")),
        },
        Case {
            name: "the last of two, put over what creation stated, two entries per page",
            bound: 2,
            detach: 1,
            held: Held::CreateThenPut,
            page_size: Some(2),
            reference: Some(|function| format!("{ACCOUNT}:function:{function}")),
        },
        Case {
            name: "the middle of three, put, one entry per page",
            bound: 3,
            detach: 1,
            held: Held::Put,
            page_size: Some(1),
            reference: Some(|function| {
                format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{function}")
            }),
        },
        Case {
            name: "the last of four, stated at creation, no page size",
            bound: 4,
            detach: 3,
            held: Held::AtCreate,
            page_size: None,
            reference: Some(|function| format!("{function}:$LATEST")),
        },
        Case {
            name: "the first of four, put over what creation stated, two entries per page",
            bound: 4,
            detach: 0,
            held: Held::CreateThenPut,
            page_size: Some(2),
            reference: None,
        },
    ]
}

#[tokio::test]
async fn dissociating_a_code_signing_config_clears_both_sides_of_the_binding() {
    for case in cases() {
        // One account per row, so the row's own functions are the only members there are.
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let label = case.name;

        // Two configurations: the one under test, and another one a neighbour function
        // holds throughout, so neither reader can pass by reporting "everything" or
        // "nothing".
        let config = create_config(&router, 0).await;
        let other = create_config(&router, 1).await;

        // The functions holding the configuration under test. The one being dissociated
        // comes to hold it the way the row says; the rest state it at creation.
        let mut functions: Vec<String> = Vec::new();
        let mut arns: Vec<String> = Vec::new();
        for index in 0..case.bound {
            let function = format!("dissoc-{}-{index}", case.bound);
            let at_create = if index == case.detach {
                match case.held {
                    Held::AtCreate => Some(config.as_str()),
                    Held::Put => None,
                    Held::CreateThenPut => Some(other.as_str()),
                }
            } else {
                Some(config.as_str())
            };
            let arn = create_function(&engine, &router, &function, at_create).await;
            if index == case.detach {
                match case.held {
                    Held::AtCreate => {}
                    Held::Put | Held::CreateThenPut => {
                        associate(label, &router, &function, &config).await;
                    }
                }
            }
            functions.push(function);
            arns.push(arn);
        }

        // A neighbour on the other configuration, and a function that holds none at all:
        // neither moves, whatever the dissociation does.
        let neighbour_arn =
            create_function(&engine, &router, "dissoc-neighbour", Some(&other)).await;
        create_function(&engine, &router, "dissoc-unbound", None).await;

        let detached = functions[case.detach].clone();
        let detached_arn = arns[case.detach].clone();
        let reference = case
            .reference
            .map_or_else(|| detached.clone(), |form| form(&detached));

        // A function nothing associated a configuration with reports none: the same 200,
        // without the member. This is the state the dissociation below puts the detached
        // function into, reached without any dissociation at all.
        let unbound = reported(label, &router, "dissoc-unbound").await;
        assert_eq!(
            unbound, None,
            "case {label:?}: a function holding no configuration must report none, got \
             {unbound:?}"
        );

        // Before: both sides report the binding.
        let before = reported(label, &router, &reference).await;
        assert_eq!(
            before,
            Some(config.clone()),
            "case {label:?}: {reference:?} must hold {config:?} before the dissociation, \
             got {before:?}"
        );
        let mut expected_before = arns.clone();
        expected_before.sort();
        let listed_before = members(label, &router, &config, case.page_size).await;
        assert_eq!(
            listed_before, expected_before,
            "case {label:?}: expected {expected_before:?} bound to the configuration \
             before the dissociation, got {listed_before:?}"
        );

        dissociate(label, &router, &reference).await;

        // After: the function's own reader reports none, and the configuration's
        // membership no longer carries it — while every other member is still there.
        let after = reported(label, &router, &reference).await;
        assert_eq!(
            after, None,
            "case {label:?}: {reference:?} must report no configuration after the \
             dissociation, got {after:?}"
        );
        let raw = get_association(&router, &reference).await;
        assert!(
            !raw.has("CodeSigningConfigArn"),
            "case {label:?}: the body must carry no CodeSigningConfigArn member after the \
             dissociation, got {}",
            raw.text()
        );
        let expected_after: Vec<String> = expected_before
            .iter()
            .filter(|arn| *arn != &detached_arn)
            .cloned()
            .collect();
        let listed_after = members(label, &router, &config, case.page_size).await;
        assert_eq!(
            listed_after, expected_after,
            "case {label:?}: expected {expected_after:?} bound to the configuration after \
             the dissociation, got {listed_after:?}"
        );
        for (index, function) in functions.iter().enumerate() {
            let expected = if index == case.detach {
                None
            } else {
                Some(config.clone())
            };
            let held = reported(label, &router, function).await;
            assert_eq!(
                held, expected,
                "case {label:?}: {function:?} must report {expected:?} after the \
                 dissociation, got {held:?}"
            );
        }

        // Nothing else's binding moved, on either side.
        let neighbour = reported(label, &router, "dissoc-neighbour").await;
        assert_eq!(
            neighbour,
            Some(other.clone()),
            "case {label:?}: the neighbour's binding must survive the dissociation, got \
             {neighbour:?}"
        );
        let other_members = members(label, &router, &other, case.page_size).await;
        assert_eq!(
            other_members,
            vec![neighbour_arn.clone()],
            "case {label:?}: the other configuration must still hold exactly its own \
             member, got {other_members:?}"
        );

        // The dissociation is not permanent, and it is the one value that moved: binding
        // the function to the OTHER configuration puts it in that configuration's
        // membership and leaves the first one's alone.
        associate(label, &router, &reference, &other).await;
        let rebound = reported(label, &router, &reference).await;
        assert_eq!(
            rebound,
            Some(other.clone()),
            "case {label:?}: the re-bound configuration must be reported, got {rebound:?}"
        );
        let mut expected_rebound = vec![neighbour_arn.clone(), detached_arn.clone()];
        expected_rebound.sort();
        let other_after = members(label, &router, &other, case.page_size).await;
        assert_eq!(
            other_after, expected_rebound,
            "case {label:?}: expected {expected_rebound:?} bound to the other \
             configuration, got {other_after:?}"
        );
        let first_after = members(label, &router, &config, case.page_size).await;
        assert_eq!(
            first_after, expected_after,
            "case {label:?}: the first configuration's membership must not take the \
             re-bound function, got {first_after:?}"
        );
    }
}

#[tokio::test]
async fn a_function_holding_no_config_reports_none_however_it_came_to_hold_none() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let config = create_config(&router, 0).await;

    // Three ways to hold nothing: never associated, dissociated once, dissociated twice
    // (the second removal asks for the state the function is already in). Every one of
    // them reads the same, because they are one stored value.
    let never = "none-never";
    create_function(&engine, &router, never, None).await;

    let once = "none-once";
    create_function(&engine, &router, once, Some(&config)).await;
    dissociate("dissociated once", &router, once).await;

    let twice = "none-twice";
    create_function(&engine, &router, twice, None).await;
    associate("dissociated twice", &router, twice, &config).await;
    dissociate("dissociated twice", &router, twice).await;
    dissociate("dissociated twice", &router, twice).await;

    for function in [never, once, twice] {
        let held = reported(function, &router, function).await;
        assert_eq!(
            held, None,
            "case {function:?}: a function holding no configuration must report none, got \
             {held:?}"
        );
        let answer = get_association(&router, function).await;
        assert!(
            !answer.has("CodeSigningConfigArn"),
            "case {function:?}: the body must carry no CodeSigningConfigArn member, got {}",
            answer.text()
        );
        // It is a REPORT of the function, not a refusal: the status is the operation's own
        // 200, and a function holding no association reports neither member of the
        // association rather than one of them.
        assert_eq!(
            (answer.status, answer.has("FunctionName")),
            (200, false),
            "case {function:?}: expected a 200 reporting no association at all, got {} {}",
            answer.status,
            answer.text()
        );
    }

    // And the configuration nothing holds any more has an empty membership, so its own
    // delete is no longer refused.
    let empty = members("drained", &router, &config, Some(1)).await;
    assert!(
        empty.is_empty(),
        "expected no functions bound to the drained configuration, got {empty:?}"
    );
}
