// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListFunctionsByCodeSigningConfig`: "List the functions that use
//! the specified code signing configuration. You can use this method prior to deleting a
//! code signing configuration, to verify that no functions are using it."
//!
//! The rule under test: the operation reports the ARN of every function whose stored
//! function-level code-signing association names the configuration the request addresses
//! — exactly the functions `GetFunctionCodeSigningConfig` reports that configuration for,
//! no more and no fewer — in a stable order (by the function ARN, the only member
//! reported and therefore the pagination token), one page of at most `MaxItems` entries
//! starting after the `Marker` the previous page answered and carrying a `NextMarker`
//! only while functions remain; an ARN naming a configuration this account does not hold
//! is the configuration's own 404 rather than an empty page; and the call changes nothing
//! a later caller sees.
//!
//! Every row drives real HTTP requests through the router and cross-checks the listing
//! against `GetFunctionCodeSigningConfig`, so an ARN the listing invented rather than read
//! fails here. The rows differ along what the rule generalises over: how many functions
//! hold the configuration (none, one, several), which of several configurations is
//! addressed, how the association came to be (stated on `CreateFunction`, set by
//! `PutFunctionCodeSigningConfig`, replaced, removed again), whether the function has
//! published versions, the page size (each published bound of `MaxItems`, sizes between
//! and no size at all) and where the page starts (no marker, a marker naming a function, a
//! marker before every function, a marker past every one).

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
/// `smithy_query(query="operation", operation="ListFunctionsByCodeSigningConfig")`.
const ABSENT_ARN: &str =
    "arn:aws:lambda:us-east-1:000000000000:code-signing-config:csc-0123456789abcdef0";

/// A second well-formed ARN naming no configuration, differing from [`ABSENT_ARN`] only in
/// its id, so a refusal cannot be keyed to one particular string.
const OTHER_ABSENT_ARN: &str =
    "arn:aws:lambda:us-east-1:000000000000:code-signing-config:csc-zzzz9999zzzz9999z";

/// The bounds the model puts on `MaxItems` (`MaxListItems`, `@range(min: 1, max: 10000)`),
/// from `smithy_query(query="operation", operation="ListFunctionsByCodeSigningConfig")`.
const MAX_ITEMS_MIN: i64 = 1;
/// See [`MAX_ITEMS_MIN`].
const MAX_ITEMS_MAX: i64 = 10_000;

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

/// `ListFunctionsByCodeSigningConfig` on the configuration `arn` names, with the query
/// string `query` (`""` for no parameters). The operation is bound at
/// `GET /2020-04-22/code-signing-configs/{CodeSigningConfigArn}/functions`.
async fn list_functions(router: &MultiServiceRouter, arn: &str, query: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{CODE_SIGNING_CONFIGS_URI}/{arn}/functions{query}"),
        &Value::Null,
    )
    .await
}

/// Where the association operations are bound
/// (`smithy_query(query="operation", operation="PutFunctionCodeSigningConfig")`:
/// `PUT|GET|DELETE /2020-06-30/functions/{FunctionName}/code-signing-config`).
fn association_uri(reference: &str) -> String {
    format!("/2020-06-30/functions/{reference}/code-signing-config")
}

/// `PutFunctionCodeSigningConfig`, which must succeed.
async fn associate(router: &MultiServiceRouter, function: &str, config_arn: &str) {
    let answer = call(
        router,
        "PUT",
        &association_uri(function),
        &json!({ "CodeSigningConfigArn": config_arn }),
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case associate {function:?} with {config_arn:?}: expected 200, got {} {}",
        answer.status,
        answer.text()
    );
}

/// `DeleteFunctionCodeSigningConfig`, which must succeed.
async fn disassociate(router: &MultiServiceRouter, function: &str) {
    let answer = call(
        router,
        "DELETE",
        &association_uri(function),
        &Value::Null,
    )
    .await;
    assert_eq!(
        answer.status,
        204,
        "case disassociate {function:?}: expected 204, got {} {}",
        answer.status,
        answer.text()
    );
}

/// The association `GetFunctionCodeSigningConfig` reports for `function`: the ARN, or
/// `None` when the function holds none.
async fn reported_association(
    case: &str,
    router: &MultiServiceRouter,
    function: &str,
) -> Option<String> {
    let answer = call(router, "GET", &association_uri(function), &Value::Null).await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: expected 200 from GetFunctionCodeSigningConfig on {function:?}, \
         got {} {}",
        answer.status,
        answer.text()
    );
    answer
        .string("CodeSigningConfigArn")
        .filter(|arn| !arn.is_empty())
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
/// function ARN it persisted.
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

/// `PublishVersion`, which must succeed: a published version is not a separate user of
/// the configuration, so it must not add an entry to the listing.
async fn publish_version(router: &MultiServiceRouter, function: &str) {
    let answer = call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{function}/versions"),
        &json!({}),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "PublishVersion on {function:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
}

/// How a function's association is set up for a row.
enum Setup {
    /// Stated on `CreateFunction`, as an index into the row's configurations.
    AtCreate(usize),
    /// Set by `PutFunctionCodeSigningConfig`, in order: the last one is what is held.
    Puts(&'static [usize]),
    /// Set by `PutFunctionCodeSigningConfig` and then removed by
    /// `DeleteFunctionCodeSigningConfig`, so the function holds none.
    PutThenRemoved(usize),
    /// Never associated with anything.
    None,
}

/// One function of a row: its name, how its association is set, the configuration it then
/// holds, and whether it has published versions.
struct Subject {
    function: &'static str,
    setup: Setup,
    /// The configuration it must then be listed under; `None` means none at all.
    holds: Option<usize>,
    versions: usize,
}

/// The functions every listing row shares: three configurations exist, so a listing that
/// reported "every function there is" would fail.
fn subjects() -> Vec<Subject> {
    vec![
        Subject {
            function: "csc-list-first",
            setup: Setup::Puts(&[0]),
            holds: Some(0),
            versions: 0,
        },
        Subject {
            function: "csc-list-second",
            setup: Setup::AtCreate(0),
            holds: Some(0),
            versions: 2,
        },
        Subject {
            function: "csc-list-third",
            setup: Setup::Puts(&[1, 0]),
            holds: Some(0),
            versions: 1,
        },
        Subject {
            function: "csc-list-fourth",
            setup: Setup::Puts(&[1]),
            holds: Some(1),
            versions: 0,
        },
        Subject {
            function: "csc-list-fifth",
            setup: Setup::AtCreate(1),
            holds: Some(1),
            versions: 1,
        },
        Subject {
            function: "csc-list-sixth",
            setup: Setup::PutThenRemoved(0),
            holds: None,
            versions: 0,
        },
        Subject {
            function: "csc-list-seventh",
            setup: Setup::None,
            holds: None,
            versions: 0,
        },
        Subject {
            function: "csc-list-eighth",
            setup: Setup::Puts(&[0, 1, 0, 2]),
            holds: Some(2),
            versions: 0,
        },
    ]
}

/// Stand the account up: three configurations and every subject above, answering the
/// configuration ARNs and, per configuration index, the function ARNs that must be listed
/// under it, in the order a listing reports them (by function ARN).
async fn populate(
    engine: &Arc<engine::InMemoryEngine>,
    router: &MultiServiceRouter,
) -> (Vec<String>, Vec<Vec<String>>) {
    let mut configs: Vec<String> = Vec::new();
    for index in 0..3 {
        configs.push(create_config(router, index).await);
    }
    let mut expected: Vec<Vec<String>> = vec![Vec::new(); configs.len()];

    for subject in subjects() {
        let at_create = match subject.setup {
            Setup::AtCreate(index) => Some(configs[index].as_str()),
            _ => None,
        };
        let arn = create_function(engine, router, subject.function, at_create).await;
        match subject.setup {
            Setup::Puts(indices) => {
                for index in indices {
                    associate(router, subject.function, &configs[*index]).await;
                }
            }
            Setup::PutThenRemoved(index) => {
                associate(router, subject.function, &configs[index]).await;
                disassociate(router, subject.function).await;
            }
            Setup::AtCreate(_) | Setup::None => {}
        }
        for _ in 0..subject.versions {
            publish_version(router, subject.function).await;
        }
        if let Some(index) = subject.holds {
            expected[index].push(arn);
        }
    }

    for arns in &mut expected {
        arns.sort();
    }
    (configs, expected)
}

#[tokio::test]
async fn list_functions_by_code_signing_config_reports_every_associated_function() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // A configuration nobody uses: an empty list and no marker, never a refusal.
    let fresh = create_config(&router, 7).await;
    let empty = list_functions(&router, &fresh, "").await;
    assert_eq!(
        (empty.status, empty.function_arns(), empty.next_marker()),
        (200, Vec::new(), None),
        "case no functions: expected 200 with an empty list and no marker, got {} {}",
        empty.status,
        empty.text()
    );

    let (configs, expected) = populate(&engine, &router).await;

    for (index, config) in configs.iter().enumerate() {
        let label = format!("configuration {index}");
        let listed = list_functions(&router, config, "").await;
        assert_eq!(
            (listed.status, listed.function_arns(), listed.next_marker()),
            (200, expected[index].clone(), None),
            "case {label:?}: expected {:?} in ARN order and no marker, got {} {}",
            expected[index],
            listed.status,
            listed.text()
        );

        // Every reported ARN belongs to a function whose own association reader agrees,
        // and every function that agrees is reported: the two surfaces read one record.
        for arn in listed.function_arns() {
            let function = arn.rsplit(':').next().unwrap_or_default().to_string();
            let held = reported_association(&label, &router, &function).await;
            assert_eq!(
                held,
                Some(config.clone()),
                "case {label:?}: {arn:?} was listed, so GetFunctionCodeSigningConfig must \
                 report {config:?} for it, got {held:?}"
            );
        }
        for subject in subjects() {
            let held = reported_association(&label, &router, subject.function).await;
            let listed_here = listed
                .function_arns()
                .iter()
                .any(|arn| arn.ends_with(&format!(":{}", subject.function)));
            assert_eq!(
                listed_here,
                held.as_deref() == Some(config.as_str()),
                "case {label:?}: {:?} holds {held:?}, so being listed must be {}, got {}",
                subject.function,
                held.as_deref() == Some(config.as_str()),
                listed_here
            );
        }

        // The call only reports: a second identical call answers the same thing.
        let again = list_functions(&router, config, "").await;
        assert_eq!(
            (again.function_arns(), again.next_marker()),
            (listed.function_arns(), listed.next_marker()),
            "case {label:?}: expected the same answer twice, got {} then {}",
            listed.text(),
            again.text()
        );
    }

    // The fresh configuration is still unused after everything above: a function is
    // listed only under the configuration it holds.
    let still_empty = list_functions(&router, &fresh, "").await;
    assert_eq!(
        (still_empty.status, still_empty.function_arns()),
        (200, Vec::new()),
        "case an unused configuration: expected an empty list, got {} {}",
        still_empty.status,
        still_empty.text()
    );
}

/// One row of the paging table: the page size asked for, and the most entries a page may
/// then carry.
struct PagingCase {
    name: &'static str,
    max_items: Option<i64>,
    page_limit: usize,
}

#[tokio::test]
async fn list_functions_by_code_signing_config_pages_over_every_function() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let config = create_config(&router, 0).await;

    // Enough functions that a small page size is not the whole answer, so a marker has to
    // be followed to see them all.
    let mut expected: Vec<String> = Vec::new();
    for index in 0..7 {
        let name = format!("csc-paged-{index}");
        let arn = create_function(&engine, &router, &name, None).await;
        associate(&router, &name, &config).await;
        expected.push(arn);
    }
    expected.sort();

    let cases = [
        PagingCase {
            name: "one at a time, the smallest page the model admits",
            max_items: Some(MAX_ITEMS_MIN),
            page_limit: 1,
        },
        PagingCase {
            name: "two at a time, a size that does not divide the total",
            max_items: Some(2),
            page_limit: 2,
        },
        PagingCase {
            name: "a size one short of the total",
            max_items: Some(6),
            page_limit: 6,
        },
        PagingCase {
            name: "a size exactly the total",
            max_items: Some(7),
            page_limit: 7,
        },
        PagingCase {
            name: "a size larger than the total",
            max_items: Some(50),
            page_limit: 50,
        },
        PagingCase {
            name: "the largest page the model admits",
            max_items: Some(MAX_ITEMS_MAX),
            page_limit: expected.len(),
        },
        PagingCase {
            name: "no page size at all",
            max_items: None,
            page_limit: expected.len(),
        },
    ];

    for case in cases {
        let label = case.name;
        let mut seen: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        // One more round than pages can possibly be needed, so a marker that never
        // settles fails the assertion below rather than looping forever.
        for _ in 0..expected.len() + 2 {
            let mut query: Vec<String> = Vec::new();
            if let Some(max_items) = case.max_items {
                query.push(format!("MaxItems={max_items}"));
            }
            if let Some(marker) = marker.as_deref() {
                query.push(format!("Marker={marker}"));
            }
            let query = if query.is_empty() {
                String::new()
            } else {
                format!("?{}", query.join("&"))
            };

            let page = list_functions(&router, &config, &query).await;
            assert_eq!(
                page.status,
                200,
                "case {label:?}: expected 200 from the listing{query}, got {} {}",
                page.status,
                page.text()
            );
            let arns = page.function_arns();
            assert!(
                arns.len() <= case.page_limit,
                "case {label:?}: expected at most {} entries per page, got {} ({arns:?})",
                case.page_limit,
                arns.len()
            );
            seen.extend(arns.iter().cloned());

            match page.next_marker() {
                Some(next) => {
                    // A marker is answered only while functions remain, and it is the key
                    // of the last entry this page reported.
                    assert_eq!(
                        Some(next.clone()),
                        arns.last().cloned(),
                        "case {label:?}: expected the marker to be the last entry of the \
                         page, got {next:?} after {arns:?}"
                    );
                    assert!(
                        seen.len() < expected.len(),
                        "case {label:?}: expected no marker once every function was \
                         reported, got {next:?} after {seen:?}"
                    );
                    marker = Some(next);
                }
                None => break,
            }
        }

        assert_eq!(
            seen, expected,
            "case {label:?}: expected the pages to concatenate to {expected:?} with \
             nothing skipped or repeated, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn list_functions_by_code_signing_config_starts_the_page_after_the_marker() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let (configs, expected) = populate(&engine, &router).await;
    let config = &configs[0];
    let held = &expected[0];
    assert!(
        held.len() > 2,
        "the fixture must hold several functions under the first configuration, got \
         {held:?}"
    );

    // A marker naming a function this configuration holds: the page starts at the first
    // function ordered after it, so that function is not reported twice.
    for (index, arn) in held.iter().enumerate() {
        let page = list_functions(&router, config, &format!("?Marker={arn}")).await;
        let remaining: Vec<String> = held[index + 1..].to_vec();
        assert_eq!(
            (page.status, page.function_arns()),
            (200, remaining.clone()),
            "case marker {arn:?}: expected {remaining:?} after it, got {} {}",
            page.status,
            page.text()
        );
    }

    // A marker ordered BEFORE every function names none of them, and the page still
    // starts at the first function after it: the whole ordered sequence.
    let before = list_functions(&router, config, "?Marker=arn:aws:lambda:").await;
    assert_eq!(
        (before.status, before.function_arns()),
        (200, held.clone()),
        "case a marker before every function: expected {held:?}, got {} {}",
        before.status,
        before.text()
    );

    // A marker ordered AFTER every function leaves nothing to report, and nothing to
    // continue after either.
    let after = list_functions(&router, config, "?Marker=zzz-past-every-function").await;
    assert_eq!(
        (after.status, after.function_arns(), after.next_marker()),
        (200, Vec::new(), None),
        "case a marker past every function: expected an empty page and no marker, got {} {}",
        after.status,
        after.text()
    );
}

#[tokio::test]
async fn list_functions_by_code_signing_config_refuses_a_page_size_outside_the_modelled_range() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let config = create_config(&router, 0).await;
    let mut expected: Vec<String> = Vec::new();
    for index in 0..3 {
        let name = format!("csc-bounded-{index}");
        let arn = create_function(&engine, &router, &name, Some(&config)).await;
        expected.push(arn);
    }
    expected.sort();

    // `MaxItems` is `@range(min: 1, max: 10000)`: below the minimum and past the maximum
    // are refused rather than clamped into a page.
    for (label, max_items) in [
        ("zero", "0"),
        ("negative", "-1"),
        ("one past the maximum", "10001"),
        ("far past the maximum", "1000000"),
    ] {
        let answer = list_functions(&router, &config, &format!("?MaxItems={max_items}")).await;
        assert_eq!(
            answer.status,
            400,
            "case MaxItems {label}: expected 400, got {} {}",
            answer.status,
            answer.text()
        );
    }

    // Each published bound is served: the minimum answers one function and a marker, the
    // maximum answers all three and none.
    let smallest = list_functions(&router, &config, &format!("?MaxItems={MAX_ITEMS_MIN}")).await;
    assert_eq!(
        (
            smallest.status,
            smallest.function_arns(),
            smallest.next_marker()
        ),
        (200, expected[..1].to_vec(), Some(expected[0].clone())),
        "case MaxItems at the minimum: expected the first function and its marker, got {} {}",
        smallest.status,
        smallest.text()
    );
    let largest = list_functions(&router, &config, &format!("?MaxItems={MAX_ITEMS_MAX}")).await;
    assert_eq!(
        (
            largest.status,
            largest.function_arns(),
            largest.next_marker()
        ),
        (200, expected.clone(), None),
        "case MaxItems at the maximum: expected {expected:?} and no marker, got {} {}",
        largest.status,
        largest.text()
    );
}

#[tokio::test]
async fn an_arn_naming_no_code_signing_configuration_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // One configuration exists and is used, so the refusals below are about the ARN
    // naming no configuration rather than about the account holding none.
    let held = create_config(&router, 0).await;
    create_function(&engine, &router, "csc-absent-subject", Some(&held)).await;

    for arn in [ABSENT_ARN, OTHER_ABSENT_ARN] {
        let refused = list_functions(&router, arn, "").await;
        assert_eq!(
            refused.status,
            404,
            "case {arn:?}: expected 404 listing a configuration this account does not \
             hold, got {} {}",
            refused.status,
            refused.text()
        );
        assert!(
            refused.error_code().contains("ResourceNotFoundException"),
            "case {arn:?}: expected ResourceNotFoundException, got {:?}",
            refused.error_code()
        );
        let sentence = format!("The Lambda code signing configuration {arn} can not be found.");
        assert!(
            refused.text().contains(&sentence),
            "case {arn:?}: expected the refusal to read {sentence:?}, got {}",
            refused.text()
        );
    }

    // A configuration that was DELETED reads exactly like one that never existed: the
    // same 404, not an empty page.
    let doomed = create_config(&router, 1).await;
    let listed = list_functions(&router, &doomed, "").await;
    assert_eq!(
        (listed.status, listed.function_arns()),
        (200, Vec::new()),
        "the configuration under test must list no functions before it is deleted, got {} {}",
        listed.status,
        listed.text()
    );
    let deleted = call(
        &router,
        "DELETE",
        &format!("{CODE_SIGNING_CONFIGS_URI}/{doomed}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        deleted.status,
        204,
        "DeleteCodeSigningConfig must succeed, got {} {}",
        deleted.status,
        deleted.text()
    );
    let gone = list_functions(&router, &doomed, "").await;
    assert_eq!(
        gone.status,
        404,
        "case a deleted configuration: expected 404, got {} {}",
        gone.status,
        gone.text()
    );

    // None of the refusals changed what the configuration that does exist reports.
    let after = list_functions(&router, &held, "").await;
    assert_eq!(
        after.function_arns().len(),
        1,
        "a refused listing must leave the stored associations alone, got {}",
        after.text()
    );
}
